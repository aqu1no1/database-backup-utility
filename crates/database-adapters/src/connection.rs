use std::error::Error as StdError;
use std::io;
use std::time::Duration;

use thiserror::Error;

pub(crate) type BoxError = Box<dyn StdError + Send + Sync>;

#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    pub connect: Duration,
    pub query: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(10),
            query: Duration::from_secs(10),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionInfo {
    pub server_version: String,
}

#[derive(Debug, Error)]
pub enum ConnectionError {
    #[error("autenticação recusada para o usuário {user}")]
    AuthenticationFailed {
        user: String,
        #[source]
        source: BoxError,
    },
    #[error("o banco {database} não existe")]
    DatabaseNotFound {
        database: String,
        #[source]
        source: BoxError,
    },
    #[error("servidor inacessível em {address}")]
    Unreachable {
        address: String,
        #[source]
        source: BoxError,
    },
    #[error("tempo esgotado ao acessar {address}")]
    Timeout { address: String },
    #[error("falha na negociação TLS com {address}")]
    Tls {
        address: String,
        #[source]
        source: BoxError,
    },
    #[error("falha ao testar a conexão com {address}")]
    Other {
        address: String,
        #[source]
        source: BoxError,
    },
}

pub(crate) fn has_io_kind(error: &(dyn StdError + 'static), kind: io::ErrorKind) -> bool {
    let mut current = Some(error);
    while let Some(err) = current {
        if err
            .downcast_ref::<io::Error>()
            .is_some_and(|io| io.kind() == kind)
        {
            return true;
        }
        current = err.source();
    }
    false
}
