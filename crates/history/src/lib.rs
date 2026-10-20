use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use directories::ProjectDirs;
use rusqlite::{Connection, OptionalExtension, Row, params};
use rusqlite_migration::{M, Migrations};
use thiserror::Error;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

pub const DATA_DIR_ENV: &str = "DBU_DATA_DIR";
pub const DATABASE_FILE: &str = "history.sqlite3";

const MIGRATIONS: &[M<'static>] = &[
    M::up(include_str!("../migrations/0001_initial.sql")),
    M::up(include_str!("../migrations/0002_restores.sql")),
];

#[derive(Debug, Error)]
pub enum HistoryError {
    #[error("não foi possível determinar o diretório de dados do usuário; defina {DATA_DIR_ENV}")]
    NoDataDir,
    #[error("falha ao criar o diretório do histórico {path}")]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("falha no banco do histórico")]
    Sqlite(#[from] rusqlite::Error),
    #[error("falha ao aplicar as migrations do histórico")]
    Migration(#[from] rusqlite_migration::Error),
    #[error("data inválida no histórico")]
    Timestamp,
    #[error("status inválido no histórico: {0}")]
    InvalidStatus(String),
    #[error("a execução {id} não existe ou já foi finalizada")]
    ExecutionNotRunning { id: String },
    #[error("uma execução não pode ser finalizada com o status running")]
    FinishWithRunning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionStatus {
    Running,
    Succeeded,
    Failed,
    Canceled,
}

impl ExecutionStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
        }
    }
}

impl fmt::Display for ExecutionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ExecutionStatus {
    type Err = HistoryError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "running" => Ok(Self::Running),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            "canceled" => Ok(Self::Canceled),
            other => Err(HistoryError::InvalidStatus(other.to_owned())),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct NewConnection<'a> {
    pub display_name: &'a str,
    pub engine: &'a str,
    pub host: Option<&'a str>,
    pub port: Option<u16>,
    pub database_name: &'a str,
    pub username: Option<&'a str>,
    pub secret_ref: Option<&'a str>,
}

#[derive(Debug, Clone, Copy)]
pub struct NewArtifact<'a> {
    pub location: &'a str,
    pub format: &'a str,
    pub size_bytes: u64,
    pub checksum_sha256: Option<&'a str>,
    pub tool_version: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreMode {
    Create,
    Overwrite,
}

