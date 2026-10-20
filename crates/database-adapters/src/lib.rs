#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseEngine {
    Postgres,
    MySql,
    MongoDb,
    Sqlite,
}

impl DatabaseEngine {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
            Self::MySql => "mysql",
            Self::MongoDb => "mongodb",
            Self::Sqlite => "sqlite",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DatabaseEngine;

    #[test]
    fn postgres_identifier_is_stable() {
        assert_eq!(DatabaseEngine::Postgres.as_str(), "postgres");
    }
}
