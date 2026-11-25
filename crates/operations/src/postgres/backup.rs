use std::path::PathBuf;
use std::time::{Duration, Instant};

use database_backup_adapters::postgres::{
    PgTools, RunnerChoice, Timeouts, ToolError, test_connection,
};
use database_backup_core::{DatabaseEngine, Password, PostgresConnectionConfig};
use database_backup_history::{ExecutionStatus, History, HistoryError, NewArtifact, NewConnection};
use database_backup_storage::{
    ArtifactMetadata, Encoding, LocalTarget, StorageError, StoredArtifact, backup_file_name,
    write_artifact_metadata,
};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tracing::info;

use crate::progress::CountingReader;
use crate::{
    Cancellation, ErrorCode, OperationError, Progress, ProgressSink, Stage, TOOL_NAME, commit,
    describe, open_target,
};

const FORMAT: &str = "pg_dump-custom";

#[derive(Debug)]
pub struct BackupRequest {
    pub config: PostgresConnectionConfig,
    pub password: Password,
    pub secret_ref: String,
    pub output_dir: Option<PathBuf>,
    pub runner: RunnerChoice,
    pub timeout: Option<Duration>,
    pub encoding: Encoding,
}

#[derive(Debug)]
pub struct BackupOutcome {
    pub execution_id: String,
    pub database: String,
    pub artifact: StoredArtifact,
    pub elapsed: Duration,
}

pub struct PreparedBackup {
    request: BackupRequest,
    tools: PgTools,
    server_version: String,
    history: History,
    target: LocalTarget,
}

enum Failure {
    Canceled,
    Error(String),
}

pub async fn prepare(request: BackupRequest) -> Result<PreparedBackup, OperationError> {
    let info = test_connection(&request.config, &request.password, Timeouts::default())
        .await
        .map_err(|error| OperationError::connection(&error))?;
    let tools = super::detect_tools(request.runner, &info.server_version).await?;
    let history_path = History::default_path().map_err(|error| OperationError::internal(&error))?;
    let history = History::open(&history_path).map_err(|error| OperationError::internal(&error))?;
    let target = open_target(request.output_dir.clone(), &history_path)
        .await
        .map_err(|error| OperationError::internal(&error))?;
    Ok(PreparedBackup {
        request,
        tools,
        server_version: info.server_version,
        history,
        target,
    })
}

impl PreparedBackup {
    pub async fn run(
        self,
        cancellation: &Cancellation,
        progress: &dyn ProgressSink,
    ) -> Result<BackupOutcome, OperationError> {
        let execution_id = start_execution(
            &self.history,
            &self.request.config,
            &self.request.secret_ref,
        )
        .map_err(|error| OperationError::internal(&error))?;
        info!(
            operation = "backup.run",
            execution_id = %execution_id,
            source = %self.request.config,
            runner = self.tools.kind().as_str(),
            status = "running",
            "backup iniciado"
        );
        let started = Instant::now();
        let job = Job {
            request: &self.request,
            tools: &self.tools,
            target: &self.target,
            server_version: &self.server_version,
            execution_id: &execution_id,
        };
        match job.execute(cancellation, progress).await {
            Ok(artifact) => {
                record_success(&self.history, &execution_id, &artifact, &self.tools).map_err(
                    |error| {
                        OperationError::new(
                            ErrorCode::Internal,
                            format!(
                                "backup salvo em {}, mas o histórico não foi atualizado: {}",
                                artifact.path.display(),
                                describe(&error)
                            ),
                        )
                    },
                )?;
                let elapsed = started.elapsed();
                info!(
                    operation = "backup.run",
                    execution_id = %execution_id,
                    status = "succeeded",
                    duration_ms = elapsed.as_millis() as u64,
                    size_bytes = artifact.size_bytes,
                    "backup concluído"
                );
                progress.report(Progress::Stage(Stage::Published));
                Ok(BackupOutcome {
                    database: self.request.config.database().to_owned(),
                    execution_id,
                    artifact,
                    elapsed,
                })
            }
            Err(Failure::Canceled) => {
                let message = "backup cancelado pelo usuário";
                let _ = self.history.finish_execution(
                    &execution_id,
                    ExecutionStatus::Canceled,
                    Some(message),
                );
                info!(operation = "backup.run", execution_id = %execution_id, status = "canceled", "backup cancelado");
                Err(OperationError::new(ErrorCode::Canceled, message))
            }
            Err(Failure::Error(message)) => {
                let _ = self.history.finish_execution(
                    &execution_id,
                    ExecutionStatus::Failed,
                    Some(&message),
                );
                info!(operation = "backup.run", execution_id = %execution_id, status = "failed", "backup falhou");
                Err(OperationError::new(ErrorCode::BackupFailed, message))
            }
        }
    }
}

struct Job<'a> {
    request: &'a BackupRequest,
    tools: &'a PgTools,
    target: &'a LocalTarget,
    server_version: &'a str,
    execution_id: &'a str,
}

