use std::collections::BTreeSet;

use database_backup_core::{ConfigError, MysqlConnectionConfig, Password};
use mysql_async::Conn;
use mysql_async::prelude::Queryable;
use thiserror::Error;

use super::connection::connect;
use super::dump::{Definer, ObjectKind};
use crate::connection::{ConnectionError, Timeouts};

const CATALOG_DATABASE: &str = "information_schema";
const PERMISSION_ERRORS: [u16; 4] = [1044, 1142, 1227, 1370];
const PROBE: &str = "__dbu_definer_probe";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseState {
    Missing,
    Exists { tables: u64 },
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
        source: mysql_async::Error,
    },
    #[error(
        "o usuário de restore não consegue criar {kind} com DEFINER {definer} no destino: {detail}; use um usuário com SET_ANY_DEFINER (ou SUPER, que também cobre o binlog), ou restaure com o próprio {definer}"
    )]
    DefinerRefused {
        kind: ObjectKind,
        definer: Definer,
        detail: String,
        #[source]
        source: Box<mysql_async::Error>,
    },
    #[error("falha ao {action}: {detail}")]
    Query {
        action: String,
        detail: String,
        #[source]
        source: mysql_async::Error,
    },
}

pub struct Admin {
    server: MysqlConnectionConfig,
    password: Password,
    timeouts: Timeouts,
}

impl Admin {
    #[must_use]
    pub fn new(server: &MysqlConnectionConfig, password: &Password, timeouts: Timeouts) -> Self {
        Self {
            server: server.clone(),
            password: Password::new(password.expose().to_owned()),
            timeouts,
        }
    }

    pub async fn database_state(&self, name: &str) -> Result<DatabaseState, AdminError> {
        let mut conn = self.client().await?;
        let action = format!("verificar o banco {name}");
        let exists = conn
            .exec_first::<u8, _, _>(
                "SELECT 1 FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = ?",
                (name,),
            )
            .await
            .map_err(|error| query_error(&action, error))?
            .is_some();
        if !exists {
            let _ = conn.disconnect().await;
            return Ok(DatabaseState::Missing);
        }
        let tables = count_tables(&mut conn, name).await;
        let _ = conn.disconnect().await;
        Ok(DatabaseState::Exists { tables: tables? })
    }

    pub async fn count_tables(&self, name: &str) -> Result<u64, AdminError> {
        let mut conn = self.client().await?;
        let tables = count_tables(&mut conn, name).await;
        let _ = conn.disconnect().await;
        tables
    }

    pub async fn non_transactional_tables(&self, name: &str) -> Result<Vec<String>, AdminError> {
        let mut conn = self.client().await?;
        let tables = conn
            .exec::<String, _, _>(
                "SELECT TABLE_NAME FROM information_schema.TABLES
                 WHERE TABLE_SCHEMA = ? AND TABLE_TYPE = 'BASE TABLE'
                   AND (ENGINE IS NULL OR ENGINE <> 'InnoDB')
                 ORDER BY TABLE_NAME",
                (name,),
            )
            .await
            .map_err(|error| {
                query_error(&format!("listar os motores das tabelas de {name}"), error)
            });
        let _ = conn.disconnect().await;
        tables
    }

    pub async fn create_database(&self, name: &str) -> Result<(), AdminError> {
        self.execute(
            &format!("CREATE DATABASE {}", quote_ident(name)),
            &format!("criar o banco {name}"),
        )
        .await
    }

    pub async fn drop_database(&self, name: &str) -> Result<(), AdminError> {
        self.execute(
            &format!("DROP DATABASE IF EXISTS {}", quote_ident(name)),
            &format!("apagar o banco {name}"),
        )
        .await
    }

    pub async fn probe_definers(
        &self,
        database: &str,
        definers: &BTreeSet<(ObjectKind, Definer)>,
    ) -> Result<(), AdminError> {
        if definers.is_empty() {
            return Ok(());
        }
        let mut conn = self.client().await?;
        let mut result = Ok(());
        for (kind, definer) in definers {
            for sql in probe_statements(database, *kind, definer) {
                if let Err(error) = conn.query_drop(&sql).await {
                    result = Err(definer_refused(*kind, definer, error));
                    break;
                }
            }
            if result.is_err() {
                break;
            }
        }
        let _ = conn.disconnect().await;
        result
    }

