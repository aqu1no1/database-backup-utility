use std::fmt;
use std::net::IpAddr;

use thiserror::Error;

const MAX_IDENTIFIER_BYTES: usize = 63;

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
    #[error("o {field} excede {MAX_IDENTIFIER_BYTES} bytes")]
    FieldTooLong { field: &'static str },
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
        validate_identifier("database", database)?;
        validate_identifier("usuário", user)?;

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

        Ok(Self {
            host: host.to_owned(),
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

fn validate_identifier(field: &'static str, value: &str) -> Result<(), ConfigError> {
    if value.trim().is_empty() {
        return Err(ConfigError::EmptyField { field });
    }
    if value.len() > MAX_IDENTIFIER_BYTES {
        return Err(ConfigError::FieldTooLong { field });
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
            Err(ConfigError::FieldTooLong { field: "database" })
        );
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