impl Job<'_> {
    async fn execute(
        &self,
        cancellation: &Cancellation,
        progress: &dyn ProgressSink,
    ) -> Result<StoredArtifact, Failure> {
        let BackupRequest {
            config,
            password,
            timeout,
            encoding,
            ..
        } = self.request;
        let timeout = *timeout;
        progress.report(Progress::Stage(Stage::ConnectionValidated));
        let file_name = backup_file_name(config.database(), OffsetDateTime::now_utc(), "dump");
        let mut pending = self
            .target
            .begin(&file_name)
            .await
            .map_err(storage_failure)?;
        let mut process = self.tools.dump(config, password).map_err(tool_failure)?;
        let stdout = process
            .take_stdout()
            .ok_or_else(|| Failure::Error("pg_dump não forneceu a saída".to_owned()))?;
        progress.report(Progress::Stage(Stage::Dump));
        let mut reader = CountingReader::new(stdout, progress);

        enum Copy {
            Done(Result<u64, StorageError>),
            Interrupted,
            TimedOut,
        }

        let deadline = async {
            match timeout {
                Some(limit) => tokio::time::sleep(limit).await,
                None => std::future::pending().await,
            }
        };
        let copy = tokio::select! {
            result = pending.copy_from(&mut reader) => Copy::Done(result),
            () = cancellation.canceled() => Copy::Interrupted,
            () = deadline => Copy::TimedOut,
        };
        drop(reader);

        match copy {
            Copy::Done(Ok(_)) => {}
            Copy::Done(Err(error)) => {
                process.cancel().await;
                return Err(storage_failure(error));
            }
            Copy::Interrupted => {
                process.cancel().await;
                return Err(Failure::Canceled);
            }
            Copy::TimedOut => {
                process.cancel().await;
                return Err(Failure::Error(format!(
                    "tempo esgotado após {}s",
                    timeout.map_or(0, |limit| limit.as_secs())
                )));
            }
        }

        process.wait().await.map_err(tool_failure)?;
        pending.finish_writing().await.map_err(storage_failure)?;
        self.tools
            .restore_list(pending.partial_path())
            .await
            .map_err(|error| {
                Failure::Error(format!("o arquivo gerado não é um archive válido: {error}"))
            })?;
        if encoding.encryption().is_some() {
            progress.report(Progress::Stage(Stage::Encryption));
        }
        let artifact = commit(pending, encoding, cancellation)
            .await
            .map_err(storage_failure)?;

        progress.report(Progress::Stage(Stage::Checksum));
        let created_at = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_default();
        let pg_dump_version = self.tools.version().to_string();
        let metadata = ArtifactMetadata {
            tool_name: TOOL_NAME,
            tool_version: env!("CARGO_PKG_VERSION"),
            engine: DatabaseEngine::Postgres.as_str(),
            format: FORMAT,
            sha256: &artifact.sha256,
            size_bytes: artifact.size_bytes,
            database: config.database(),
            host: Some(config.host()),
            port: Some(config.port()),
            server_version: self.server_version,
            pg_dump_version: Some(&pg_dump_version),
            mysqldump_version: None,
            mongodump_version: None,
            non_transactional_tables: None,
            topology: None,
            runner: Some(self.tools.kind().as_str()),
            created_at: &created_at,
            execution_id: self.execution_id,
        };
        if let Err(error) = write_artifact_metadata(&artifact, &metadata).await {
            let _ = std::fs::remove_file(&artifact.path);
            return Err(storage_failure(error));
        }
        Ok(artifact)
    }
}

fn start_execution(
    history: &History,
    config: &PostgresConnectionConfig,
    secret_ref: &str,
) -> Result<String, HistoryError> {
    let display_name = config.to_string();
    let connection_id = history.upsert_connection(&NewConnection {
        display_name: &display_name,
        engine: DatabaseEngine::Postgres.as_str(),
        host: Some(config.host()),
        port: Some(config.port()),
        database_name: config.database(),
        username: Some(config.user()),
        secret_ref: Some(secret_ref),
    })?;
    let job_id = history.create_job(&connection_id, &format!("{}-full", config.database()))?;
    history.start_execution(&job_id)
}

fn record_success(
    history: &History,
    execution_id: &str,
    artifact: &StoredArtifact,
    tools: &PgTools,
) -> Result<(), HistoryError> {
    let location = artifact.path.display().to_string();
    let tool_version = format!("pg_dump {} ({})", tools.version(), tools.kind().as_str());
    history.add_artifact(
        execution_id,
        &NewArtifact {
            location: &location,
            format: FORMAT,
            size_bytes: artifact.size_bytes,
            checksum_sha256: Some(&artifact.sha256),
            tool_version: Some(&tool_version),
        },
    )?;
    history.finish_execution(execution_id, ExecutionStatus::Succeeded, None)
}

fn tool_failure(error: ToolError) -> Failure {
    Failure::Error(error.to_string())
}

fn storage_failure(error: StorageError) -> Failure {
    match error {
        StorageError::Canceled => Failure::Canceled,
        error => Failure::Error(describe(&error)),
    }
}
