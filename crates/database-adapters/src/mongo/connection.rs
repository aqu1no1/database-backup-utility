use std::fmt;
use std::io;
use std::time::Duration;

use database_backup_core::{MongoConnectionConfig, Password, TlsRequirement};
use mongodb::Client;
use mongodb::bson::{Document, doc};
use mongodb::error::{Error as DriverError, ErrorKind};
use mongodb::options::{ClientOptions, Credential, ServerAddress, Tls, TlsOptions};
use tokio::net::TcpStream;

use crate::connection::{ConnectionError, Timeouts, has_io_kind};
use crate::process::ca_bundle;

const AUTHENTICATION_FAILED: i32 = 18;
const UNAUTHORIZED: i32 = 13;
const APP_NAME: &str = "database-backup";
const ADMIN_DATABASE: &str = "admin";
const SHUTDOWN_LIMIT: Duration = Duration::from_secs(2);
const DRIVER_TOO_OLD_MARKER: &str = "driver requires at least";
const DRIVER_MIN_SERVER: &str = "4.4";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MongoVersion {
    pub major: u32,
    pub minor: u32,
}

impl MongoVersion {
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = text.trim().split(|c: char| !c.is_ascii_digit());
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        Some(Self { major, minor })
    }
}

impl fmt::Display for MongoVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Topology {
    Standalone,
    ReplicaSet { name: String, primary: bool },
    Sharded,
}

