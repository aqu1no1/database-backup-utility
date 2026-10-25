use std::fmt;
use std::io::IsTerminal;
use std::path::PathBuf;

use database_backup_core::{
    DatabaseEngine, Password, SecretError, SecretProvider, SecretSource, read_env,
    read_password_file,
};

pub const PASSWORD_FILE_FLAG: &str = "--password-file";

pub struct CliSecrets {
    pub password_file: Option<PathBuf>,
    pub engine: DatabaseEngine,
}

impl CliSecrets {
    fn env_names(&self) -> (&'static str, &'static str) {
        match self.engine {
            DatabaseEngine::Mysql => ("DBU_MYSQL_PASSWORD", "DBU_MYSQL_PASSWORD_FILE"),
            DatabaseEngine::Mongodb => ("DBU_MONGO_PASSWORD", "DBU_MONGO_PASSWORD_FILE"),
            DatabaseEngine::Postgres | DatabaseEngine::Sqlite => {
                ("DBU_PG_PASSWORD", "DBU_PG_PASSWORD_FILE")
            }
        }
    }
}

impl SecretProvider for CliSecrets {
    fn password(&self, target: &dyn fmt::Display) -> Result<(Password, SecretSource), SecretError> {
        let (password_env, password_file_env) = self.env_names();
        let env_file = std::env::var_os(password_file_env)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        let env_password = read_env(password_env);

        let mut configured = Vec::new();
        if self.password_file.is_some() {
            configured.push(PASSWORD_FILE_FLAG.to_owned());
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
