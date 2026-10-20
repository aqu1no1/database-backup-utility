use database_backup_core::{ConfigError, Password, PostgresConnectionConfig};
use thiserror::Error;
use tokio_postgres::Client;
use tokio_postgres::error::SqlState;

use super::connection::{ConnectionError, Timeouts, connect};

const MAINTENANCE_DATABASE: &str = "postgres";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseState {
    Missing,
    Empty,
    HasData { tables: i64 },
}

#[derive(Debug, Error)]
pub enum AdminError {
    #[error(transparent)]
    Connection(#[from] ConnectionError),
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error("sem permissão para {action}")]
    PermissionDenied {
        action: String,
        #[source]
        source: tokio_postgres::Error,
    },
    #[error("falha ao {action}: {detail}")]
    Query {
        action: String,
        detail: String,
        #[source]
        source: tokio_postgres::Error,
    },
}

pub struct Admin {
    server: PostgresConnectionConfig,
    password: Password,
    timeouts: Timeouts,
}

impl Admin {
    #[must_use]
    pub fn new(server: &PostgresConnectionConfig, password: &Password, timeouts: Timeouts) -> Self {
        Self {
            server: server.clone(),
            password: Password::new(password.expose().to_owned()),
            timeouts,
        }
    }

    pub async fn database_state(&self, name: &str) -> Result<DatabaseState, AdminError> {
        let client = self.client(MAINTENANCE_DATABASE).await?;
        let action = format!("verificar o banco {name}");
        let exists = client
            .query_opt("SELECT 1 FROM pg_database WHERE datname = $1", &[&name])
            .await
            .map_err(|error| query_error(&action, error))?
            .is_some();
        if !exists {
            return Ok(DatabaseState::Missing);
        }
        let tables = self.count_tables(name).await?;
        Ok(if tables == 0 {
            DatabaseState::Empty
        } else {
            DatabaseState::HasData { tables }
        })
    }

    pub async fn count_tables(&self, name: &str) -> Result<i64, AdminError> {
        let client = self.client(name).await?;
        client
            .query_one(
                "SELECT COUNT(*) FROM pg_catalog.pg_tables
                 WHERE schemaname NOT IN ('pg_catalog', 'information_schema')",
                &[],
            )
            .await
            .map(|row| row.get(0))
            .map_err(|error| query_error(&format!("contar as tabelas de {name}"), error))
    }

    pub async fn create_database(&self, name: &str) -> Result<(), AdminError> {
        self.execute(
            &format!("CREATE DATABASE {}", quote_ident(name)),
            &format!("criar o banco {name}"),
        )
        .await
    }

    pub async fn rename_database(&self, from: &str, to: &str) -> Result<(), AdminError> {
        self.execute(
            &format!(
                "ALTER DATABASE {} RENAME TO {}",
                quote_ident(from),
                quote_ident(to)
            ),
            &format!("renomear o banco {from} para {to}"),
        )
        .await
    }

    pub async fn drop_database(&self, name: &str) -> Result<(), AdminError> {
        self.execute(
            &format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", quote_ident(name)),
            &format!("apagar o banco {name}"),
        )
        .await
    }

    pub async fn terminate_connections(&self, name: &str) -> Result<(), AdminError> {
        let client = self.client(MAINTENANCE_DATABASE).await?;
        client
            .execute(
                "SELECT pg_terminate_backend(pid) FROM pg_stat_activity
                 WHERE datname = $1 AND pid <> pg_backend_pid()",
                &[&name],
            )
            .await
            .map(|_| ())
            .map_err(|error| query_error(&format!("encerrar as conexões de {name}"), error))
    }

    async fn execute(&self, sql: &str, action: &str) -> Result<(), AdminError> {
        let client = self.client(MAINTENANCE_DATABASE).await?;
        client
            .batch_execute(sql)
            .await
            .map_err(|error| query_error(action, error))
    }

    async fn client(&self, database: &str) -> Result<Client, AdminError> {
        let config = self.server.with_database(database)?;
        Ok(connect(&config, &self.password, self.timeouts).await?)
    }
}

const MAX_IDENTIFIER_BYTES: usize = 63;

#[must_use]
pub fn derived_name(base: &str, suffix: &str) -> String {
    let budget = MAX_IDENTIFIER_BYTES.saturating_sub(suffix.len());
    let mut end = base.len().min(budget);
    while !base.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{suffix}", &base[..end])
}

#[must_use]
pub fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn query_error(action: &str, error: tokio_postgres::Error) -> AdminError {
    if error.code() == Some(&SqlState::INSUFFICIENT_PRIVILEGE) {
        return AdminError::PermissionDenied {
            action: action.to_owned(),
            source: error,
        };
    }
    let detail = error
        .as_db_error()
        .map_or_else(|| error.to_string(), |db| db.message().to_owned());
    AdminError::Query {
        action: action.to_owned(),
        detail,
        source: error,
    }
}

#[cfg(test)]
mod tests {
    use super::{derived_name, quote_ident};

    #[test]
    fn derived_name_fits_postgres_limit() {
        assert_eq!(
            derived_name("my_db", "__restore_20261020T190509Z"),
            "my_db__restore_20261020T190509Z"
        );
        let long = "a".repeat(63);
        let name = derived_name(&long, "__previous_20261020T190509Z");
        assert_eq!(name.len(), 63);
        assert!(name.ends_with("__previous_20261020T190509Z"));
        let accented = "ç".repeat(30);
        assert!(derived_name(&accented, "__restore_20261020T190509Z").len() <= 63);
    }

    #[test]
    fn quotes_and_escapes_identifiers() {
        assert_eq!(quote_ident("my_db"), "\"my_db\"");
        assert_eq!(quote_ident("we\"ird"), "\"we\"\"ird\"");
    }
}
