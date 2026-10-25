use std::fmt;
use std::net::IpAddr;

use thiserror::Error;

const MAX_IDENTIFIER_BYTES: usize = 63;
const MYSQL_MAX_DATABASE_CHARS: usize = 64;
const MYSQL_MAX_USER_CHARS: usize = 32;
const MONGO_FORBIDDEN_NAME_CHARS: [char; 13] = [
    '/', '\\', '.', ' ', '"', '$', '*', '<', '>', ':', '|', '?', '\0',
];
const MONGO_RESERVED_DATABASES: [&str; 3] = ["admin", "local", "config"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TlsMode {
    #[default]
    Auto,
    Disable,
    VerifyFull,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsRequirement {
    Plain,
    VerifyFull,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("o host não pode ser vazio")]
    EmptyHost,
    #[error("o host contém espaços ou caracteres inválidos")]
    InvalidHost,
    #[error("a porta deve estar entre 1 e 65535")]
    InvalidPort,
    #[error("o {field} não pode ser vazio")]
    EmptyField { field: &'static str },
    #[error("o {field} excede {max} {unit}")]
    FieldTooLong {
        field: &'static str,
        max: usize,
        unit: &'static str,
    },
    #[error("o {field} contém caracteres não permitidos pelo MySQL (/, \\, . ou espaço no fim)")]
    InvalidName { field: &'static str },
    #[error(
        "o {field} contém caracteres não permitidos pelo MongoDB (/ \\ . espaço \" $ * < > : | ? ou nulo)"
    )]
    InvalidMongoName { field: &'static str },
    #[error("o banco {name} é interno do MongoDB e não pode ser origem nem destino")]
    ReservedDatabase { name: String },
    #[error("informe um único host, sem lista de hosts nem nome de replica set")]
    MultipleHosts,
    #[error("conexão sem TLS só é permitida para localhost; use --tls verify-full para {host}")]
    PlainTlsForRemoteHost { host: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostgresConnectionConfig {
    host: String,
    port: u16,
    database: String,
    user: String,
    tls: TlsRequirement,
}

impl PostgresConnectionConfig {
    pub fn new(
        host: &str,
        port: u16,
        database: &str,
        user: &str,
        tls: TlsMode,
    ) -> Result<Self, ConfigError> {
        let (host, tls) = validate_server(host, port, tls)?;
        validate_identifier("database", database)?;
        validate_identifier("usuário", user)?;
        Ok(Self {
            host,
            port,
            database: database.to_owned(),
            user: user.to_owned(),
            tls,
        })
    }

    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    #[must_use]
    pub fn database(&self) -> &str {
        &self.database
    }

    #[must_use]
    pub fn user(&self) -> &str {
        &self.user
    }

    #[must_use]
    pub fn tls(&self) -> TlsRequirement {
        self.tls
    }

    #[must_use]
    pub fn is_local(&self) -> bool {
        is_local_host(&self.host)
    }

    pub fn with_database(&self, database: &str) -> Result<Self, ConfigError> {
        validate_identifier("database", database)?;
        Ok(Self {
            database: database.to_owned(),
            ..self.clone()
        })
    }
}

impl fmt::Display for PostgresConnectionConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}@{}:{}/{}",
            self.user, self.host, self.port, self.database
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MysqlConnectionConfig {
    host: String,
    port: u16,
    database: String,
    user: String,
    tls: TlsRequirement,
}

impl MysqlConnectionConfig {
    pub fn new(
        host: &str,
        port: u16,
        database: &str,
        user: &str,
        tls: TlsMode,
    ) -> Result<Self, ConfigError> {
        let (host, tls) = validate_server(host, port, tls)?;
        validate_mysql_database(database)?;
        validate_chars("usuário", user, MYSQL_MAX_USER_CHARS)?;
        Ok(Self {
            host,
            port,
            database: database.to_owned(),
            user: user.to_owned(),
            tls,
        })
    }

    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    #[must_use]
    pub fn database(&self) -> &str {
        &self.database
    }

    #[must_use]
    pub fn user(&self) -> &str {
        &self.user
    }

    #[must_use]
    pub fn tls(&self) -> TlsRequirement {
        self.tls
    }

    #[must_use]
    pub fn is_local(&self) -> bool {
        is_local_host(&self.host)
    }

    pub fn with_database(&self, database: &str) -> Result<Self, ConfigError> {
        validate_mysql_database(database)?;
        Ok(Self {
            database: database.to_owned(),
            ..self.clone()
        })
    }
}

impl fmt::Display for MysqlConnectionConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}@{}:{}/{}",
            self.user, self.host, self.port, self.database
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MongoConnectionConfig {
    host: String,
    port: u16,
    database: String,
    user: String,
    auth_database: String,
    tls: TlsRequirement,
}

impl MongoConnectionConfig {
    pub fn new(
        host: &str,
        port: u16,
        database: &str,
        user: &str,
        auth_database: &str,
        tls: TlsMode,
    ) -> Result<Self, ConfigError> {
        let (host, tls) = validate_server(host, port, tls)?;
        if host.contains([',', '/']) {
            return Err(ConfigError::MultipleHosts);
        }
        validate_mongo_database(database)?;
        validate_mongo_name("auth-database", auth_database)?;
        if user.trim().is_empty() {
            return Err(ConfigError::EmptyField { field: "usuário" });
        }
        Ok(Self {
            host,
            port,
            database: database.to_owned(),
            user: user.to_owned(),
            auth_database: auth_database.to_owned(),
            tls,
        })
    }

    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    #[must_use]
    pub fn database(&self) -> &str {
        &self.database
    }

    #[must_use]
    pub fn user(&self) -> &str {
        &self.user
    }

    #[must_use]
    pub fn auth_database(&self) -> &str {
        &self.auth_database
    }

    #[must_use]
    pub fn tls(&self) -> TlsRequirement {
        self.tls
    }

    #[must_use]
    pub fn is_local(&self) -> bool {
        is_local_host(&self.host)
    }

    pub fn with_database(&self, database: &str) -> Result<Self, ConfigError> {
        validate_mongo_database(database)?;
        Ok(Self {
            database: database.to_owned(),
            ..self.clone()
        })
    }
}

impl fmt::Display for MongoConnectionConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}@{}:{}/{}",
            self.user, self.host, self.port, self.database
        )
    }
}

