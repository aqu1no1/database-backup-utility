pub mod connection;
pub mod secret;

pub use connection::{ConfigError, Password, PostgresConnectionConfig, TlsMode, TlsRequirement};
pub use secret::{
    MAX_PASSWORD_FILE_BYTES, SecretError, SecretProvider, SecretSource, read_env,
    read_password_file,
};

pub const PROJECT_NAME: &str = "Database Backup Utility";

#[cfg(test)]
mod tests {
    use super::PROJECT_NAME;

    #[test]
    fn project_name_is_not_empty() {
        assert!(!PROJECT_NAME.is_empty());
    }
}
