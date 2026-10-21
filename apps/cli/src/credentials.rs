use std::io::IsTerminal;
use std::path::PathBuf;

use database_backup_core::{
    Password, PostgresConnectionConfig, SecretError, SecretProvider, SecretSource, read_env,
    read_password_file,
};

pub const PASSWORD_ENV: &str = "DBU_PG_PASSWORD";
pub const PASSWORD_FILE_ENV: &str = "DBU_PG_PASSWORD_FILE";
pub const PASSWORD_FILE_FLAG: &str = "--password-file";

pub struct CliSecrets {
    pub password_file: Option<PathBuf>,
}

impl SecretProvider for CliSecrets {
    fn password(
        &self,
        config: &PostgresConnectionConfig,
    ) -> Result<(Password, SecretSource), SecretError> {
        let env_file = std::env::var_os(PASSWORD_FILE_ENV)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        let env_password = read_env(PASSWORD_ENV);

        let mut configured = Vec::new();
        if self.password_file.is_some() {
            configured.push(PASSWORD_FILE_FLAG.to_owned());
        }
        if env_file.is_some() {
            configured.push(PASSWORD_FILE_ENV.to_owned());
        }
        if env_password.is_some() {
            configured.push(PASSWORD_ENV.to_owned());
        }
        if configured.len() > 1 {
            return Err(SecretError::Ambiguous(configured));
        }

        if let Some(path) = self.password_file.clone().or(env_file) {
            let path = std::path::absolute(&path).unwrap_or(path);
            let password = read_password_file(&path)?;
            return Ok((password, SecretSource::File(path)));
        }
        if let Some(password) = env_password {
            return Ok((password, SecretSource::Env(PASSWORD_ENV)));
        }
        if !std::io::stdin().is_terminal() {
            return Err(SecretError::Missing(format!(
                "use {PASSWORD_FILE_FLAG}, {PASSWORD_FILE_ENV} ou {PASSWORD_ENV}, ou execute em um terminal interativo"
            )));
        }
        rpassword::prompt_password(format!("Senha para {config}: "))
            .map(|value| (Password::new(value), SecretSource::Prompt))
            .map_err(|_| SecretError::Prompt)
    }
}
