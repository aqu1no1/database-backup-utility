use std::fmt;
use std::str::FromStr;

use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseEngine {
    Postgres,
    Mysql,
    Sqlite,
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error("SGBD desconhecido: {0}")]
pub struct UnknownEngine(pub String);

impl DatabaseEngine {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
            Self::Mysql => "mysql",
            Self::Sqlite => "sqlite",
        }
    }

    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Postgres => "PostgreSQL",
            Self::Mysql => "MySQL",
            Self::Sqlite => "SQLite",
        }
    }
}

impl fmt::Display for DatabaseEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for DatabaseEngine {
    type Err = UnknownEngine;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "postgres" | "postgresql" => Ok(Self::Postgres),
            "mysql" => Ok(Self::Mysql),
            "sqlite" => Ok(Self::Sqlite),
            other => Err(UnknownEngine(other.to_owned())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_history_names_and_the_v1_manifest_name() {
        assert_eq!("postgres".parse(), Ok(DatabaseEngine::Postgres));
        assert_eq!("postgresql".parse(), Ok(DatabaseEngine::Postgres));
        assert_eq!("mysql".parse(), Ok(DatabaseEngine::Mysql));
        assert_eq!("sqlite".parse(), Ok(DatabaseEngine::Sqlite));
        assert_eq!(
            "mongodb".parse::<DatabaseEngine>(),
            Err(UnknownEngine("mongodb".to_owned()))
        );
    }
}
