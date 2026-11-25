use std::fs::OpenOptions;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use database_backup_adapters::sqlite::{
    self as adapter, DEFAULT_BUSY_LIMIT, DatabaseInfo, SqliteError,
};
use database_backup_core::DatabaseEngine;
use database_backup_history::{
    ExecutionStatus, History, HistoryError, NewArtifact, NewConnection, NewRestore, RestoreMode,
};
use database_backup_storage::{
    ArtifactMetadata, Encoding, LocalTarget, StorageError, StoredArtifact, backup_file_name,
    check_restore_source, sync_dir, write_artifact_metadata,
};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tracing::{info, warn};

use crate::archive::{VerifiedSource, verify_checksum};
use crate::{
    Cancellation, ErrorCode, OperationError, Progress, ProgressSink, Stage, TOOL_NAME, commit,
    describe, open_target, timestamp,
};

const ENGINE: DatabaseEngine = DatabaseEngine::Sqlite;
const FORMAT: &str = "sqlite-backup-api";
const EXTENSION: &str = "sqlite3";
const PARTIAL_SUFFIX: &str = ".partial";
const IN_USE_SUFFIXES: [&str; 3] = ["-wal", "-shm", "-journal"];
const WAL_SUFFIXES: [&str; 2] = ["-wal", "-shm"];
const REPORT_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Debug)]
pub struct BackupRequest {
    pub source: PathBuf,
    pub output_dir: Option<PathBuf>,
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
    history: History,
    target: LocalTarget,
}

#[derive(Debug)]
pub struct RestoreRequest {
    pub file: PathBuf,
    pub source: PathBuf,
    pub target: PathBuf,
    pub overwrite: bool,
    pub allow_unverified: bool,
    pub timeout: Option<Duration>,
}

#[derive(Debug)]
pub struct RestoreOutcome {
    pub restore_id: String,
    pub database: String,
    pub mode: RestoreMode,
    pub previous_database: Option<String>,
    pub objects: usize,
    pub checksum_verified: bool,
    pub elapsed: Duration,
}

pub struct PreparedRestore {
    request: RestoreRequest,
    target: PathBuf,
    archive: DatabaseInfo,
    verified: VerifiedSource,
    history: History,
    mode: RestoreMode,
}

struct Control<'a> {
    cancellation: &'a Cancellation,
    progress: &'a dyn ProgressSink,
    timeout: Option<Duration>,
}

enum Copied {
    Done,
    Canceled,
    TimedOut(Duration),
    Failed(String),
}

enum Failure {
    Canceled,
    Error(String),
}

enum Outcome {
    Succeeded { previous: Option<String> },
    Canceled,
    Failed(String),
}

struct ClaimedPartial {
    path: PathBuf,
    _lock: std::fs::File,
}