impl Topology {
    #[must_use]
    pub fn from_hello(hello: &Document) -> Self {
        if hello.get_str("msg").is_ok_and(|msg| msg == "isdbgrid") {
            return Self::Sharded;
        }
        match hello.get_str("setName") {
            Ok(name) => Self::ReplicaSet {
                name: name.to_owned(),
                primary: hello
                    .get_bool("isWritablePrimary")
                    .or_else(|_| hello.get_bool("ismaster"))
                    .unwrap_or(false),
            },
            Err(_) => Self::Standalone,
        }
    }

    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Standalone => "standalone",
            Self::ReplicaSet { .. } => "replica_set",
            Self::Sharded => "sharded",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MongoServer {
    pub server_version: String,
    pub version: MongoVersion,
    pub topology: Topology,
}

pub async fn test_connection(
    config: &MongoConnectionConfig,
    password: &Password,
    timeouts: Timeouts,
) -> Result<MongoServer, ConnectionError> {
    inspect(config, password, timeouts, true).await
}

pub async fn probe_server(
    config: &MongoConnectionConfig,
    password: &Password,
    timeouts: Timeouts,
) -> Result<MongoServer, ConnectionError> {
    inspect(config, password, timeouts, false).await
}

async fn inspect(
    config: &MongoConnectionConfig,
    password: &Password,
    timeouts: Timeouts,
    require_database: bool,
) -> Result<MongoServer, ConnectionError> {
    probe_tcp(config, timeouts).await?;
    let client = connect(config, password, timeouts)?;
    let task = tokio::spawn({
        let config = config.clone();
        let client = client.clone();
        async move {
            let server = describe_server(&config, &client).await?;
            if require_database {
                ensure_database(&config, &client).await?;
            }
            Ok::<_, ConnectionError>(server)
        }
    });
    let outcome = tokio::time::timeout(timeouts.connect + timeouts.query, task).await;
    close(client).await;
    match outcome {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => Err(ConnectionError::Other {
            address: address(config),
            source: Box::new(error),
        }),
        Err(_) => Err(ConnectionError::Timeout {
            address: address(config),
        }),
    }
}

pub(crate) fn connect(
    config: &MongoConnectionConfig,
    password: &Password,
    timeouts: Timeouts,
) -> Result<Client, ConnectionError> {
    let tls = match config.tls() {
        TlsRequirement::Plain => Tls::Disabled,
        TlsRequirement::VerifyFull => {
            let ca = ca_bundle().map_err(|source| ConnectionError::Tls {
                address: address(config),
                source: Box::new(source),
            })?;
            Tls::Enabled(TlsOptions::builder().ca_file_path(ca).build())
        }
    };
    let credential = Credential::builder()
        .username(config.user().to_owned())
        .password(password.expose().to_owned())
        .source(config.auth_database().to_owned())
        .build();
    let options = ClientOptions::builder()
        .hosts(vec![ServerAddress::Tcp {
            host: config.host().to_owned(),
            port: Some(config.port()),
        }])
        .direct_connection(true)
        .credential(credential)
        .tls(tls)
        .connect_timeout(timeouts.connect)
        .server_selection_timeout(timeouts.connect)
        .app_name(APP_NAME.to_owned())
        .build();
    Client::with_options(options).map_err(|error| classify(config, error))
}

pub(crate) async fn close(client: Client) {
    let _ = tokio::time::timeout(SHUTDOWN_LIMIT, client.shutdown()).await;
}

async fn probe_tcp(
    config: &MongoConnectionConfig,
    timeouts: Timeouts,
) -> Result<(), ConnectionError> {
    let target = (config.host(), config.port());
    match tokio::time::timeout(timeouts.connect, TcpStream::connect(target)).await {
        Ok(Ok(stream)) => {
            drop(stream);
            Ok(())
        }
        Ok(Err(source)) if has_io_kind(&source, io::ErrorKind::TimedOut) => {
            Err(ConnectionError::Timeout {
                address: address(config),
            })
        }
        Ok(Err(source)) => Err(ConnectionError::Unreachable {
            address: address(config),
            source: Box::new(source),
        }),
        Err(_) => Err(ConnectionError::Timeout {
            address: address(config),
        }),
    }
}

async fn describe_server(
    config: &MongoConnectionConfig,
    client: &Client,
) -> Result<MongoServer, ConnectionError> {
    let admin = client.database(ADMIN_DATABASE);
    let hello = admin
        .run_command(doc! { "hello": 1 })
        .await
        .map_err(|error| classify(config, error))?;
    let build = admin
        .run_command(doc! { "buildInfo": 1 })
        .await
        .map_err(|error| classify(config, error))?;
    let server_version = build
        .get_str("version")
        .map_err(|_| other(config, "o servidor não informou a versão"))?
        .to_owned();
    let version = MongoVersion::parse(&server_version)
        .ok_or_else(|| other(config, "a versão do servidor não foi reconhecida"))?;
    Ok(MongoServer {
        server_version,
        version,
        topology: Topology::from_hello(&hello),
    })
}

async fn ensure_database(
    config: &MongoConnectionConfig,
    client: &Client,
) -> Result<(), ConnectionError> {
    let names = client
        .database(config.database())
        .list_collection_names()
        .authorized_collections(true)
        .await
        .map_err(|error| classify(config, error))?;
    if names.is_empty() {
        return Err(ConnectionError::DatabaseNotFound {
            database: config.database().to_owned(),
            source: "nenhuma coleção visível para o usuário no banco".into(),
        });
    }
    Ok(())
}

pub(crate) fn classify(config: &MongoConnectionConfig, error: DriverError) -> ConnectionError {
    let address = address(config);
    let authentication = match &*error.kind {
        ErrorKind::Authentication { .. } => true,
        ErrorKind::Command(command) => {
            command.code == AUTHENTICATION_FAILED || command.code == UNAUTHORIZED
        }
        _ => false,
    };
    if authentication {
        return ConnectionError::AuthenticationFailed {
            user: config.user().to_owned(),
            source: Box::new(error),
        };
    }
    match &*error.kind {
        ErrorKind::InvalidTlsConfig { .. } => ConnectionError::Tls {
            address,
            source: Box::new(error),
        },
        ErrorKind::ServerSelection { message, .. } if message.contains(DRIVER_TOO_OLD_MARKER) => {
            ConnectionError::Other {
                address,
                source: format!(
                    "o servidor é mais antigo que o MongoDB {DRIVER_MIN_SERVER}, a versão mínima suportada"
                )
                .into(),
            }
        }
        ErrorKind::ServerSelection { .. } => match config.tls() {
            TlsRequirement::VerifyFull => ConnectionError::Tls {
                address,
                source: Box::new(error),
            },
            TlsRequirement::Plain => ConnectionError::Timeout { address },
        },
        ErrorKind::Io(_) | ErrorKind::DnsResolve { .. } => ConnectionError::Unreachable {
            address,
            source: Box::new(error),
        },
        _ => ConnectionError::Other {
            address,
            source: Box::new(error),
        },
    }
}

pub(crate) fn address(config: &MongoConnectionConfig) -> String {
    format!("{}:{}", config.host(), config.port())
}

fn other(config: &MongoConnectionConfig, message: &'static str) -> ConnectionError {
    ConnectionError::Other {
        address: address(config),
        source: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_server_versions() {
        let v80 = Some(MongoVersion { major: 8, minor: 0 });
        assert_eq!(MongoVersion::parse("8.0.15"), v80);
        assert_eq!(MongoVersion::parse("8.0.0-rc4"), v80);
        assert_eq!(MongoVersion::parse(" 8.0 "), v80);
        assert_eq!(
            MongoVersion::parse("4.4.30"),
            Some(MongoVersion { major: 4, minor: 4 })
        );
        assert_eq!(MongoVersion::parse("r4.2.24"), None);
        assert_eq!(MongoVersion::parse("8"), None);
        assert_eq!(MongoVersion::parse("sem versão"), None);
        assert!(
            MongoVersion { major: 4, minor: 4 }
                < MongoVersion {
                    major: 10,
                    minor: 0
                }
        );
        assert_eq!(MongoVersion { major: 7, minor: 0 }.to_string(), "7.0");
    }

    #[test]
    fn topology_comes_from_the_hello_reply() {
        let standalone = doc! { "isWritablePrimary": true, "maxWireVersion": 21, "ok": 1 };
        let primary = doc! {
            "setName": "rs0", "isWritablePrimary": true, "secondary": false,
            "hosts": ["a:27017"], "ok": 1,
        };
        let secondary = doc! {
            "setName": "rs0", "isWritablePrimary": false, "secondary": true, "ok": 1,
        };
        let legacy_primary = doc! { "setName": "rs0", "ismaster": true, "ok": 1 };
        let arbiter = doc! {
            "setName": "rs0", "isWritablePrimary": false, "secondary": false,
            "arbiterOnly": true, "ok": 1,
        };
        let mongos = doc! { "isWritablePrimary": true, "msg": "isdbgrid", "ok": 1 };
        let replica = |primary| Topology::ReplicaSet {
            name: "rs0".to_owned(),
            primary,
        };

        assert_eq!(Topology::from_hello(&standalone), Topology::Standalone);
        assert_eq!(Topology::from_hello(&primary), replica(true));
        assert_eq!(Topology::from_hello(&secondary), replica(false));
        assert_eq!(Topology::from_hello(&legacy_primary), replica(true));
        assert_eq!(Topology::from_hello(&arbiter), replica(false));
        assert_eq!(Topology::from_hello(&mongos), Topology::Sharded);
        assert_eq!(Topology::Standalone.as_str(), "standalone");
        assert_eq!(replica(true).as_str(), "replica_set");
        assert_eq!(Topology::Sharded.as_str(), "sharded");
    }
}