impl RestoreMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Overwrite => "overwrite",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct NewRestore<'a> {
    pub artifact_id: Option<&'a str>,
    pub source_location: &'a str,
    pub source_sha256: Option<&'a str>,
    pub target_host: &'a str,
    pub target_port: u16,
    pub target_database: &'a str,
    pub mode: RestoreMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreRecord {
    pub id: String,
    pub artifact_id: Option<String>,
    pub source_location: String,
    pub target_database: String,
    pub mode: String,
    pub previous_database: Option<String>,
    pub status: ExecutionStatus,
    pub duration_ms: Option<i64>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionRecord {
    pub id: String,
    pub job_id: String,
    pub status: ExecutionStatus,
    pub started_at: OffsetDateTime,
    pub finished_at: Option<OffsetDateTime>,
    pub duration_ms: Option<i64>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactRecord {
    pub id: String,
    pub execution_id: String,
    pub location: String,
    pub format: String,
    pub size_bytes: u64,
    pub checksum_sha256: Option<String>,
    pub tool_version: Option<String>,
    pub created_at: OffsetDateTime,
}

pub struct History {
    conn: Connection,
}

impl History {
    pub fn default_path() -> Result<PathBuf, HistoryError> {
        if let Some(dir) = std::env::var_os(DATA_DIR_ENV).filter(|value| !value.is_empty()) {
            return Ok(PathBuf::from(dir).join(DATABASE_FILE));
        }
        ProjectDirs::from("dev", "aqu1no1", "database-backup-utility")
            .map(|dirs| dirs.data_dir().join(DATABASE_FILE))
            .ok_or(HistoryError::NoDataDir)
    }

    pub fn open(path: &Path) -> Result<Self, HistoryError> {
        Self::open_with(path, &Migrations::from_slice(MIGRATIONS))
    }

    fn open_with(path: &Path, migrations: &Migrations<'_>) -> Result<Self, HistoryError> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).map_err(|source| HistoryError::CreateDir {
                path: parent.to_owned(),
                source,
            })?;
        }
        let mut conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", true)?;
        migrations.to_latest(&mut conn)?;
        Ok(Self { conn })
    }

    pub fn schema_version(&self) -> Result<i64, HistoryError> {
        Ok(self
            .conn
            .pragma_query_value(None, "user_version", |row| row.get(0))?)
    }

    pub fn upsert_connection(
        &self,
        connection: &NewConnection<'_>,
    ) -> Result<String, HistoryError> {
        let now = now()?;
        let id = self.conn.query_row(
            "INSERT INTO connections
                 (id, display_name, engine, host, port, database_name, username, secret_ref, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)
             ON CONFLICT (display_name) DO UPDATE SET
                 engine = excluded.engine,
                 host = excluded.host,
                 port = excluded.port,
                 database_name = excluded.database_name,
                 username = excluded.username,
                 secret_ref = excluded.secret_ref,
                 updated_at = excluded.updated_at
             RETURNING id",
            params![
                new_id(),
                connection.display_name,
                connection.engine,
                connection.host,
                connection.port,
                connection.database_name,
                connection.username,
                connection.secret_ref,
                now,
            ],
            |row| row.get(0),
        )?;
        Ok(id)
    }

    pub fn create_job(&self, connection_id: &str, name: &str) -> Result<String, HistoryError> {
        let id = self.conn.query_row(
            "INSERT INTO backup_jobs (id, connection_id, name, backup_type, created_at)
             VALUES (?1, ?2, ?3, 'full', ?4)
             ON CONFLICT (connection_id, name) DO UPDATE SET name = excluded.name
             RETURNING id",
            params![new_id(), connection_id, name, now()?],
            |row| row.get(0),
        )?;
        Ok(id)
    }

    pub fn start_execution(&self, job_id: &str) -> Result<String, HistoryError> {
        let id = new_id();
        self.conn.execute(
            "INSERT INTO backup_executions (id, job_id, status, started_at) VALUES (?1, ?2, 'running', ?3)",
            params![id, job_id, now()?],
        )?;
        Ok(id)
    }

    pub fn finish_execution(
        &self,
        id: &str,
        status: ExecutionStatus,
        error_message: Option<&str>,
    ) -> Result<(), HistoryError> {
        if status == ExecutionStatus::Running {
            return Err(HistoryError::FinishWithRunning);
        }
        let started_at: Option<String> = self
            .conn
            .query_row(
                "SELECT started_at FROM backup_executions WHERE id = ?1 AND status = 'running'",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(started_at) = started_at else {
            return Err(HistoryError::ExecutionNotRunning { id: id.to_owned() });
        };

        let finished_at = OffsetDateTime::now_utc();
        let duration_ms = elapsed_ms(&started_at, finished_at)?;
        self.conn.execute(
            "UPDATE backup_executions
             SET status = ?2, finished_at = ?3, duration_ms = ?4, error_message = ?5
             WHERE id = ?1",
            params![
                id,
                status.as_str(),
                format_time(finished_at)?,
                duration_ms,
                error_message
            ],
        )?;
        Ok(())
    }

    pub fn add_artifact(
        &self,
        execution_id: &str,
        artifact: &NewArtifact<'_>,
    ) -> Result<String, HistoryError> {
        let id = new_id();
        let size_bytes = i64::try_from(artifact.size_bytes).unwrap_or(i64::MAX);
        self.conn.execute(
            "INSERT INTO backup_artifacts
                 (id, execution_id, location, format, size_bytes, checksum_sha256, tool_version, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                id,
                execution_id,
                artifact.location,
                artifact.format,
                size_bytes,
                artifact.checksum_sha256,
                artifact.tool_version,
                now()?,
            ],
        )?;
        Ok(id)
    }

    pub fn list_executions(
        &self,
        job_id: &str,
        limit: u32,
    ) -> Result<Vec<ExecutionRecord>, HistoryError> {
        let mut statement = self.conn.prepare(
            "SELECT id, job_id, status, started_at, finished_at, duration_ms, error_message
             FROM backup_executions WHERE job_id = ?1
             ORDER BY started_at DESC, id DESC LIMIT ?2",
        )?;
        let rows = statement.query_map(params![job_id, limit], raw_execution)?;
        rows.map(|row| row?.try_into()).collect()
    }

    pub fn get_artifacts(&self, execution_id: &str) -> Result<Vec<ArtifactRecord>, HistoryError> {
        let mut statement = self.conn.prepare(
            "SELECT id, execution_id, location, format, size_bytes, checksum_sha256, tool_version, created_at
             FROM backup_artifacts WHERE execution_id = ?1 ORDER BY created_at, id",
        )?;
        let rows = statement.query_map([execution_id], raw_artifact)?;
        rows.map(|row| row?.try_into()).collect()
    }

    pub fn find_artifact_by_location(
        &self,
        location: &str,
    ) -> Result<Option<ArtifactRecord>, HistoryError> {
        let raw = self
            .conn
            .query_row(
                "SELECT id, execution_id, location, format, size_bytes, checksum_sha256, tool_version, created_at
                 FROM backup_artifacts WHERE location = ?1 ORDER BY created_at DESC LIMIT 1",
                [location],
                raw_artifact,
            )
            .optional()?;
        raw.map(TryInto::try_into).transpose()
    }

    pub fn start_restore(&self, restore: &NewRestore<'_>) -> Result<String, HistoryError> {
        let id = new_id();
        self.conn.execute(
            "INSERT INTO restore_executions
                 (id, artifact_id, source_location, source_sha256, target_host, target_port,
                  target_database, mode, status, started_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'running', ?9)",
            params![
                id,
                restore.artifact_id,
                restore.source_location,
                restore.source_sha256,
                restore.target_host,
                restore.target_port,
                restore.target_database,
                restore.mode.as_str(),
                now()?,
            ],
        )?;
        Ok(id)
    }

    pub fn finish_restore(
        &self,
        id: &str,
        status: ExecutionStatus,
        previous_database: Option<&str>,
        error_message: Option<&str>,
    ) -> Result<(), HistoryError> {
        if status == ExecutionStatus::Running {
            return Err(HistoryError::FinishWithRunning);
        }
        let started_at: Option<String> = self
            .conn
            .query_row(
                "SELECT started_at FROM restore_executions WHERE id = ?1 AND status = 'running'",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(started_at) = started_at else {
            return Err(HistoryError::ExecutionNotRunning { id: id.to_owned() });
        };
        let finished_at = OffsetDateTime::now_utc();
        self.conn.execute(
            "UPDATE restore_executions
             SET status = ?2, finished_at = ?3, duration_ms = ?4, previous_database = ?5,
                 error_message = ?6
             WHERE id = ?1",
            params![
                id,
                status.as_str(),
                format_time(finished_at)?,
                elapsed_ms(&started_at, finished_at)?,
                previous_database,
                error_message
            ],
        )?;
        Ok(())
    }

    pub fn list_restores(&self, limit: u32) -> Result<Vec<RestoreRecord>, HistoryError> {
        let mut statement = self.conn.prepare(
            "SELECT id, artifact_id, source_location, target_database, mode, previous_database,
                    status, duration_ms, error_message
             FROM restore_executions ORDER BY started_at DESC, id DESC LIMIT ?1",
        )?;
        let rows = statement.query_map([limit], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, Option<i64>>(7)?,
                row.get::<_, Option<String>>(8)?,
            ))
        })?;
        rows.map(|row| {
            let (
                id,
                artifact_id,
                source_location,
                target_database,
                mode,
                previous_database,
                status,
                duration_ms,
                error_message,
            ) = row?;
            Ok(RestoreRecord {
                id,
                artifact_id,
                source_location,
                target_database,
                mode,
                previous_database,
                status: status.parse()?,
                duration_ms,
                error_message,
            })
        })
        .collect()
    }
}