pub struct Password(String);

impl Password {
    #[must_use]
    pub fn new(value: String) -> Self {
        Self(value)
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Password(***)")
    }
}

impl Drop for Password {
    fn drop(&mut self) {
        let mut bytes = std::mem::take(&mut self.0).into_bytes();
        bytes.iter_mut().for_each(|byte| *byte = 0);
        std::hint::black_box(&bytes);
    }
}

fn validate_server(
    host: &str,
    port: u16,
    tls: TlsMode,
) -> Result<(String, TlsRequirement), ConfigError> {
    let host = host.trim();
    if host.is_empty() {
        return Err(ConfigError::EmptyHost);
    }
    if host.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(ConfigError::InvalidHost);
    }
    if port == 0 {
        return Err(ConfigError::InvalidPort);
    }
    let tls = match (tls, is_local_host(host)) {
        (TlsMode::Auto, true) => TlsRequirement::Plain,
        (TlsMode::Auto, false) | (TlsMode::VerifyFull, _) => TlsRequirement::VerifyFull,
        (TlsMode::Disable, true) => TlsRequirement::Plain,
        (TlsMode::Disable, false) => {
            return Err(ConfigError::PlainTlsForRemoteHost {
                host: host.to_owned(),
            });
        }
    };
    Ok((host.to_owned(), tls))
}

fn validate_identifier(field: &'static str, value: &str) -> Result<(), ConfigError> {
    if value.trim().is_empty() {
        return Err(ConfigError::EmptyField { field });
    }
    if value.len() > MAX_IDENTIFIER_BYTES {
        return Err(ConfigError::FieldTooLong {
            field,
            max: MAX_IDENTIFIER_BYTES,
            unit: "bytes",
        });
    }
    Ok(())
}

fn validate_mysql_database(value: &str) -> Result<(), ConfigError> {
    validate_chars("database", value, MYSQL_MAX_DATABASE_CHARS)?;
    if value.ends_with(' ') || value.contains(['/', '\\', '.', '\0']) {
        return Err(ConfigError::InvalidName { field: "database" });
    }
    Ok(())
}

fn validate_mongo_name(field: &'static str, value: &str) -> Result<(), ConfigError> {
    validate_identifier(field, value)?;
    if value.contains(MONGO_FORBIDDEN_NAME_CHARS) {
        return Err(ConfigError::InvalidMongoName { field });
    }
    Ok(())
}

fn validate_mongo_database(value: &str) -> Result<(), ConfigError> {
    validate_mongo_name("database", value)?;
    if MONGO_RESERVED_DATABASES.contains(&value) {
        return Err(ConfigError::ReservedDatabase {
            name: value.to_owned(),
        });
    }
    Ok(())
}