    async fn execute(&self, sql: &str, action: &str) -> Result<(), AdminError> {
        let mut conn = self.client().await?;
        let result = conn
            .query_drop(sql)
            .await
            .map_err(|error| query_error(action, error));
        let _ = conn.disconnect().await;
        result
    }

    async fn client(&self) -> Result<Conn, AdminError> {
        let config = self.server.with_database(CATALOG_DATABASE)?;
        Ok(connect(&config, &self.password, self.timeouts).await?)
    }
}

async fn count_tables(conn: &mut Conn, name: &str) -> Result<u64, AdminError> {
    conn.exec_first::<u64, _, _>(
        "SELECT COUNT(*) FROM information_schema.TABLES
         WHERE TABLE_SCHEMA = ? AND TABLE_TYPE = 'BASE TABLE'",
        (name,),
    )
    .await
    .map(Option::unwrap_or_default)
    .map_err(|error| query_error(&format!("contar as tabelas de {name}"), error))
}

fn probe_statements(database: &str, kind: ObjectKind, definer: &Definer) -> Vec<String> {
    let db = quote_ident(database);
    let name = format!("{db}.{}", quote_ident(PROBE));
    let definer = format!(
        "{}@{}",
        quote_ident(&definer.user),
        quote_ident(&definer.host)
    );
    match kind {
        ObjectKind::Trigger => vec![
            format!("CREATE TABLE {name} (i INT)"),
            format!(
                "CREATE DEFINER={definer} TRIGGER {db}.{} BEFORE INSERT ON {name} FOR EACH ROW SET NEW.i = NEW.i",
                quote_ident(&format!("{PROBE}_trigger"))
            ),
            format!("DROP TABLE {name}"),
        ],
        ObjectKind::View => vec![
            format!("CREATE DEFINER={definer} SQL SECURITY DEFINER VIEW {name} AS SELECT 1 AS i"),
            format!("DROP VIEW {name}"),
        ],
        ObjectKind::Procedure => vec![
            format!("CREATE DEFINER={definer} PROCEDURE {name}() SELECT 1"),
            format!("DROP PROCEDURE {name}"),
        ],
        ObjectKind::Function => vec![
            format!(
                "CREATE DEFINER={definer} FUNCTION {name}() RETURNS INT DETERMINISTIC RETURN 1"
            ),
            format!("DROP FUNCTION {name}"),
        ],
        ObjectKind::Event => vec![
            format!(
                "CREATE DEFINER={definer} EVENT {name} ON SCHEDULE AT CURRENT_TIMESTAMP + INTERVAL 1 DAY DISABLE DO SELECT 1"
            ),
            format!("DROP EVENT {name}"),
        ],
    }
}

fn definer_refused(kind: ObjectKind, definer: &Definer, error: mysql_async::Error) -> AdminError {
    let detail = match &error {
        mysql_async::Error::Server(server) => server.message.clone(),
        other => other.to_string(),
    };
    AdminError::DefinerRefused {
        kind,
        definer: definer.clone(),
        detail,
        source: Box::new(error),
    }
}

#[must_use]
pub fn quote_ident(name: &str) -> String {
    format!("`{}`", name.replace('`', "``"))
}

fn query_error(action: &str, error: mysql_async::Error) -> AdminError {
    if let mysql_async::Error::Server(server) = &error
        && PERMISSION_ERRORS.contains(&server.code)
    {
        return AdminError::PermissionDenied {
            action: action.to_owned(),
            source: error,
        };
    }
    let detail = match &error {
        mysql_async::Error::Server(server) => server.message.clone(),
        other => other.to_string(),
    };
    AdminError::Query {
        action: action.to_owned(),
        detail,
        source: error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_quotes_definer_and_names_from_the_dump() {
        let definer = Definer {
            user: "x`; DROP DATABASE prod; --".to_owned(),
            host: "%".to_owned(),
        };
        let statements = probe_statements("dest", ObjectKind::View, &definer);

        assert_eq!(
            statements[0],
            "CREATE DEFINER=`x``; DROP DATABASE prod; --`@`%` SQL SECURITY DEFINER VIEW `dest`.`__dbu_definer_probe` AS SELECT 1 AS i"
        );
        assert_eq!(statements[1], "DROP VIEW `dest`.`__dbu_definer_probe`");
    }

    #[test]
    fn quotes_and_escapes_identifiers() {
        assert_eq!(quote_ident("my_db"), "`my_db`");
        assert_eq!(quote_ident("we`ird"), "`we``ird`");
    }
}