struct RawExecution {
    id: String,
    job_id: String,
    status: String,
    started_at: String,
    finished_at: Option<String>,
    duration_ms: Option<i64>,
    error_message: Option<String>,
}

fn raw_execution(row: &Row<'_>) -> rusqlite::Result<RawExecution> {
    Ok(RawExecution {
        id: row.get(0)?,
        job_id: row.get(1)?,
        status: row.get(2)?,
        started_at: row.get(3)?,
        finished_at: row.get(4)?,
        duration_ms: row.get(5)?,
        error_message: row.get(6)?,
    })
}

impl TryFrom<RawExecution> for ExecutionRecord {
    type Error = HistoryError;

    fn try_from(raw: RawExecution) -> Result<Self, Self::Error> {
        Ok(Self {
            id: raw.id,
            job_id: raw.job_id,
            status: raw.status.parse()?,
            started_at: parse_time(&raw.started_at)?,
            finished_at: raw.finished_at.as_deref().map(parse_time).transpose()?,
            duration_ms: raw.duration_ms,
            error_message: raw.error_message,
        })
    }
}

struct RawArtifact {
    id: String,
    execution_id: String,
    location: String,
    format: String,
    size_bytes: i64,
    checksum_sha256: Option<String>,
    tool_version: Option<String>,
    created_at: String,
}

