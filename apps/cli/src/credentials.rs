use std::fmt;
use std::io::IsTerminal;
use std::path::PathBuf;

use database_backup_core::{
    CredentialStore, DatabaseEngine, Password, SecretError, SecretProvider, SecretSource, read_env,
    read_keyring, read_password_file,
};
use database_backup_operations::keychain::Keychain;

pub const PASSWORD_FILE_FLAG: &str = "--password-file";
const KEYRING_SOURCE: &str = "keychain do perfil";

pub struct CliSecrets {
    pub password_file: Option<PathBuf>,
    pub keyring: Option<String>,
    pub engine: DatabaseEngine,
}

impl CliSecrets {
    fn env_names(&self) -> (&'static str, &'static str) {
        match self.engine {
            DatabaseEngine::Mysql => ("DBU_MYSQL_PASSWORD", "DBU_MYSQL_PASSWORD_FILE"),
            DatabaseEngine::Mariadb => ("DBU_MARIADB_PASSWORD", "DBU_MARIADB_PASSWORD_FILE"),
            DatabaseEngine::Mongodb => ("DBU_MONGO_PASSWORD", "DBU_MONGO_PASSWORD_FILE"),
            DatabaseEngine::Postgres | DatabaseEngine::Sqlite => {
                ("DBU_PG_PASSWORD", "DBU_PG_PASSWORD_FILE")
            }
        }
    }
}

impl SecretProvider for CliSecrets {
    fn password(&self, target: &dyn fmt::Display) -> Result<(Password, SecretSource), SecretError> {
        self.password_with(target, &Keychain::system())
    }
}

impl CliSecrets {
    fn password_with(
        &self,
        target: &dyn fmt::Display,
        keychain: &dyn CredentialStore,
    ) -> Result<(Password, SecretSource), SecretError> {
        let (password_env, password_file_env) = self.env_names();
        let env_file = std::env::var_os(password_file_env)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        let env_password = read_env(password_env);

        let mut configured = Vec::new();
        if self.password_file.is_some() {
            configured.push(PASSWORD_FILE_FLAG.to_owned());
        }
        if self.keyring.is_some() {
            configured.push(KEYRING_SOURCE.to_owned());
        }
        if env_file.is_some() {
            configured.push(password_file_env.to_owned());
        }
        if env_password.is_some() {
            configured.push(password_env.to_owned());
        }
        if configured.len() > 1 {
            return Err(SecretError::Ambiguous(configured));
        }

        if let Some(profile) = &self.keyring {
            return read_keyring(keychain, profile);
        }
        if let Some(path) = self.password_file.clone().or(env_file) {
            let path = std::path::absolute(&path).unwrap_or(path);
            let password = read_password_file(&path)?;
            return Ok((password, SecretSource::File(path)));
        }
        if let Some(password) = env_password {
            return Ok((password, SecretSource::Env(password_env)));
        }
        if !std::io::stdin().is_terminal() {
            return Err(SecretError::Missing(format!(
                "use {PASSWORD_FILE_FLAG}, {password_file_env} ou {password_env}, ou execute em um terminal interativo"
            )));
        }
        rpassword::prompt_password(format!("Senha para {target}: "))
            .map(|value| (Password::new(value), SecretSource::Prompt))
            .map_err(|_| SecretError::Prompt)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::*;

    const SECRET: &str = "s3nh@-do-keychain";

    #[derive(Default)]
    struct MemoryKeychain(RefCell<HashMap<String, String>>);

    impl CredentialStore for MemoryKeychain {
        fn get(&self, account: &str) -> Result<Option<Password>, SecretError> {
            Ok(self.0.borrow().get(account).cloned().map(Password::new))
        }

        fn set(&self, account: &str, password: &Password) -> Result<(), SecretError> {
            self.0
                .borrow_mut()
                .insert(account.to_owned(), password.expose().to_owned());
            Ok(())
        }

        fn delete(&self, account: &str) -> Result<bool, SecretError> {
            Ok(self.0.borrow_mut().remove(account).is_some())
        }
    }

    fn secrets(password_file: Option<&str>, keyring: Option<&str>) -> CliSecrets {
        CliSecrets {
            password_file: password_file.map(PathBuf::from),
            keyring: keyring.map(str::to_owned),
            engine: DatabaseEngine::Mongodb,
        }
    }

    #[test]
    fn keyring_profile_reads_the_saved_password_without_prompting() {
        let keychain = MemoryKeychain::default();
        keychain
            .set("prod", &Password::new(SECRET.to_owned()))
            .unwrap();

        let (password, source) = secrets(None, Some("prod"))
            .password_with(&"mongo", &keychain)
            .unwrap();
        assert_eq!(password.expose(), SECRET);
        assert_eq!(source.secret_ref(), "keyring:prod");

        let missing = secrets(None, Some("staging"))
            .password_with(&"mongo", &keychain)
            .unwrap_err();
        assert!(matches!(missing, SecretError::KeychainMissing(_)));
    }

    #[test]
    fn keyring_and_password_file_together_are_ambiguous() {
        let error = secrets(Some("/secrets/mongo.pass"), Some("prod"))
            .password_with(&"mongo", &MemoryKeychain::default())
            .unwrap_err();
        assert!(matches!(error, SecretError::Ambiguous(ref sources) if sources.len() == 2));
        assert!(error.to_string().contains(KEYRING_SOURCE));
    }
}
