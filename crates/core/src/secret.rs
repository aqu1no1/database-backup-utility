use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::{Password, PostgresConnectionConfig};

pub const MAX_PASSWORD_FILE_BYTES: u64 = 4 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretSource {
    Prompt,
    Env(&'static str),
    File(PathBuf),
}

impl SecretSource {
    #[must_use]
    pub fn secret_ref(&self) -> String {
        match self {
            Self::Prompt => "prompt".to_owned(),
            Self::Env(name) => format!("env:{name}"),
            Self::File(path) => format!("file:{}", path.display()),
        }
    }
}

impl fmt::Display for SecretSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.secret_ref())
    }
}

#[derive(Debug, Error)]
pub enum SecretError {
    #[error("senha não informada: {0}")]
    Missing(String),
    #[error("mais de uma fonte de senha definida: {}; use só uma", .0.join(", "))]
    Ambiguous(Vec<String>),
    #[error("arquivo de senha não encontrado: {}", .0.display())]
    NotFound(PathBuf),
    #[error("arquivo de senha recusado ({reason}): {}", path.display())]
    UnsafeFile { path: PathBuf, reason: &'static str },
    #[error("arquivo de senha maior que {MAX_PASSWORD_FILE_BYTES} bytes: {}", .0.display())]
    TooLarge(PathBuf),
    #[error("arquivo de senha vazio: {}", .0.display())]
    Empty(PathBuf),
    #[error("arquivo de senha não está em UTF-8: {}", .0.display())]
    NotUtf8(PathBuf),
    #[error("não foi possível ler o arquivo de senha {}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("não foi possível ler a senha do terminal")]
    Prompt,
}

pub trait SecretProvider {
    fn password(
        &self,
        config: &PostgresConnectionConfig,
    ) -> Result<(Password, SecretSource), SecretError>;
}

#[must_use]
pub fn read_env(name: &str) -> Option<Password> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .map(Password::new)
}

pub fn read_password_file(path: &Path) -> Result<Password, SecretError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|source| match source.kind() {
        io::ErrorKind::NotFound => SecretError::NotFound(path.to_owned()),
        _ => SecretError::Io {
            path: path.to_owned(),
            source,
        },
    })?;
    let unsafe_file = |reason| SecretError::UnsafeFile {
        path: path.to_owned(),
        reason,
    };
    if metadata.file_type().is_symlink() {
        return Err(unsafe_file("link simbólico"));
    }
    if !metadata.is_file() {
        return Err(unsafe_file("não é um arquivo regular"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(unsafe_file(
                "acessível por grupo ou outros usuários; use chmod 600",
            ));
        }
    }
    if metadata.len() > MAX_PASSWORD_FILE_BYTES {
        return Err(SecretError::TooLarge(path.to_owned()));
    }

    let bytes = std::fs::read(path).map_err(|source| SecretError::Io {
        path: path.to_owned(),
        source,
    })?;
    let mut value = match String::from_utf8(bytes) {
        Ok(value) => value,
        Err(error) => {
            let mut bytes = error.into_bytes();
            bytes.iter_mut().for_each(|byte| *byte = 0);
            std::hint::black_box(&bytes);
            return Err(SecretError::NotUtf8(path.to_owned()));
        }
    };
    if value.ends_with("\r\n") {
        value.truncate(value.len() - 2);
    } else if value.ends_with('\n') {
        value.truncate(value.len() - 1);
    }
    let password = Password::new(value);
    if password.expose().is_empty() {
        return Err(SecretError::Empty(path.to_owned()));
    }
    Ok(password)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const SECRET: &str = "s3nh@-de-teste";

    #[cfg(unix)]
    fn file(dir: &TempDir, name: &str, body: &[u8], mode: u32) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.path().join(name);
        std::fs::write(&path, body).expect("arquivo");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).expect("modo");
        path
    }

    #[cfg(unix)]
    #[test]
    fn accepts_private_files_and_strips_one_trailing_newline() {
        let dir = TempDir::new().expect("dir");
        let cases: [(&[u8], u32, &str); 4] = [
            (b"s3nh@-de-teste\n", 0o600, SECRET),
            (b"s3nh@-de-teste\r\n", 0o400, SECRET),
            (b"  com espacos  \n", 0o600, "  com espacos  "),
            (b"duas\n\n", 0o600, "duas\n"),
        ];

        for (index, (body, mode, expected)) in cases.into_iter().enumerate() {
            let path = file(&dir, &format!("pass-{index}"), body, mode);
            let password = read_password_file(&path).expect("arquivo válido");
            assert_eq!(password.expose(), expected);
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_files_readable_or_writable_by_others() {
        let dir = TempDir::new().expect("dir");
        for (index, mode) in [0o640, 0o644, 0o604, 0o620, 0o602].into_iter().enumerate() {
            let path = file(&dir, &format!("pass-{index}"), SECRET.as_bytes(), mode);
            let error = read_password_file(&path).expect_err("permissão aberta");
            assert!(matches!(error, SecretError::UnsafeFile { .. }), "{mode:o}");
            assert!(!error.to_string().contains(SECRET));
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinks_directories_and_bad_content() {
        let dir = TempDir::new().expect("dir");
        let target = file(&dir, "real", SECRET.as_bytes(), 0o600);
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        let empty = file(&dir, "vazio", b"\n", 0o600);
        let large = file(&dir, "grande", &vec![b'a'; 4097], 0o600);
        let binary = file(&dir, "binario", &[0xff, 0xfe, 0x00], 0o600);

        assert!(matches!(
            read_password_file(&link),
            Err(SecretError::UnsafeFile { .. })
        ));
        assert!(matches!(
            read_password_file(dir.path()),
            Err(SecretError::UnsafeFile { .. })
        ));
        assert!(matches!(
            read_password_file(&empty),
            Err(SecretError::Empty(_))
        ));
        assert!(matches!(
            read_password_file(&large),
            Err(SecretError::TooLarge(_))
        ));
        assert!(matches!(
            read_password_file(&binary),
            Err(SecretError::NotUtf8(_))
        ));
        assert!(matches!(
            read_password_file(&dir.path().join("ausente")),
            Err(SecretError::NotFound(_))
        ));
    }

    #[test]
    fn source_reference_never_contains_the_secret() {
        let file = SecretSource::File(PathBuf::from("/etc/dbu/pg.pass"));
        assert_eq!(SecretSource::Prompt.secret_ref(), "prompt");
        assert_eq!(
            SecretSource::Env("DBU_PG_PASSWORD").secret_ref(),
            "env:DBU_PG_PASSWORD"
        );
        assert_eq!(file.secret_ref(), "file:/etc/dbu/pg.pass");
        let ambiguous =
            SecretError::Ambiguous(vec!["--password-file".into(), "DBU_PG_PASSWORD".into()]);
        assert_eq!(
            ambiguous.to_string(),
            "mais de uma fonte de senha definida: --password-file, DBU_PG_PASSWORD; use só uma"
        );
    }
}