fn validate_chars(field: &'static str, value: &str, max: usize) -> Result<(), ConfigError> {
    if value.trim().is_empty() {
        return Err(ConfigError::EmptyField { field });
    }
    if value.chars().count() > max {
        return Err(ConfigError::FieldTooLong {
            field,
            max,
            unit: "caracteres",
        });
    }
    Ok(())
}

fn is_local_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.trim_matches(['[', ']'])
        .parse::<IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(host: &str, tls: TlsMode) -> Result<PostgresConnectionConfig, ConfigError> {
        PostgresConnectionConfig::new(host, 5432, "my_database", "postgres", tls)
    }

    #[test]
    fn accepts_local_config_without_tls() {
        let config = config("localhost", TlsMode::Auto).expect("configuração válida");
        assert_eq!(config.tls(), TlsRequirement::Plain);
        assert!(config.is_local());
        assert_eq!(config.to_string(), "postgres@localhost:5432/my_database");
    }

    #[test]
    fn loopback_addresses_are_local() {
        for host in ["127.0.0.1", "::1", "[::1]", "LOCALHOST"] {
            let config = config(host, TlsMode::Auto).expect("configuração válida");
            assert_eq!(config.tls(), TlsRequirement::Plain, "{host}");
        }
    }

    #[test]
    fn remote_host_requires_tls() {
        let auto = config("db.example.com", TlsMode::Auto).expect("configuração válida");
        assert_eq!(auto.tls(), TlsRequirement::VerifyFull);
        assert_eq!(
            config("10.0.0.5", TlsMode::Disable),
            Err(ConfigError::PlainTlsForRemoteHost {
                host: "10.0.0.5".to_owned()
            })
        );
    }

    #[test]
    fn local_host_can_force_tls() {
        let config = config("localhost", TlsMode::VerifyFull).expect("configuração válida");
        assert_eq!(config.tls(), TlsRequirement::VerifyFull);
    }

    #[test]
    fn rejects_invalid_parameters() {
        let cases = [
            (("", 5432, "db", "user"), ConfigError::EmptyHost),
            (("my host", 5432, "db", "user"), ConfigError::InvalidHost),
            (("localhost", 0, "db", "user"), ConfigError::InvalidPort),
            (
                ("localhost", 5432, " ", "user"),
                ConfigError::EmptyField { field: "database" },
            ),
            (
                ("localhost", 5432, "db", ""),
                ConfigError::EmptyField { field: "usuário" },
            ),
        ];
        for ((host, port, database, user), expected) in cases {
            assert_eq!(
                PostgresConnectionConfig::new(host, port, database, user, TlsMode::Auto),
                Err(expected)
            );
        }
    }

    #[test]
    fn rejects_identifiers_longer_than_postgres_limit() {
        let long = "a".repeat(MAX_IDENTIFIER_BYTES + 1);
        assert_eq!(
            PostgresConnectionConfig::new("localhost", 5432, &long, "user", TlsMode::Auto),
            Err(ConfigError::FieldTooLong {
                field: "database",
                max: MAX_IDENTIFIER_BYTES,
                unit: "bytes"
            })
        );
    }

    #[test]
    fn mysql_config_applies_mysql_name_limits_and_tls_rules() {
        let ok = MysqlConnectionConfig::new("localhost", 3306, "app", "root", TlsMode::Auto)
            .expect("configuração válida");
        assert_eq!(ok.tls(), TlsRequirement::Plain);
        assert_eq!(ok.to_string(), "root@localhost:3306/app");
        let accented = "ç".repeat(MYSQL_MAX_DATABASE_CHARS);
        assert!(ok.with_database(&accented).is_ok());

        let too_long = "a".repeat(MYSQL_MAX_DATABASE_CHARS + 1);
        let long_user = "u".repeat(MYSQL_MAX_USER_CHARS + 1);
        let cases = [
            (
                ("localhost", too_long.as_str(), "root", TlsMode::Auto),
                ConfigError::FieldTooLong {
                    field: "database",
                    max: MYSQL_MAX_DATABASE_CHARS,
                    unit: "caracteres",
                },
            ),
            (
                ("localhost", "app", long_user.as_str(), TlsMode::Auto),
                ConfigError::FieldTooLong {
                    field: "usuário",
                    max: MYSQL_MAX_USER_CHARS,
                    unit: "caracteres",
                },
            ),
            (
                ("localhost", "../etc", "root", TlsMode::Auto),
                ConfigError::InvalidName { field: "database" },
            ),
            (
                ("localhost", "app ", "root", TlsMode::Auto),
                ConfigError::InvalidName { field: "database" },
            ),
            (
                ("db.example.com", "app", "root", TlsMode::Disable),
                ConfigError::PlainTlsForRemoteHost {
                    host: "db.example.com".to_owned(),
                },
            ),
        ];
        for ((host, database, user, tls), expected) in cases {
            assert_eq!(
                MysqlConnectionConfig::new(host, 3306, database, user, tls),
                Err(expected)
            );
        }
    }

    #[test]
    fn mongodb_config_validates_names_hosts_and_tls() {
        let new = |host: &str, database: &str, user: &str, auth: &str, tls: TlsMode| {
            MongoConnectionConfig::new(host, 27017, database, user, auth, tls)
        };
        let ok =
            new("localhost", "app", "root", "admin", TlsMode::Auto).expect("configuração válida");
        assert_eq!(ok.tls(), TlsRequirement::Plain);
        assert!(ok.is_local());
        assert_eq!(ok.auth_database(), "admin");
        assert_eq!(ok.to_string(), "root@localhost:27017/app");
        let other = ok.with_database("copia").expect("banco válido");
        assert_eq!(
            (other.database(), other.auth_database()),
            ("copia", "admin")
        );

        for name in [
            "a.b", "a b", "a\"b", "a$b", "a*b", "a/b", "a\\b", "a<b", "a>b", "a:b", "a|b", "a?b",
            "a\0b", " app",
        ] {
            assert_eq!(
                new("localhost", name, "root", "admin", TlsMode::Auto),
                Err(ConfigError::InvalidMongoName { field: "database" }),
                "{name:?}"
            );
            assert_eq!(
                ok.with_database(name),
                Err(ConfigError::InvalidMongoName { field: "database" }),
                "{name:?}"
            );
        }
        for name in ["admin", "local", "config"] {
            assert_eq!(
                new("localhost", name, "root", "admin", TlsMode::Auto),
                Err(ConfigError::ReservedDatabase {
                    name: name.to_owned()
                })
            );
        }
        assert!(new("localhost", "app", "root", "app_users", TlsMode::Auto).is_ok());
        assert_eq!(
            new("localhost", "app", "root", "a.b", TlsMode::Auto),
            Err(ConfigError::InvalidMongoName {
                field: "auth-database"
            })
        );
        assert_eq!(
            new("localhost", "app", " ", "admin", TlsMode::Auto),
            Err(ConfigError::EmptyField { field: "usuário" })
        );
        assert_eq!(
            new(
                "localhost",
                &"a".repeat(MAX_IDENTIFIER_BYTES + 1),
                "root",
                "admin",
                TlsMode::Auto
            ),
            Err(ConfigError::FieldTooLong {
                field: "database",
                max: MAX_IDENTIFIER_BYTES,
                unit: "bytes"
            })
        );

        for host in [
            "a.example,b.example",
            "rs0/a.example",
            "mongodb://a.example",
        ] {
            assert_eq!(
                new(host, "app", "root", "admin", TlsMode::VerifyFull),
                Err(ConfigError::MultipleHosts),
                "{host}"
            );
        }
        assert_eq!(
            new("db.example.com", "app", "root", "admin", TlsMode::Disable),
            Err(ConfigError::PlainTlsForRemoteHost {
                host: "db.example.com".to_owned()
            })
        );
        let remote = new("db.example.com", "app", "root", "admin", TlsMode::Auto)
            .expect("configuração válida");
        assert_eq!(remote.tls(), TlsRequirement::VerifyFull);
        assert!(!remote.is_local());
    }

    #[test]
    fn with_database_keeps_connection_and_validates_name() {
        let config = config("localhost", TlsMode::Auto).expect("configuração válida");
        let other = config.with_database("postgres").expect("banco válido");

        assert_eq!(other.database(), "postgres");
        assert_eq!(other.host(), "localhost");
        assert_eq!(other.tls(), config.tls());
        assert_eq!(
            config.with_database(""),
            Err(ConfigError::EmptyField { field: "database" })
        );
    }

    #[test]
    fn password_debug_is_redacted() {
        let password = Password::new("super-secret".to_owned());
        assert_eq!(format!("{password:?}"), "Password(***)");
        assert_eq!(password.expose(), "super-secret");
    }
}
