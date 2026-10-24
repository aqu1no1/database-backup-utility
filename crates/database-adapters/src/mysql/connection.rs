use std::io;
use std::time::Duration;

use database_backup_core::{MysqlConnectionConfig, Password, TlsRequirement};
use mysql_async::prelude::Queryable;
use mysql_async::{Conn, IoError, OptsBuilder, SslOpts};

use crate::connection::{BoxError, ConnectionError, ConnectionInfo, Timeouts, has_io_kind};

const ACCESS_DENIED: u16 = 1045;
const DATABASE_ACCESS_DENIED: u16 = 1044;
const UNKNOWN_DATABASE: u16 = 1049;

pub async fn test_connection(
    config: &MysqlConnectionConfig,
    password: &Password,
    timeouts: Timeouts,
) -> Result<ConnectionInfo, ConnectionError> {
    let mut conn = connect(config, password, timeouts).await?;
    let queried = tokio::time::timeout(timeouts.query, async {
        conn.query_drop("SELECT 1").await?;
        conn.query_first::<String, _>("SELECT VERSION()").await
    })
    .await;
    let _ = conn.disconnect().await;
    match queried {
        Ok(Ok(Some(server_version))) => Ok(ConnectionInfo { server_version }),
        Ok(Ok(None)) => Err(ConnectionError::Other {
            address: address(config),
            source: "o servidor não informou a versão".into(),
        }),
        Ok(Err(error)) => Err(classify(config, error)),
        Err(_) => Err(ConnectionError::Timeout {
            address: address(config),
        }),
    }
}

pub(crate) async fn connect(
    config: &MysqlConnectionConfig,
    password: &Password,
    timeouts: Timeouts,
) -> Result<Conn, ConnectionError> {
    let ssl = match config.tls() {
        TlsRequirement::Plain => None,
        TlsRequirement::VerifyFull => {
            Some(verified_tls().map_err(|source| ConnectionError::Tls {
                address: address(config),
                source,
            })?)
        }
    };
    let opts = OptsBuilder::default()
        .ip_or_hostname(config.host())
        .tcp_port(config.port())
        .user(Some(config.user()))
        .pass(Some(password.expose()))
        .db_name(Some(config.database()))
        .prefer_socket(false)
        .ssl_opts(ssl);

    let limit: Duration = timeouts.connect + timeouts.query;
    match tokio::time::timeout(limit, Conn::new(opts)).await {
        Ok(Ok(conn)) => Ok(conn),
        Ok(Err(error)) => Err(classify(config, error)),
        Err(_) => Err(ConnectionError::Timeout {
            address: address(config),
        }),
    }
}

fn verified_tls() -> Result<SslOpts, BoxError> {
    let native = rustls_native_certs::load_native_certs();
    if native.certs.is_empty() {
        return Err("nenhum certificado raiz encontrado no sistema".into());
    }
    let roots = native
        .certs
        .into_iter()
        .map(|cert| cert.as_ref().to_vec().into())
        .collect();
    Ok(SslOpts::default()
        .with_root_certs(roots)
        .with_disable_built_in_roots(true))
}

pub(crate) fn classify(
    config: &MysqlConnectionConfig,
    error: mysql_async::Error,
) -> ConnectionError {
    let address = address(config);
    match error {
        mysql_async::Error::Server(server)
            if server.code == ACCESS_DENIED || server.code == DATABASE_ACCESS_DENIED =>
        {
            ConnectionError::AuthenticationFailed {
                user: config.user().to_owned(),
                source: Box::new(server),
            }
        }
        mysql_async::Error::Server(server) if server.code == UNKNOWN_DATABASE => {
            ConnectionError::DatabaseNotFound {
                database: config.database().to_owned(),
                source: Box::new(server),
            }
        }
        mysql_async::Error::Io(IoError::Tls(tls)) => ConnectionError::Tls {
            address,
            source: Box::new(tls),
        },
        mysql_async::Error::Driver(
            driver @ mysql_async::DriverError::NoClientSslFlagFromServer,
        ) => ConnectionError::Tls {
            address,
            source: Box::new(driver),
        },
        mysql_async::Error::Io(IoError::Io(io)) => {
            if io
                .get_ref()
                .is_some_and(|inner| inner.is::<rustls::Error>())
            {
                ConnectionError::Tls {
                    address,
                    source: Box::new(io),
                }
            } else if has_io_kind(&io, io::ErrorKind::TimedOut) {
                ConnectionError::Timeout { address }
            } else {
                ConnectionError::Unreachable {
                    address,
                    source: Box::new(io),
                }
            }
        }
        other => ConnectionError::Other {
            address,
            source: Box::new(other),
        },
    }
}

pub(crate) fn address(config: &MysqlConnectionConfig) -> String {
    format!("{}:{}", config.host(), config.port())
}