fn raw_artifact(row: &Row<'_>) -> rusqlite::Result<RawArtifact> {
    Ok(RawArtifact {
        id: row.get(0)?,
        execution_id: row.get(1)?,
        location: row.get(2)?,
        format: row.get(3)?,
        size_bytes: row.get(4)?,
        checksum_sha256: row.get(5)?,
        tool_version: row.get(6)?,
        created_at: row.get(7)?,
    })
}

impl TryFrom<RawArtifact> for ArtifactRecord {
    type Error = HistoryError;

    fn try_from(raw: RawArtifact) -> Result<Self, Self::Error> {
        Ok(Self {
            id: raw.id,
            execution_id: raw.execution_id,
            location: raw.location,
            format: raw.format,
            size_bytes: u64::try_from(raw.size_bytes).unwrap_or(0),
            checksum_sha256: raw.checksum_sha256,
            tool_version: raw.tool_version,
            created_at: parse_time(&raw.created_at)?,
        })
    }
}

fn elapsed_ms(started_at: &str, finished_at: OffsetDateTime) -> Result<i64, HistoryError> {
    let elapsed = finished_at - parse_time(started_at)?;
    Ok(i64::try_from(elapsed.whole_milliseconds().max(0)).unwrap_or(i64::MAX))
}

fn new_id() -> String {
    Uuid::now_v7().to_string()
}

fn now() -> Result<String, HistoryError> {
    format_time(OffsetDateTime::now_utc())
}

fn format_time(value: OffsetDateTime) -> Result<String, HistoryError> {
    value.format(&Rfc3339).map_err(|_| HistoryError::Timestamp)
}

