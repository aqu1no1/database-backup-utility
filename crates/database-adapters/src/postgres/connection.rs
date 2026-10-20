use std::error::Error as StdError;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use database_backup_core::{Password, PostgresConnectionConfig, TlsRequirement};
use rustls::{ClientConfig, RootCertStore};
use thiserror::Error;
use tokio_postgres::config::SslMode;
use tokio_postgres::error::SqlState;
use tokio_postgres::tls::{MakeTlsConnect, TlsConnect};
use tokio_postgres::{Client, Config, NoTls, Socket};
use tokio_postgres_rustls::MakeRustlsConnect;

type BoxError = Box<dyn StdError + Send + Sync>;

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

pub async fn test_connection(
    config: &PostgresConnectionConfig,
    password: &Password,
    timeouts: Timeouts,
) -> Result<ConnectionInfo, ConnectionError> {
    let mut pg_config = Config::new();
    pg_config
        .host(config.host())
        .port(config.port())
        .dbname(config.database())
        .user(config.user())
        .password(password.expose())
        .connect_timeout(timeouts.connect);

    match config.tls() {
        TlsRequirement::Plain => {
            pg_config.ssl_mode(SslMode::Disable);
            run(config, &pg_config, NoTls, timeouts).await
        }
        TlsRequirement::VerifyFull => {
            pg_config.ssl_mode(SslMode::Require);
            let tls = verified_tls().map_err(|source| ConnectionError::Tls {
                address: address(config),
                source,
            })?;
            run(config, &pg_config, tls, timeouts).await
        }
    }
}

async fn run<T>(
    config: &PostgresConnectionConfig,
    pg_config: &Config,
    tls: T,
    timeouts: Timeouts,
) -> Result<ConnectionInfo, ConnectionError>
where
    T: MakeTlsConnect<Socket> + Send + 'static,
    T::Stream: Send + 'static,
    <T::TlsConnect as TlsConnect<Socket>>::Future: Send,
{
    let total = timeouts.connect + timeouts.query;
    match tokio::time::timeout(total, connect_and_query(pg_config, tls)).await {
        Ok(result) => result.map_err(|error| classify(config, error)),
        Err(_) => Err(ConnectionError::Timeout {
            address: address(config),
        }),
    }
}

async fn connect_and_query<T>(
    pg_config: &Config,
    tls: T,
) -> Result<ConnectionInfo, tokio_postgres::Error>
where
    T: MakeTlsConnect<Socket> + Send + 'static,
    T::Stream: Send + 'static,
    <T::TlsConnect as TlsConnect<Socket>>::Future: Send,
{
    let (client, connection) = pg_config.connect(tls).await?;
    let connection_task = tokio::spawn(async move {
        let _ = connection.await;
    });
    let result = query_server(&client).await;
    drop(client);
    connection_task.abort();
    result
}

async fn query_server(client: &Client) -> Result<ConnectionInfo, tokio_postgres::Error> {
    client.query_one("SELECT 1", &[]).await?;
    let server_version: String = client.query_one("SHOW server_version", &[]).await?.get(0);
    Ok(ConnectionInfo { server_version })
}

fn verified_tls() -> Result<MakeRustlsConnect, BoxError> {
    let native = rustls_native_certs::load_native_certs();
    let mut roots = RootCertStore::empty();
    roots.add_parsable_certificates(native.certs);
    if roots.is_empty() {
        return Err("nenhum certificado raiz encontrado no sistema".into());
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let client_config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(MakeRustlsConnect::new(client_config))
}

fn classify(config: &PostgresConnectionConfig, error: tokio_postgres::Error) -> ConnectionError {
    let address = address(config);
    let code = error.code().cloned();
    let message = error.to_string();
    let timed_out = has_io_kind(&error, io::ErrorKind::TimedOut);
    let source: BoxError = Box::new(error);

    match code {
        Some(code)
            if code == SqlState::INVALID_PASSWORD
                || code == SqlState::INVALID_AUTHORIZATION_SPECIFICATION =>
        {
            return ConnectionError::AuthenticationFailed {
                user: config.user().to_owned(),
                source,
            };
        }
        Some(code) if code == SqlState::INVALID_CATALOG_NAME => {
            return ConnectionError::DatabaseNotFound {
                database: config.database().to_owned(),
                source,
            };
        }
        Some(_) => return ConnectionError::Other { address, source },
        None => {}
    }

    if timed_out || message.starts_with("timeout waiting for server") {
        ConnectionError::Timeout { address }
    } else if message.starts_with("error performing TLS handshake") {
        ConnectionError::Tls { address, source }
    } else if message.starts_with("error connecting to server")
        || message.starts_with("error communicating with the server")
    {
        ConnectionError::Unreachable { address, source }
    } else {
        ConnectionError::Other { address, source }
    }
}

fn has_io_kind(error: &(dyn StdError + 'static), kind: io::ErrorKind) -> bool {
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

fn address(config: &PostgresConnectionConfig) -> String {
    format!("{}:{}", config.host(), config.port())
}
