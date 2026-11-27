use std::collections::BTreeMap;
use std::path::Path;

use database_backup_history::{
    BackupSummary, ConnectionProfile, ExecutionStatus, History, HistoryError, RestoreRecord,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::{ErrorCode, default_output_dir};

pub const HISTORY_LIMIT: u32 = 200;
const MAX_DIRECTORY_BYTES: usize = 4096;
const RUNNER_KEY: &str = "runner";
const BACKUP_DIRECTORY_KEY: &str = "backup_directory";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BridgeError {
    code: &'static str,
    params: BTreeMap<&'static str, String>,
}

impl BridgeError {
    #[must_use]
    pub fn new(code: ErrorCode) -> Self {
        Self {
            code: code.as_str(),
            params: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn internal() -> Self {
        Self::new(ErrorCode::Internal)
    }

    fn invalid(field: &str) -> Self {
        let mut error = Self::new(ErrorCode::InvalidInput);
        error.params.insert("field", field.to_owned());
        error
    }

    #[must_use]
    pub fn code(&self) -> &'static str {
        self.code
    }
}

impl From<HistoryError> for BridgeError {
    fn from(_: HistoryError) -> Self {
        Self::internal()
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    #[serde(rename = "postgresql")]
    Postgres,
    Mysql,
    Mariadb,
    Mongodb,
    Sqlite,
}

impl Engine {
    fn parse(value: &str) -> Result<Self, BridgeError> {
        match value {
            "postgres" => Ok(Self::Postgres),
            "mysql" => Ok(Self::Mysql),
            "mariadb" => Ok(Self::Mariadb),
            "mongodb" => Ok(Self::Mongodb),
            "sqlite" => Ok(Self::Sqlite),
            _ => Err(BridgeError::internal()),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OperationKind {
    Backup,
    Restore,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OperationStatus {
    Running,
    Verified,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HistoryItem {
    pub id: String,
    pub kind: OperationKind,
    pub engine: Engine,
    pub source: String,
    pub destination: String,
    pub started_at: String,
    pub status: OperationStatus,
    pub error_code: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ServerConnection {
    pub name: String,
    pub engine: Engine,
    pub host: String,
    pub port: u16,
    pub database: String,
    pub user: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ToolRunner {
    Native,
    Docker,
}

impl ToolRunner {
    fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Docker => "docker",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    pub runner: ToolRunner,
    pub backup_directory: String,
}

pub fn history_items(history: &History, limit: u32) -> Result<Vec<HistoryItem>, BridgeError> {
    let mut items = history
        .list_backups(limit, None)?
        .into_iter()
        .map(backup_item)
        .chain(history.list_restores(limit)?.into_iter().map(restore_item))
        .collect::<Result<Vec<_>, _>>()?;
    items.sort_by(|(left, _), (right, _)| right.cmp(left));
    Ok(items
        .into_iter()
        .take(usize::try_from(limit).unwrap_or(usize::MAX))
        .map(|(_, item)| item)
        .collect())
}

pub fn connections(history: &History) -> Result<Vec<ServerConnection>, BridgeError> {
    history
        .list_profiles()?
        .into_iter()
        .map(server_connection)
        .collect()
}

pub fn settings(history: &History, history_path: &Path) -> Result<Settings, BridgeError> {
    let mut settings = Settings {
        runner: ToolRunner::Native,
        backup_directory: default_output_dir(history_path)
            .to_string_lossy()
            .into_owned(),
    };
    for (key, value) in history.settings()? {
        match key.as_str() {
            RUNNER_KEY => settings.runner = parse_runner(&value)?,
            BACKUP_DIRECTORY_KEY => settings.backup_directory = value,
            _ => {}
        }
    }
    Ok(settings)
}

pub fn save_settings(history: &History, settings: Settings) -> Result<Settings, BridgeError> {
    let directory = settings.backup_directory.as_str();
    if directory.is_empty()
        || directory.len() > MAX_DIRECTORY_BYTES
        || directory.contains('\0')
        || !Path::new(directory).is_absolute()
    {
        return Err(BridgeError::invalid("backupDirectory"));
    }
    history.save_settings(&[
        (RUNNER_KEY, settings.runner.as_str()),
        (BACKUP_DIRECTORY_KEY, directory),
    ])?;
    Ok(settings)
}

fn parse_runner(value: &str) -> Result<ToolRunner, BridgeError> {
    match value {
        "native" => Ok(ToolRunner::Native),
        "docker" => Ok(ToolRunner::Docker),
        _ => Err(BridgeError::internal()),
    }
}

fn started_at(value: OffsetDateTime) -> Result<String, BridgeError> {
    value.format(&Rfc3339).map_err(|_| BridgeError::internal())
}

fn cancelled_code(status: OperationStatus) -> Option<&'static str> {
    (status == OperationStatus::Cancelled).then(|| ErrorCode::Canceled.as_str())
}

fn backup_item(summary: BackupSummary) -> Result<(OffsetDateTime, HistoryItem), BridgeError> {
    let status = match summary.status {
        ExecutionStatus::Running => OperationStatus::Running,
        ExecutionStatus::Succeeded if summary.checksum_sha256.is_some() => {
            OperationStatus::Verified
        }
        ExecutionStatus::Succeeded => OperationStatus::Completed,
        ExecutionStatus::Failed => OperationStatus::Failed,
        ExecutionStatus::Canceled => OperationStatus::Cancelled,
    };
    let item = HistoryItem {
        id: summary.execution_id,
        kind: OperationKind::Backup,
        engine: Engine::parse(&summary.engine)?,
        source: summary.database,
        destination: summary.location.unwrap_or_default(),
        started_at: started_at(summary.started_at)?,
        status,
        error_code: cancelled_code(status),
    };
    Ok((summary.started_at, item))
}

fn restore_item(record: RestoreRecord) -> Result<(OffsetDateTime, HistoryItem), BridgeError> {
    let status = match record.status {
        ExecutionStatus::Running => OperationStatus::Running,
        ExecutionStatus::Succeeded => OperationStatus::Completed,
        ExecutionStatus::Failed => OperationStatus::Failed,
        ExecutionStatus::Canceled => OperationStatus::Cancelled,
    };
    let item = HistoryItem {
        id: record.id,
        kind: OperationKind::Restore,
        engine: Engine::parse(&record.engine)?,
        source: record.source_location,
        destination: record.target_database,
        started_at: started_at(record.started_at)?,
        status,
        error_code: cancelled_code(status),
    };
    Ok((record.started_at, item))
}

fn server_connection(profile: ConnectionProfile) -> Result<ServerConnection, BridgeError> {
    Ok(ServerConnection {
        engine: Engine::parse(&profile.engine)?,
        name: profile.name,
        host: profile.host,
        port: profile.port,
        database: profile.database_name,
        user: profile.username,
    })
}

#[cfg(test)]
mod tests {
    use database_backup_history::{DATABASE_FILE, NewConnection, NewRestore, RestoreMode};
    use serde_json::Value;
    use tempfile::TempDir;
    use time::macros::datetime;

    use super::*;

    const CONTRACT: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../apps/desktop/src/app/core/tauri/contract"
    );

    fn fixture(name: &str) -> Value {
        let path = format!("{CONTRACT}/{name}");
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
    }

    fn backup(status: ExecutionStatus, checksum: Option<&str>) -> BackupSummary {
        BackupSummary {
            execution_id: "bkp-1".to_owned(),
            engine: "postgres".to_owned(),
            database: "app".to_owned(),
            host: Some("localhost".to_owned()),
            port: Some(5432),
            status,
            started_at: datetime!(2026-11-20 14:25:00 UTC),
            duration_ms: Some(1200),
            error_message: None,
            location: Some("/srv/backups/app.dump".to_owned()),
            size_bytes: Some(1024),
            checksum_sha256: checksum.map(str::to_owned),
        }
    }

    fn restore(status: ExecutionStatus) -> RestoreRecord {
        RestoreRecord {
            id: "rst-1".to_owned(),
            engine: "mariadb".to_owned(),
            artifact_id: None,
            source_location: "/srv/backups/blog.sql".to_owned(),
            checksum_verified: true,
            target_database: "blog_copy".to_owned(),
            mode: "create".to_owned(),
            previous_database: None,
            status,
            started_at: datetime!(2026-11-21 09:00:00 UTC),
            duration_ms: None,
            error_message: Some("cancelado".to_owned()),
        }
    }

    fn history() -> (TempDir, std::path::PathBuf, History) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(DATABASE_FILE);
        let history = History::open(&path).unwrap();
        (dir, path, history)
    }

    #[test]
    fn history_items_match_the_angular_contract() {
        let items = [
            backup_item(backup(ExecutionStatus::Succeeded, Some(&"a".repeat(64)))),
            backup_item(backup(ExecutionStatus::Succeeded, None)),
            restore_item(restore(ExecutionStatus::Canceled)),
        ]
        .into_iter()
        .map(|item| item.unwrap().1)
        .collect::<Vec<_>>();

        assert_eq!(
            serde_json::to_value(items).unwrap(),
            fixture("history.json")
        );
    }

    #[test]
    fn connections_and_settings_match_the_angular_contract() {
        let profile = ConnectionProfile {
            name: "development".to_owned(),
            engine: "postgres".to_owned(),
            host: "localhost".to_owned(),
            port: 5432,
            database_name: "my_database".to_owned(),
            username: "postgres".to_owned(),
            tls: "auto".to_owned(),
            auth_database: None,
            password_file: Some("/secrets/pg.pass".to_owned()),
        };
        assert_eq!(
            serde_json::to_value([server_connection(profile).unwrap()]).unwrap(),
            fixture("connections.json")
        );

        let settings: Settings = serde_json::from_value(fixture("settings.json")).unwrap();
        assert_eq!(
            serde_json::to_value(&settings).unwrap(),
            fixture("settings.json")
        );
    }

    #[test]
    fn every_rust_error_code_is_known_to_the_interface() {
        let known = fixture("error-codes.json");
        let known = known.as_array().unwrap();
        for code in ErrorCode::ALL {
            assert!(
                known.contains(&Value::from(code.as_str())),
                "{}",
                code.as_str()
            );
        }
        let error = serde_json::to_value(BridgeError::invalid("backupDirectory")).unwrap();
        assert_eq!(
            error,
            serde_json::json!({ "code": "invalid_input", "params": { "field": "backupDirectory" } })
        );
    }

    #[test]
    fn settings_default_to_native_and_the_backups_folder_and_reject_relative_paths() {
        let (dir, path, history) = history();
        let defaults = settings(&history, &path).unwrap();
        assert_eq!(defaults.runner, ToolRunner::Native);
        assert_eq!(
            Path::new(&defaults.backup_directory),
            dir.path().join("backups")
        );

        for directory in ["", "backups", "/tmp/a\0b"] {
            let error = save_settings(
                &history,
                Settings {
                    runner: ToolRunner::Docker,
                    backup_directory: directory.to_owned(),
                },
            )
            .unwrap_err();
            assert_eq!(error.code(), "invalid_input", "{directory:?}");
        }
        assert_eq!(settings(&history, &path).unwrap(), defaults);

        let saved = Settings {
            runner: ToolRunner::Docker,
            backup_directory: "/srv/backups".to_owned(),
        };
        save_settings(&history, saved.clone()).unwrap();
        assert_eq!(settings(&history, &path).unwrap(), saved);
    }

    #[test]
    fn history_lists_backups_and_restores_from_the_real_history_newest_first() {
        let (_dir, _path, history) = history();
        assert!(history_items(&history, HISTORY_LIMIT).unwrap().is_empty());

        let connection = history
            .upsert_connection(&NewConnection {
                display_name: "dev",
                engine: "mysql",
                host: Some("localhost"),
                port: Some(3306),
                database_name: "loja",
                username: Some("backup"),
                secret_ref: Some("env:DBU_MYSQL_PASSWORD"),
            })
            .unwrap();
        let job = history.create_job(&connection, "dev-full").unwrap();
        let execution = history.start_execution(&job).unwrap();
        history
            .finish_execution(&execution, ExecutionStatus::Canceled, Some("cancelado"))
            .unwrap();
        let restore = history
            .start_restore(&NewRestore {
                artifact_id: None,
                source_location: "/srv/backups/loja.sql",
                source_sha256: None,
                checksum_verified: false,
                engine: "mysql",
                target_host: Some("localhost"),
                target_port: Some(3306),
                target_database: "loja_copia",
                mode: RestoreMode::Create,
            })
            .unwrap();

        let items = history_items(&history, HISTORY_LIMIT).unwrap();
        assert_eq!(items.len(), 2);
        assert!(
            items
                .windows(2)
                .all(|pair| pair[0].started_at >= pair[1].started_at)
        );
        let backup = items.iter().find(|item| item.id == execution).unwrap();
        assert_eq!(backup.kind, OperationKind::Backup);
        assert_eq!(backup.engine, Engine::Mysql);
        assert_eq!(backup.status, OperationStatus::Cancelled);
        assert_eq!(backup.error_code, Some("cancelled"));
        let running = items.iter().find(|item| item.id == restore).unwrap();
        assert_eq!(running.status, OperationStatus::Running);
        assert_eq!(running.destination, "loja_copia");
        assert!(
            !serde_json::to_string(&items)
                .unwrap()
                .contains("DBU_MYSQL_PASSWORD")
        );

        assert_eq!(history_items(&history, 1).unwrap().len(), 1);
    }
}