fn parse_time(value: &str) -> Result<OffsetDateTime, HistoryError> {
    OffsetDateTime::parse(value, &Rfc3339).map_err(|_| HistoryError::Timestamp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn temp_history() -> (TempDir, PathBuf, History) {
        let dir = TempDir::new().expect("diretório temporário");
        let path = dir.path().join("dados").join(DATABASE_FILE);
        let history = History::open(&path).expect("abrir histórico");
        (dir, path, history)
    }

    fn sample_connection() -> NewConnection<'static> {
        NewConnection {
            display_name: "dev-local",
            engine: "postgres",
            host: Some("localhost"),
            port: Some(5432),
            database_name: "my_database",
            username: Some("postgres"),
            secret_ref: Some("env:DBU_PG_PASSWORD"),
        }
    }

    #[test]
    fn open_creates_database_and_applies_migrations() {
        let (_dir, path, history) = temp_history();

        assert!(path.exists());
        assert_eq!(
            history.schema_version().expect("versão"),
            MIGRATIONS.len() as i64
        );
    }

    #[test]
    fn reopening_is_idempotent() {
        let (_dir, path, history) = temp_history();
        let connection_id = history
            .upsert_connection(&sample_connection())
            .expect("conexão");
        drop(history);

        let reopened = History::open(&path).expect("reabrir");
        let again = reopened
            .upsert_connection(&sample_connection())
            .expect("conexão");

        assert_eq!(
            reopened.schema_version().expect("versão"),
            MIGRATIONS.len() as i64
        );
        assert_eq!(connection_id, again);
    }

    #[test]
    fn records_and_queries_a_full_execution() {
        let (_dir, _path, history) = temp_history();
        let connection_id = history
            .upsert_connection(&sample_connection())
            .expect("conexão");
        let job_id = history.create_job(&connection_id, "diario").expect("job");
        let execution_id = history.start_execution(&job_id).expect("início");
        let checksum = "a".repeat(64);
        let artifact = NewArtifact {
            location: "/backups/my_database.dump",
            format: "pg_dump-custom",
            size_bytes: 2048,
            checksum_sha256: Some(&checksum),
            tool_version: Some("pg_dump 17.11"),
        };
        history
            .add_artifact(&execution_id, &artifact)
            .expect("artefato");
        history
            .finish_execution(&execution_id, ExecutionStatus::Succeeded, None)
            .expect("fim");

        let executions = history.list_executions(&job_id, 10).expect("execuções");
        let artifacts = history.get_artifacts(&execution_id).expect("artefatos");

        assert_eq!(executions.len(), 1);
        let execution = &executions[0];
        assert_eq!(execution.status, ExecutionStatus::Succeeded);
        assert!(
            execution
                .finished_at
                .is_some_and(|end| end >= execution.started_at)
        );
        assert!(execution.duration_ms.is_some_and(|ms| ms >= 0));
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].size_bytes, 2048);
        assert_eq!(
            artifacts[0].checksum_sha256.as_deref(),
            Some(checksum.as_str())
        );
    }

    #[test]
    fn failed_execution_keeps_sanitized_error() {
        let (_dir, _path, history) = temp_history();
        let connection_id = history
            .upsert_connection(&sample_connection())
            .expect("conexão");
        let job_id = history.create_job(&connection_id, "diario").expect("job");
        let execution_id = history.start_execution(&job_id).expect("início");

        history
            .finish_execution(
                &execution_id,
                ExecutionStatus::Failed,
                Some("pg_dump saiu com código 1"),
            )
            .expect("fim");
        let error = history
            .finish_execution(&execution_id, ExecutionStatus::Succeeded, None)
            .expect_err("não pode finalizar duas vezes");

        let execution = &history.list_executions(&job_id, 1).expect("execuções")[0];
        assert_eq!(execution.status, ExecutionStatus::Failed);
        assert_eq!(
            execution.error_message.as_deref(),
            Some("pg_dump saiu com código 1")
        );
        assert!(matches!(error, HistoryError::ExecutionNotRunning { .. }));
    }

    #[test]
    fn upgrading_from_first_schema_keeps_backups_and_adds_restores() {
        let dir = TempDir::new().expect("diretório temporário");
        let path = dir.path().join(DATABASE_FILE);
        let first = History::open_with(&path, &Migrations::from_slice(&MIGRATIONS[..1]))
            .expect("schema 0001");
        let connection_id = first
            .upsert_connection(&sample_connection())
            .expect("conexão");
        assert_eq!(first.schema_version().expect("versão"), 1);
        drop(first);

        let upgraded = History::open(&path).expect("schema atual");
        let again = upgraded
            .upsert_connection(&sample_connection())
            .expect("conexão");

        assert_eq!(upgraded.schema_version().expect("versão"), 2);
        assert_eq!(again, connection_id);
        assert!(upgraded.list_restores(10).expect("restores").is_empty());
    }

    #[test]
    fn records_restore_linked_to_artifact() {
        let (_dir, _path, history) = temp_history();
        let connection_id = history
            .upsert_connection(&sample_connection())
            .expect("conexão");
        let job_id = history.create_job(&connection_id, "diario").expect("job");
        let execution_id = history.start_execution(&job_id).expect("início");
        let checksum = "b".repeat(64);
        let artifact_id = history
            .add_artifact(
                &execution_id,
                &NewArtifact {
                    location: "/backups/db.dump",
                    format: "pg_dump-custom",
                    size_bytes: 10,
                    checksum_sha256: Some(&checksum),
                    tool_version: None,
                },
            )
            .expect("artefato");

        let found = history
            .find_artifact_by_location("/backups/db.dump")
            .expect("busca")
            .expect("artefato encontrado");
        let restore_id = history
            .start_restore(&NewRestore {
                artifact_id: Some(&found.id),
                source_location: &found.location,
                source_sha256: found.checksum_sha256.as_deref(),
                target_host: "localhost",
                target_port: 5432,
                target_database: "db",
                mode: RestoreMode::Overwrite,
            })
            .expect("início do restore");
        history
            .finish_restore(
                &restore_id,
                ExecutionStatus::Succeeded,
                Some("db__previous_20261020T190000Z"),
                None,
            )
            .expect("fim do restore");

        let restores = history.list_restores(10).expect("restores");
        assert_eq!(found.id, artifact_id);
        assert_eq!(restores.len(), 1);
        assert_eq!(restores[0].status, ExecutionStatus::Succeeded);
        assert_eq!(restores[0].mode, "overwrite");
        assert_eq!(
            restores[0].previous_database.as_deref(),
            Some("db__previous_20261020T190000Z")
        );
        assert!(restores[0].duration_ms.is_some());
        assert!(
            history
                .find_artifact_by_location("/outro.dump")
                .expect("busca")
                .is_none()
        );
    }

    #[test]
    fn migrating_existing_database_keeps_data() {
        let (_dir, path, history) = temp_history();
        let connection_id = history
            .upsert_connection(&sample_connection())
            .expect("conexão");
        drop(history);

        let mut evolved = MIGRATIONS.to_vec();
        evolved.push(M::up("ALTER TABLE connections ADD COLUMN notes TEXT;"));
        let upgraded = History::open_with(&path, &Migrations::new(evolved)).expect("migrar");

        let (name, notes): (String, Option<String>) = upgraded
            .conn
            .query_row(
                "SELECT display_name, notes FROM connections WHERE id = ?1",
                [&connection_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("conexão preservada");
        assert_eq!(
            upgraded.schema_version().expect("versão"),
            MIGRATIONS.len() as i64 + 1
        );
        assert_eq!(name, "dev-local");
        assert_eq!(notes, None);
    }

    #[test]
    fn schema_has_no_password_columns() {
        let (_dir, _path, history) = temp_history();
        let mut statement = history
            .conn
            .prepare(
                "SELECT m.name, p.name FROM sqlite_schema m
                 JOIN pragma_table_info(m.name) p
                 WHERE m.type = 'table'",
            )
            .expect("consulta do schema");
        let columns: Vec<(String, String)> = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("colunas")
            .collect::<Result<_, _>>()
            .expect("colunas");

        assert!(!columns.is_empty());
        for (table, column) in columns {
            let lowered = column.to_lowercase();
            assert!(
                !lowered.contains("password") && !lowered.contains("token"),
                "{table}.{column}"
            );
        }
    }

    #[test]
    fn artifact_requires_existing_execution() {
        let (_dir, _path, history) = temp_history();
        let artifact = NewArtifact {
            location: "/tmp/x.dump",
            format: "pg_dump-custom",
            size_bytes: 1,
            checksum_sha256: None,
            tool_version: None,
        };

        let result = history.add_artifact("execucao-inexistente", &artifact);

        assert!(matches!(result, Err(HistoryError::Sqlite(_))));
    }
}
