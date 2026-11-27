use std::sync::Arc;

use database_backup_core::{CredentialStore, Password, SecretError};
use keyring_core::{Entry, Error};

pub const KEYCHAIN_SERVICE: &str = "dev.aqu1no1.database-backup-utility";

type PlatformStore = Arc<keyring_core::CredentialStore>;

#[derive(Clone, Default)]
pub struct Keychain {
    store: Option<PlatformStore>,
}

impl Keychain {
    #[must_use]
    pub fn system() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_store(store: PlatformStore) -> Self {
        Self { store: Some(store) }
    }

    fn entry(&self, account: &str) -> Result<Entry, SecretError> {
        let store = match &self.store {
            Some(store) => Arc::clone(store),
            None => platform_store()?,
        };
        Entry::new_in_store(&store, KEYCHAIN_SERVICE, account).map_err(sanitize)
    }
}

impl CredentialStore for Keychain {
    fn get(&self, account: &str) -> Result<Option<Password>, SecretError> {
        match self.entry(account)?.get_password() {
            Ok(value) => Ok(Some(Password::new(value))),
            Err(Error::NoEntry) => Ok(None),
            Err(error) => Err(sanitize(error)),
        }
    }

    fn set(&self, account: &str, password: &Password) -> Result<(), SecretError> {
        self.entry(account)?
            .set_password(password.expose())
            .map_err(sanitize)
    }

    fn delete(&self, account: &str) -> Result<bool, SecretError> {
        match self.entry(account)?.delete_credential() {
            Ok(()) => Ok(true),
            Err(Error::NoEntry) => Ok(false),
            Err(error) => Err(sanitize(error)),
        }
    }
}

fn sanitize(error: Error) -> SecretError {
    match error {
        Error::NoStorageAccess(source) | Error::PlatformFailure(source) => {
            SecretError::KeychainUnavailable(source.to_string())
        }
        Error::NoDefaultStore => {
            SecretError::KeychainUnavailable("nenhum keychain configurado".to_owned())
        }
        Error::NotSupportedByStore(reason) => SecretError::KeychainUnavailable(reason),
        Error::BadEncoding(_) | Error::BadDataFormat(..) => {
            SecretError::Keychain("a senha salva não está em UTF-8".to_owned())
        }
        Error::TooLong(field, limit) => {
            SecretError::Keychain(format!("{field} passa do limite de {limit}"))
        }
        Error::Invalid(field, reason) => SecretError::Keychain(format!("{field}: {reason}")),
        Error::Ambiguous(_) => {
            SecretError::Keychain("há mais de uma senha salva para o perfil".to_owned())
        }
        Error::BadStoreFormat(reason) => SecretError::Keychain(reason),
        _ => SecretError::Keychain("erro desconhecido".to_owned()),
    }
}

#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "android"))
))]
fn platform_store() -> Result<PlatformStore, SecretError> {
    let store: PlatformStore = zbus_secret_service_keyring_store::Store::new().map_err(sanitize)?;
    Ok(store)
}

#[cfg(target_os = "macos")]
fn platform_store() -> Result<PlatformStore, SecretError> {
    let store: PlatformStore =
        apple_native_keyring_store::keychain::Store::new().map_err(sanitize)?;
    Ok(store)
}

#[cfg(target_os = "windows")]
fn platform_store() -> Result<PlatformStore, SecretError> {
    let store: PlatformStore = windows_native_keyring_store::Store::new().map_err(sanitize)?;
    Ok(store)
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "windows",
    all(
        unix,
        not(any(target_os = "macos", target_os = "ios", target_os = "android"))
    )
)))]
fn platform_store() -> Result<PlatformStore, SecretError> {
    Err(SecretError::KeychainUnavailable(
        "sistema operacional sem keychain suportado".to_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use database_backup_core::read_keyring;
    use keyring_core::mock;

    use super::*;

    const SECRET: &str = "s3nh@-do-keychain";

    fn keychain() -> (Keychain, PlatformStore) {
        let store: PlatformStore = mock::Store::new().unwrap();
        (Keychain::with_store(Arc::clone(&store)), store)
    }

    fn fail_next_call(store: &PlatformStore, account: &str, error: Error) {
        let entry = Entry::new_in_store(store, KEYCHAIN_SERVICE, account).unwrap();
        let cred: &mock::Cred = entry.as_any().downcast_ref().unwrap();
        cred.set_error(error);
    }

    #[test]
    fn saves_reads_and_deletes_the_password_of_one_profile() {
        let (keychain, _) = keychain();
        keychain
            .set("prod", &Password::new(SECRET.to_owned()))
            .unwrap();

        let (password, source) = read_keyring(&keychain, "prod").unwrap();
        assert_eq!(password.expose(), SECRET);
        assert_eq!(source.secret_ref(), "keyring:prod");
        assert!(keychain.get("staging").unwrap().is_none());

        assert!(keychain.delete("prod").unwrap());
        assert!(keychain.get("prod").unwrap().is_none());
        assert!(!keychain.delete("prod").unwrap());
    }

    #[test]
    fn storage_failures_become_unavailable_without_leaking_the_password() {
        let (keychain, store) = keychain();
        keychain
            .set("prod", &Password::new(SECRET.to_owned()))
            .unwrap();
        fail_next_call(
            &store,
            "prod",
            Error::NoStorageAccess("Secret Service não respondeu".into()),
        );

        let error = keychain.get("prod").unwrap_err();
        assert!(matches!(error, SecretError::KeychainUnavailable(_)));
        assert!(error.to_string().contains("--password-file"));
        assert!(!error.to_string().contains(SECRET));

        fail_next_call(
            &store,
            "prod",
            Error::BadEncoding(SECRET.as_bytes().to_vec()),
        );
        let error = keychain.get("prod").unwrap_err();
        assert!(matches!(error, SecretError::Keychain(_)));
        assert!(!error.to_string().contains(SECRET));
    }
}