impl Drop for ClaimedPartial {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub async fn inspect(file: &Path) -> Result<DatabaseInfo, OperationError> {
    let path = file.to_owned();
    tokio::task::spawn_blocking(move || adapter::inspect_backup(&path))
        .await
        .map_err(|error| OperationError::internal(&error))?
        .map_err(|error| {
            let code = match error {
                SqliteError::NotFound(_) => ErrorCode::BackupMissing,
                SqliteError::Integrity { .. } | SqliteError::Sqlite { .. } => {
                    ErrorCode::BackupCorrupted
                }
                _ => ErrorCode::BackupIncompatible,
            };
            OperationError::from_source(code, &error)
        })
}

pub async fn prepare_backup(mut request: BackupRequest) -> Result<PreparedBackup, OperationError> {
    let source = std::path::absolute(&request.source)
        .map_err(|error| OperationError::from_source(ErrorCode::InvalidInput, &error))?;
    let checked = source.clone();
    tokio::task::spawn_blocking(move || adapter::check_database_file(&checked))
        .await
        .map_err(|error| OperationError::internal(&error))?
        .map_err(|error| source_failure(&error))?;

    let history_path = History::default_path().map_err(|error| OperationError::internal(&error))?;
    if same_file(&source, &history_path) {
        return Err(OperationError::new(
            ErrorCode::InvalidInput,
            format!(
                "{} é o histórico interno do utilitário, não um banco de origem",
                source.display()
            ),
        ));
    }
    let history = History::open(&history_path).map_err(|error| OperationError::internal(&error))?;
    let target = open_target(request.output_dir.clone(), &history_path)
        .await
        .map_err(|error| OperationError::internal(&error))?;
    if is_inside(&source, target.dir()) {
        return Err(OperationError::new(
            ErrorCode::InvalidInput,
            format!(
                "a origem {} está dentro do diretório de backups {}; use outro --output-dir",
                source.display(),
                target.dir().display()
            ),
        ));
    }
    request.source = source;
    Ok(PreparedBackup {
        request,
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
        let source = &self.request.source;
        let location = source.display().to_string();
        let execution_id = start_execution(&self.history, source, &location)
            .map_err(|error| OperationError::internal(&error))?;
        info!(
            operation = "backup.run",
            execution_id = %execution_id,
            engine = ENGINE.as_str(),
            source = %location,
            status = "running",
            "backup iniciado"
        );
        progress.report(Progress::Stage(Stage::ConnectionValidated));
        let started = Instant::now();
        let auxiliary_before = existing_files(source, &WAL_SUFFIXES);
        let outcome = execute_backup(
            source,
            &self.target,
            &execution_id,
            &self.request.encoding,
            &Control {
                cancellation,
                progress,
                timeout: self.request.timeout,
            },
        )
        .await;
        warn_created_auxiliary(source, &auxiliary_before);

        match outcome {
            Ok(artifact) => {
                if let Err(error) = record_success(&self.history, &execution_id, &artifact) {
                    return Err(OperationError::new(
                        ErrorCode::Internal,
                        format!(
                            "backup salvo em {}, mas o histórico não foi atualizado: {}",
                            artifact.path.display(),
                            describe(&error)
                        ),
                    ));
                }
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
                    execution_id,
                    database: location,
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

async fn execute_backup(
    source: &Path,
    target: &LocalTarget,
    execution_id: &str,
    encoding: &Encoding,
    control: &Control<'_>,
) -> Result<StoredArtifact, Failure> {
    let file_name = backup_file_name(&source_stem(source), OffsetDateTime::now_utc(), EXTENSION);
    let mut pending = target.begin(&file_name).await.map_err(storage_failure)?;
    control.progress.report(Progress::Stage(Stage::Dump));
    match copy(source, pending.partial_path(), control).await {
        Copied::Done => {}
        Copied::Canceled => return Err(Failure::Canceled),
        Copied::TimedOut(limit) => return Err(Failure::Error(timed_out(limit))),
        Copied::Failed(message) => return Err(Failure::Error(message)),
    }
    pending
        .adopt_written_file()
        .await
        .map_err(storage_failure)?;
    inspect(pending.partial_path()).await.map_err(|error| {
        Failure::Error(format!("a cópia gerada não passou na verificação: {error}"))
    })?;
    if control.cancellation.is_canceled() {
        return Err(Failure::Canceled);
    }
    if encoding.compression().is_some() {
        control.progress.report(Progress::Stage(Stage::Compression));
    }
    if encoding.encryption().is_some() {
        control.progress.report(Progress::Stage(Stage::Encryption));
    }
    let artifact = commit(pending, encoding, control.cancellation)
        .await
        .map_err(storage_failure)?;

    control.progress.report(Progress::Stage(Stage::Checksum));
    let created_at = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default();
    let database = source
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let metadata = ArtifactMetadata {
        tool_name: TOOL_NAME,
        tool_version: env!("CARGO_PKG_VERSION"),
        engine: ENGINE.as_str(),
        format: FORMAT,
        sha256: &artifact.sha256,
        size_bytes: artifact.size_bytes,
        database: &database,
        host: None,
        port: None,
        server_version: adapter::library_version(),
        pg_dump_version: None,
        mysqldump_version: None,
        mongodump_version: None,
        non_transactional_tables: None,
        topology: None,
        runner: None,
        created_at: &created_at,
        execution_id,
    };
    if let Err(error) = write_artifact_metadata(&artifact, &metadata).await {
        let _ = std::fs::remove_file(&artifact.path);
        return Err(storage_failure(error));
    }
    Ok(artifact)
}

pub async fn prepare_restore(request: RestoreRequest) -> Result<PreparedRestore, OperationError> {
    check_restore_source(&request.file)
        .await
        .map_err(|error| OperationError::source_file(&error))?;
    let target = std::path::absolute(&request.target)
        .map_err(|error| OperationError::from_source(ErrorCode::InvalidInput, &error))?;
    let history_path = History::default_path().map_err(|error| OperationError::internal(&error))?;
    if same_file(&target, &history_path) || same_file(&target, &request.file) {
        return Err(OperationError::new(
            ErrorCode::InvalidInput,
            format!(
                "{} é o histórico interno do utilitário ou o próprio backup; escolha outro destino",
                target.display()
            ),
        ));
    }
    let history = History::open(&history_path).map_err(|error| OperationError::internal(&error))?;
    let verified = verify_checksum(
        &request.file,
        &request.source,
        &history,
        request.allow_unverified,
        ENGINE,
    )
    .await?;
    let archive = inspect(&request.file).await?;
    let mode = plan_target(&target, request.overwrite)?;
    if mode == RestoreMode::Overwrite {
        ensure_not_in_use(&target)
            .await
            .map_err(|message| OperationError::new(ErrorCode::RestoreFailed, message))?;
    }
    Ok(PreparedRestore {
        request,
        target,
        archive,
        verified,
        history,
        mode,
    })
}

impl PreparedRestore {
    #[must_use]
    pub fn mode(&self) -> RestoreMode {
        self.mode
    }

    pub async fn run(
        self,
        confirmation: Option<&str>,
        cancellation: &Cancellation,
    ) -> Result<RestoreOutcome, OperationError> {
        let mode = plan_target(&self.target, self.request.overwrite)?;
        let name = self.request.target.display().to_string();
        if mode == RestoreMode::Overwrite && confirmation != Some(name.as_str()) {
            return Err(OperationError::new(
                ErrorCode::ConfirmationRequired,
                format!("confirmação não confere com {name}; nada foi alterado"),
            ));
        }
        let source_location = self.request.source.display().to_string();
        let target_location = self.target.display().to_string();
        let restore_id = self
            .history
            .start_restore(&NewRestore {
                artifact_id: self.verified.artifact_id.as_deref(),
                source_location: &source_location,
                source_sha256: Some(&self.verified.sha256),
                checksum_verified: self.verified.matched_reference,
                engine: ENGINE.as_str(),
                target_host: None,
                target_port: None,
                target_database: &target_location,
                mode,
            })
            .map_err(|error| OperationError::internal(&error))?;
        info!(
            operation = "restore.run",
            restore_id = %restore_id,
            engine = ENGINE.as_str(),
            target = %target_location,
            mode = mode.as_str(),
            status = "running",
            "restore iniciado"
        );
        let started = Instant::now();
        let outcome = execute_restore(
            &self.request.file,
            &self.target,
            mode,
            self.archive.tables,
            &Control {
                cancellation,
                progress: &|_: Progress| {},
                timeout: self.request.timeout,
            },
        )
        .await;
        match outcome {
            Outcome::Succeeded { previous } => {
                let _ = self.history.finish_restore(
                    &restore_id,
                    ExecutionStatus::Succeeded,
                    previous.as_deref(),
                    None,
                );
                let elapsed = started.elapsed();
                info!(
                    operation = "restore.run",
                    restore_id = %restore_id,
                    status = "succeeded",
                    duration_ms = elapsed.as_millis() as u64,
                    "restore concluído"
                );
                Ok(RestoreOutcome {
                    restore_id,
                    database: target_location,
                    mode,
                    previous_database: previous,
                    objects: self.archive.objects,
                    checksum_verified: self.verified.matched_reference,
                    elapsed,
                })
            }
            Outcome::Canceled => {
                let message = "restore cancelado; o destino não foi alterado";
                let _ = self.history.finish_restore(
                    &restore_id,
                    ExecutionStatus::Canceled,
                    None,
                    Some(message),
                );
                info!(operation = "restore.run", restore_id = %restore_id, status = "canceled", "restore cancelado");
                Err(OperationError::new(ErrorCode::Canceled, message))
            }
            Outcome::Failed(message) => {
                let _ = self.history.finish_restore(
                    &restore_id,
                    ExecutionStatus::Failed,
                    None,
                    Some(&message),
                );
                info!(operation = "restore.run", restore_id = %restore_id, status = "failed", "restore falhou");
                Err(OperationError::new(ErrorCode::RestoreFailed, message))
            }
        }
    }
}

fn plan_target(target: &Path, overwrite: bool) -> Result<RestoreMode, OperationError> {
    if !target.parent().is_some_and(Path::is_dir) {
        return Err(OperationError::new(
            ErrorCode::InvalidInput,
            format!("o diretório de destino de {} não existe", target.display()),
        ));
    }
    let metadata = match std::fs::symlink_metadata(target) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(RestoreMode::Create),
        Err(error) => {
            return Err(OperationError::new(
                ErrorCode::RestoreFailed,
                format!("não foi possível ler {}: {error}", target.display()),
            ));
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(OperationError::new(
            ErrorCode::InvalidInput,
            format!(
                "o destino {} não é um arquivo regular (link simbólico ou diretório)",
                target.display()
            ),
        ));
    }
    if let Some(in_use) = existing_files(target, &IN_USE_SUFFIXES).first() {
        return Err(OperationError::new(
            ErrorCode::RestoreFailed,
            format!(
                "o destino {} parece estar em uso (existe {}); feche quem usa o banco e tente de novo",
                target.display(),
                in_use.display()
            ),
        ));
    }
    if !overwrite {
        return Err(OperationError::new(
            ErrorCode::ConfirmationRequired,
            format!(
                "o arquivo {} já existe; use --overwrite para substituí-lo (o atual será mantido como {}.previous-<data>)",
                target.display(),
                target.display()
            ),
        ));
    }
    Ok(RestoreMode::Overwrite)
}

async fn execute_restore(
    file: &Path,
    target: &Path,
    mode: RestoreMode,
    expected_tables: usize,
    control: &Control<'_>,
) -> Outcome {
    let partial = with_suffix(target, PARTIAL_SUFFIX);
    let _claimed = match claim_partial(&partial) {
        Ok(lock) => ClaimedPartial {
            path: partial.clone(),
            _lock: lock,
        },
        Err(message) => return Outcome::Failed(message),
    };

    match copy(file, &partial, control).await {
        Copied::Done => {}
        Copied::Canceled => return Outcome::Canceled,
        Copied::TimedOut(limit) => return Outcome::Failed(timed_out(limit)),
        Copied::Failed(message) => return Outcome::Failed(message),
    }
    if let Err(error) = sync_file(&partial).await {
        return Outcome::Failed(describe(&error));
    }
    match inspect(&partial).await {
        Ok(info) if info.tables == expected_tables => {}
        Ok(info) => {
            return Outcome::Failed(format!(
                "o restore gerou {} tabela(s), mas o backup tem {expected_tables}",
                info.tables
            ));
        }
        Err(error) => return Outcome::Failed(error.to_string()),
    }
    if control.cancellation.is_canceled() {
        return Outcome::Canceled;
    }

    let previous = match mode {
        RestoreMode::Create => {
            if let Err(error) = std::fs::hard_link(&partial, target) {
                return Outcome::Failed(if error.kind() == io::ErrorKind::AlreadyExists {
                    format!(
                        "{} foi criado por outro processo durante o restore; nada foi alterado",
                        target.display()
                    )
                } else {
                    format!("não foi possível criar {}: {error}", target.display())
                });
            }
            None
        }
        RestoreMode::Overwrite => {
            if let Err(message) = ensure_not_in_use(target).await {
                return Outcome::Failed(message);
            }
            let previous = with_suffix(target, &format!(".previous-{}", timestamp()));
            if let Err(error) = std::fs::hard_link(target, &previous) {
                return Outcome::Failed(format!(
                    "não foi possível preservar o arquivo atual em {}: {error}; {} não foi alterado",
                    previous.display(),
                    target.display()
                ));
            }
            if let Err(error) = std::fs::rename(&partial, target) {
                let _ = std::fs::remove_file(&previous);
                return Outcome::Failed(format!(
                    "não foi possível substituir {}: {error}; o arquivo não foi alterado",
                    target.display()
                ));
            }
            Some(previous.display().to_string())
        }
    };
    if let Err(error) = sync_dir(target).await {
        return Outcome::Failed(describe(&error));
    }
    Outcome::Succeeded { previous }
}

async fn copy(source: &Path, destination: &Path, control: &Control<'_>) -> Copied {
    let cancel = Arc::new(AtomicBool::new(false));
    let copied = Arc::new(AtomicU64::new(0));
    let (source, destination) = (source.to_owned(), destination.to_owned());
    let (flag, counter) = (Arc::clone(&cancel), Arc::clone(&copied));
    let mut task = tokio::task::spawn_blocking(move || {
        adapter::copy_database_with_progress(
            &source,
            &destination,
            &flag,
            DEFAULT_BUSY_LIMIT,
            &counter,
        )
    });
    let limit = control.timeout;
    let deadline = async {
        match limit {
            Some(limit) => tokio::time::sleep(limit).await,
            None => std::future::pending().await,
        }
    };
    tokio::pin!(deadline);
    let mut ticker = tokio::time::interval(REPORT_INTERVAL);
    let mut reported = 0;
    let interrupted = loop {
        tokio::select! {
            joined = &mut task => {
                report_bytes(control.progress, &copied, &mut reported);
                return match joined {
                    Ok(Ok(())) => Copied::Done,
                    Ok(Err(SqliteError::Canceled)) => Copied::Canceled,
                    Ok(Err(error)) => Copied::Failed(describe(&error)),
                    Err(error) => Copied::Failed(error.to_string()),
                };
            }
            () = control.cancellation.canceled() => break true,
            () = &mut deadline => break false,
            _ = ticker.tick() => report_bytes(control.progress, &copied, &mut reported),
        }
    };
    cancel.store(true, Ordering::Relaxed);
    let _ = task.await;
    match limit {
        Some(limit) if !interrupted => Copied::TimedOut(limit),
        _ => Copied::Canceled,
    }
}

fn report_bytes(progress: &dyn ProgressSink, copied: &AtomicU64, reported: &mut u64) {
    let total = copied.load(Ordering::Relaxed);
    if total > *reported {
        *reported = total;
        progress.report(Progress::BytesWritten(total));
    }
}

fn start_execution(
    history: &History,
    source: &Path,
    location: &str,
) -> Result<String, HistoryError> {
    let display_name = format!("{}:{location}", ENGINE.as_str());
    let connection_id = history.upsert_connection(&NewConnection {
        display_name: &display_name,
        engine: ENGINE.as_str(),
        host: None,
        port: None,
        database_name: location,
        username: None,
        secret_ref: None,
    })?;
    let job_id = history.create_job(&connection_id, &format!("{}-full", source_stem(source)))?;
    history.start_execution(&job_id)
}

fn record_success(
    history: &History,
    execution_id: &str,
    artifact: &StoredArtifact,
) -> Result<(), HistoryError> {
    let location = artifact.path.display().to_string();
    let tool_version = format!("SQLite {}", adapter::library_version());
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

fn source_failure(error: &SqliteError) -> OperationError {
    let code = match error {
        SqliteError::NotFound(_) => ErrorCode::DatabaseNotFound,
        _ => ErrorCode::InvalidInput,
    };
    OperationError::from_source(code, error)
}

fn storage_failure(error: StorageError) -> Failure {
    match error {
        StorageError::Canceled => Failure::Canceled,
        error => Failure::Error(describe(&error)),
    }
}

fn timed_out(limit: Duration) -> String {
    format!("tempo esgotado após {}s", limit.as_secs())
}

fn source_stem(source: &Path) -> String {
    source
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| ENGINE.as_str().to_owned())
}

fn same_file(left: &Path, right: &Path) -> bool {
    matches!(
        (left.canonicalize(), right.canonicalize()),
        (Ok(left), Ok(right)) if left == right
    )
}

fn is_inside(path: &Path, dir: &Path) -> bool {
    matches!(
        (path.canonicalize(), dir.canonicalize()),
        (Ok(path), Ok(dir)) if path.starts_with(&dir)
    )
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn claim_partial(path: &Path) -> Result<std::fs::File, String> {
    let cannot_create =
        |error: io::Error| format!("não foi possível criar {}: {error}", path.display());
    match create_locked(path) {
        Ok(file) => return Ok(file),
        Err(error) if error.kind() != io::ErrorKind::AlreadyExists => {
            return Err(cannot_create(error));
        }
        Err(_) => {}
    }
    let existing = std::fs::File::open(path).map_err(cannot_create)?;
    match existing.try_lock() {
        Ok(()) => {
            std::fs::remove_file(path).map_err(cannot_create)?;
            warn!(
                operation = "restore.run",
                file = %path.display(),
                "removido arquivo .partial de um restore interrompido"
            );
        }
        Err(std::fs::TryLockError::WouldBlock) => {
            return Err(format!(
                "{} está em uso por outro restore em andamento",
                path.display()
            ));
        }
        Err(std::fs::TryLockError::Error(error)) => return Err(cannot_create(error)),
    }
    drop(existing);
    create_locked(path).map_err(cannot_create)
}

fn create_locked(path: &Path) -> io::Result<std::fs::File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    file.try_lock().map_err(io::Error::from)?;
    Ok(file)
}

async fn ensure_not_in_use(target: &Path) -> Result<(), String> {
    let path = target.to_owned();
    tokio::task::spawn_blocking(move || adapter::check_not_in_use(&path))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| describe(&error))
}

fn existing_files(base: &Path, suffixes: &[&str]) -> Vec<PathBuf> {
    suffixes
        .iter()
        .map(|suffix| with_suffix(base, suffix))
        .filter(|path| path.exists())
        .collect()
}

fn warn_created_auxiliary(source: &Path, before: &[PathBuf]) {
    let created: Vec<String> = existing_files(source, &WAL_SUFFIXES)
        .into_iter()
        .filter(|path| !before.contains(path))
        .map(|path| path.display().to_string())
        .collect();
    if !created.is_empty() {
        warn!(
            operation = "backup.run",
            files = %created.join(", "),
            "o SQLite criou estes arquivos ao lado da origem (normal em modo WAL); os dados não mudaram e eles somem quando a aplicação fechar o banco"
        );
    }
}

async fn sync_file(path: &Path) -> Result<(), StorageError> {
    let io_error = |source| StorageError::Io {
        path: path.to_owned(),
        source,
    };
    let file = tokio::fs::File::open(path).await.map_err(io_error)?;
    file.sync_all().await.map_err(io_error)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use rusqlite::Connection;
    use tempfile::TempDir;

    use super::*;

    const ROWS: i64 = 500;

    fn database(dir: &Path, name: &str, rows: i64) -> PathBuf {
        let path = dir.join(name);
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE notes (id INTEGER PRIMARY KEY, body BLOB)")
            .unwrap();
        conn.execute(
            "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < ?1)
             INSERT INTO notes (body) SELECT randomblob(4000) FROM n",
            [rows],
        )
        .unwrap();
        path
    }

    fn rows(path: &Path) -> i64 {
        Connection::open(path)
            .unwrap()
            .query_row("SELECT count(*) FROM notes", [], |row| row.get(0))
            .unwrap()
    }

    fn files(dir: &Path) -> usize {
        std::fs::read_dir(dir).unwrap().count()
    }

    async fn run_backup(
        source: &Path,
        output: &Path,
        cancellation: &Cancellation,
        progress: &dyn ProgressSink,
    ) -> Result<StoredArtifact, Failure> {
        let target = LocalTarget::new(output.to_owned()).await.unwrap();
        execute_backup(
            source,
            &target,
            "execution",
            &Encoding::default(),
            &Control {
                cancellation,
                progress,
                timeout: None,
            },
        )
        .await
    }

    fn prepared_restore(dir: &Path, file: &Path, target: &Path, tables: usize) -> PreparedRestore {
        PreparedRestore {
            request: RestoreRequest {
                file: file.to_owned(),
                source: file.to_owned(),
                target: target.to_owned(),
                overwrite: true,
                allow_unverified: true,
                timeout: None,
            },
            target: target.to_owned(),
            archive: DatabaseInfo {
                objects: tables,
                tables,
            },
            verified: VerifiedSource {
                artifact_id: None,
                sha256: "0".repeat(64),
                matched_reference: false,
                source_version: None,
                manifest: None,
            },
            history: History::open(&dir.join("history.sqlite3")).unwrap(),
            mode: RestoreMode::Overwrite,
        }
    }

    #[tokio::test]
    async fn backup_reports_stages_and_the_copied_bytes() {
        let dir = TempDir::new().unwrap();
        let source = database(dir.path(), "app.db", ROWS);
        let reports = Mutex::new(Vec::new());
        let sink = |progress: Progress| reports.lock().unwrap().push(progress);

        let artifact = run_backup(
            &source,
            &dir.path().join("out"),
            &Cancellation::new(),
            &sink,
        )
        .await
        .ok()
        .unwrap();

        let reports = reports.into_inner().unwrap();
        assert_eq!(reports.first(), Some(&Progress::Stage(Stage::Dump)));
        assert_eq!(reports.last(), Some(&Progress::Stage(Stage::Checksum)));
        let bytes = reports.iter().rev().find_map(|progress| match progress {
            Progress::BytesWritten(total) => Some(*total),
            Progress::Stage(_) => None,
        });
        assert_eq!(bytes, Some(artifact.size_bytes));
        assert_eq!(rows(&artifact.path), ROWS);
    }

    #[tokio::test]
    async fn canceled_backup_leaves_no_file() {
        let dir = TempDir::new().unwrap();
        let source = database(dir.path(), "app.db", ROWS);
        let output = dir.path().join("out");
        let cancellation = Cancellation::new();
        let sink = |progress: Progress| {
            if progress == Progress::Stage(Stage::Dump) {
                cancellation.cancel();
            }
        };

        let result = run_backup(&source, &output, &cancellation, &sink).await;

        assert!(matches!(result, Err(Failure::Canceled)));
        assert_eq!(files(&output), 0);
    }

    #[tokio::test]
    async fn restore_requires_the_exact_target_before_overwriting() {
        let dir = TempDir::new().unwrap();
        let file = database(dir.path(), "backup.sqlite3", 3);
        let target = database(dir.path(), "app.db", 1);

        for confirmation in [None, Some("app.db")] {
            let error = prepared_restore(dir.path(), &file, &target, 1)
                .run(confirmation, &Cancellation::new())
                .await
                .unwrap_err();
            assert_eq!(error.code(), ErrorCode::ConfirmationRequired);
            assert_eq!(rows(&target), 1);
        }

        let outcome = prepared_restore(dir.path(), &file, &target, 1)
            .run(target.to_str(), &Cancellation::new())
            .await
            .unwrap();

        assert_eq!(outcome.mode, RestoreMode::Overwrite);
        assert_eq!(rows(&target), 3);
        assert_eq!(rows(Path::new(&outcome.previous_database.unwrap())), 1);
    }

    #[tokio::test]
    async fn canceled_restore_keeps_the_target() {
        let dir = TempDir::new().unwrap();
        let file = database(dir.path(), "backup.sqlite3", 3);
        let target = database(dir.path(), "app.db", 1);
        let cancellation = Cancellation::new();
        cancellation.cancel();

        let error = prepared_restore(dir.path(), &file, &target, 1)
            .run(target.to_str(), &cancellation)
            .await
            .unwrap_err();

        assert_eq!(error.code(), ErrorCode::Canceled);
        assert_eq!(rows(&target), 1);
        assert!(!with_suffix(&target, PARTIAL_SUFFIX).exists());
    }

    fn assert_send<T: Send>(_: T) {}

    #[allow(dead_code)]
    fn futures_are_send(
        backup: PreparedBackup,
        restore: PreparedRestore,
        backup_request: BackupRequest,
        restore_request: RestoreRequest,
        cancellation: &Cancellation,
        progress: &dyn ProgressSink,
    ) {
        assert_send(prepare_backup(backup_request));
        assert_send(prepare_restore(restore_request));
        assert_send(backup.run(cancellation, progress));
        assert_send(restore.run(None, cancellation));
    }
}
