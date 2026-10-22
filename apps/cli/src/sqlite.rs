use std::fs::OpenOptions;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use database_backup_adapters::sqlite::{
    self as adapter, DEFAULT_BUSY_LIMIT, DatabaseInfo, SqliteError,
};
use database_backup_core::DatabaseEngine;
use database_backup_history::{
    ExecutionStatus, History, HistoryError, NewArtifact, NewConnection, NewRestore, RestoreMode,
};
use database_backup_storage::{
    ArtifactMetadata, LocalTarget, StorageError, StoredArtifact, backup_file_name,
    check_restore_source, sync_dir, write_metadata,
};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tracing::{info, warn};

use crate::archive::verify_checksum;
use crate::backup::{BackupResult, human_size, open_target};
use crate::output::{fail, success};
use crate::restore::{RestoreResult, confirm_overwrite, timestamp};
use crate::{describe, exit};

const ENGINE: DatabaseEngine = DatabaseEngine::Sqlite;
const FORMAT: &str = "sqlite-backup-api";
const EXTENSION: &str = "sqlite3";
const PARTIAL_SUFFIX: &str = ".partial";
const IN_USE_SUFFIXES: [&str; 3] = ["-wal", "-shm", "-journal"];
const WAL_SUFFIXES: [&str; 2] = ["-wal", "-shm"];

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

pub struct RestoreRequest<'a> {
    pub file: &'a Path,
    pub target: &'a Path,
    pub overwrite: bool,
    pub confirm: Option<&'a str>,
    pub allow_unverified: bool,
    pub timeout: Option<u64>,
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

pub async fn inspect(file: &Path) -> Result<DatabaseInfo, String> {
    let path = file.to_owned();
    tokio::task::spawn_blocking(move || adapter::inspect_backup(&path))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| describe(&error))
}

pub async fn backup(source: &Path, output_dir: Option<PathBuf>, timeout: Option<u64>) -> ExitCode {
    let source = match std::path::absolute(source) {
        Ok(path) => path,
        Err(error) => return fail(exit::INVALID_INPUT, &error.to_string()),
    };
    let checked = source.clone();
    match tokio::task::spawn_blocking(move || adapter::check_database_file(&checked)).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return source_failure(&error),
        Err(error) => return fail(exit::FAILURE, &error.to_string()),
    }

    let history_path = match History::default_path() {
        Ok(path) => path,
        Err(error) => return fail(exit::FAILURE, &describe(&error)),
    };
    if same_file(&source, &history_path) {
        return fail(
            exit::INVALID_INPUT,
            &format!(
                "{} é o histórico interno do utilitário, não um banco de origem",
                source.display()
            ),
        );
    }
    let history = match History::open(&history_path) {
        Ok(history) => history,
        Err(error) => return fail(exit::FAILURE, &describe(&error)),
    };
    let target = match open_target(output_dir, &history_path).await {
        Ok(target) => target,
        Err(code) => return code,
    };
    if is_inside(&source, target.dir()) {
        return fail(
            exit::INVALID_INPUT,
            &format!(
                "a origem {} está dentro do diretório de backups {}; use outro --output-dir",
                source.display(),
                target.dir().display()
            ),
        );
    }

    let location = source.display().to_string();
    let execution_id = match start_execution(&history, &source, &location) {
        Ok(id) => id,
        Err(error) => return fail(exit::FAILURE, &describe(&error)),
    };
    info!(
        operation = "backup.run",
        execution_id = %execution_id,
        engine = ENGINE.as_str(),
        source = %location,
        status = "running",
        "backup iniciado"
    );
    let started = Instant::now();
    let auxiliary_before = existing_files(&source, &WAL_SUFFIXES);
    let outcome = execute_backup(&source, &target, &execution_id, timeout).await;
    warn_created_auxiliary(&source, &auxiliary_before);

    match outcome {
        Ok(artifact) => {
            if let Err(error) = record_success(&history, &execution_id, &artifact) {
                return fail(
                    exit::FAILURE,
                    &format!(
                        "backup salvo em {}, mas o histórico não foi atualizado: {}",
                        artifact.path.display(),
                        describe(&error)
                    ),
                );
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
            let text = format!(
                "Backup concluído: {} ({}, sha256 {}…) em {:.1}s",
                artifact.path.display(),
                human_size(artifact.size_bytes),
                &artifact.sha256[..12],
                elapsed.as_secs_f64()
            );
            success(
                &BackupResult {
                    execution_id: &execution_id,
                    database: &location,
                    file: artifact.path.display().to_string(),
                    size_bytes: artifact.size_bytes,
                    sha256: &artifact.sha256,
                    duration_ms: elapsed.as_millis(),
                },
                &text,
            )
        }
        Err(Failure::Canceled) => {
            let message = "backup cancelado pelo usuário";
            let _ =
                history.finish_execution(&execution_id, ExecutionStatus::Canceled, Some(message));
            info!(operation = "backup.run", execution_id = %execution_id, status = "canceled", "backup cancelado");
            fail(exit::CANCELED, message)
        }
        Err(Failure::Error(message)) => {
            let _ =
                history.finish_execution(&execution_id, ExecutionStatus::Failed, Some(&message));
            info!(operation = "backup.run", execution_id = %execution_id, status = "failed", "backup falhou");
            fail(exit::BACKUP_FAILED, &message)
        }
    }
}

async fn execute_backup(
    source: &Path,
    target: &LocalTarget,
    execution_id: &str,
    timeout: Option<u64>,
) -> Result<StoredArtifact, Failure> {
    let file_name = backup_file_name(&source_stem(source), OffsetDateTime::now_utc(), EXTENSION);
    let mut pending = target.begin(&file_name).await.map_err(storage_failure)?;
    match copy(
        source.to_owned(),
        pending.partial_path().to_owned(),
        timeout,
    )
    .await
    {
        Copied::Done => {}
        Copied::Canceled => return Err(Failure::Canceled),
        Copied::TimedOut(limit) => return Err(Failure::Error(timed_out(limit))),
        Copied::Failed(message) => return Err(Failure::Error(message)),
    }
    pending
        .adopt_written_file()
        .await
        .map_err(storage_failure)?;
    inspect(pending.partial_path()).await.map_err(|message| {
        Failure::Error(format!(
            "a cópia gerada não passou na verificação: {message}"
        ))
    })?;
    let artifact = pending.commit().await.map_err(storage_failure)?;

    let created_at = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default();
    let database = source
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let metadata = ArtifactMetadata {
        tool_name: env!("CARGO_BIN_NAME"),
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
        runner: None,
        created_at: &created_at,
        execution_id,
    };
    if let Err(error) = write_metadata(&artifact.path, &metadata).await {
        let _ = std::fs::remove_file(&artifact.path);
        return Err(storage_failure(error));
    }
    Ok(artifact)
}

pub async fn restore(request: &RestoreRequest<'_>) -> ExitCode {
    let file = match std::path::absolute(request.file) {
        Ok(path) => path,
        Err(error) => return fail(exit::INVALID_ARCHIVE, &error.to_string()),
    };
    if let Err(error) = check_restore_source(&file).await {
        return fail(exit::INVALID_ARCHIVE, &describe(&error));
    }
    let target = match std::path::absolute(request.target) {
        Ok(path) => path,
        Err(error) => return fail(exit::INVALID_INPUT, &error.to_string()),
    };
    let history_path = match History::default_path() {
        Ok(path) => path,
        Err(error) => return fail(exit::FAILURE, &describe(&error)),
    };
    if same_file(&target, &history_path) || same_file(&target, &file) {
        return fail(
            exit::INVALID_INPUT,
            &format!(
                "{} é o histórico interno do utilitário ou o próprio backup; escolha outro destino",
                target.display()
            ),
        );
    }
    let history = match History::open(&history_path) {
        Ok(history) => history,
        Err(error) => return fail(exit::FAILURE, &describe(&error)),
    };
    let verified = match verify_checksum(&file, &history, request.allow_unverified, ENGINE).await {
        Ok(verified) => verified,
        Err(message) => return fail(exit::INVALID_ARCHIVE, &message),
    };
    let archive = match inspect(&file).await {
        Ok(info) => info,
        Err(message) => return fail(exit::INVALID_ARCHIVE, &message),
    };
    let mode = match plan_target(&target, request).await {
        Ok(mode) => mode,
        Err(code) => return code,
    };

    let source_location = file.display().to_string();
    let target_location = target.display().to_string();
    let restore_id = match history.start_restore(&NewRestore {
        artifact_id: verified.artifact_id.as_deref(),
        source_location: &source_location,
        source_sha256: Some(&verified.sha256),
        checksum_verified: verified.matched_reference,
        engine: ENGINE.as_str(),
        target_host: None,
        target_port: None,
        target_database: &target_location,
        mode,
    }) {
        Ok(id) => id,
        Err(error) => return fail(exit::FAILURE, &describe(&error)),
    };
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

    match execute_restore(&file, &target, mode, archive.tables, request.timeout).await {
        Outcome::Succeeded { previous } => {
            let _ = history.finish_restore(
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
            let mut text = format!(
                "Restore concluído em {} ({} objetos) em {:.1}s",
                target.display(),
                archive.objects,
                elapsed.as_secs_f64()
            );
            if let Some(previous) = &previous {
                text.push_str(&format!("\nArquivo anterior mantido como {previous}"));
            }
            success(
                &RestoreResult {
                    restore_id: &restore_id,
                    database: &target_location,
                    mode: mode.as_str(),
                    previous_database: previous.as_deref(),
                    objects: archive.objects,
                    checksum_verified: verified.matched_reference,
                    duration_ms: elapsed.as_millis(),
                },
                &text,
            )
        }
        Outcome::Canceled => {
            let message = "restore cancelado; o destino não foi alterado";
            let _ =
                history.finish_restore(&restore_id, ExecutionStatus::Canceled, None, Some(message));
            info!(operation = "restore.run", restore_id = %restore_id, status = "canceled", "restore cancelado");
            fail(exit::CANCELED, message)
        }
        Outcome::Failed(message) => {
            let _ =
                history.finish_restore(&restore_id, ExecutionStatus::Failed, None, Some(&message));
            info!(operation = "restore.run", restore_id = %restore_id, status = "failed", "restore falhou");
            fail(exit::RESTORE_FAILED, &message)
        }
    }
}

async fn plan_target(target: &Path, request: &RestoreRequest<'_>) -> Result<RestoreMode, ExitCode> {
    if !target.parent().is_some_and(Path::is_dir) {
        return Err(fail(
            exit::INVALID_INPUT,
            &format!("o diretório de destino de {} não existe", target.display()),
        ));
    }
    let metadata = match std::fs::symlink_metadata(target) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(RestoreMode::Create),
        Err(error) => {
            return Err(fail(
                exit::RESTORE_FAILED,
                &format!("não foi possível ler {}: {error}", target.display()),
            ));
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(fail(
            exit::INVALID_INPUT,
            &format!(
                "o destino {} não é um arquivo regular (link simbólico ou diretório)",
                target.display()
            ),
        ));
    }
    if let Some(in_use) = IN_USE_SUFFIXES
        .iter()
        .map(|suffix| with_suffix(target, suffix))
        .find(|path| path.exists())
    {
        return Err(fail(
            exit::RESTORE_FAILED,
            &format!(
                "o destino {} parece estar em uso (existe {}); feche quem usa o banco e tente de novo",
                target.display(),
                in_use.display()
            ),
        ));
    }
    if !request.overwrite {
        return Err(fail(
            exit::CONFIRMATION_REQUIRED,
            &format!(
                "o arquivo {} já existe; use --overwrite para substituí-lo (o atual será mantido como {}.previous-<data>)",
                target.display(),
                target.display()
            ),
        ));
    }
    ensure_not_in_use(target)
        .await
        .map_err(|message| fail(exit::RESTORE_FAILED, &message))?;
    let name = request.target.display().to_string();
    confirm_overwrite(
        &name,
        &format!(
            "O arquivo {name} será substituído; o atual será mantido como cópia.\nDigite o caminho do arquivo para confirmar: "
        ),
        request.confirm,
    )?;
    Ok(RestoreMode::Overwrite)
}

async fn execute_restore(
    file: &Path,
    target: &Path,
    mode: RestoreMode,
    expected_tables: usize,
    timeout: Option<u64>,
) -> Outcome {
    let partial = with_suffix(target, PARTIAL_SUFFIX);
    let _claimed = match claim_partial(&partial) {
        Ok(lock) => ClaimedPartial {
            path: partial.clone(),
            _lock: lock,
        },
        Err(message) => return Outcome::Failed(message),
    };

    match copy(file.to_owned(), partial.clone(), timeout).await {
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
        Err(message) => return Outcome::Failed(message),
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

async fn copy(source: PathBuf, destination: PathBuf, timeout: Option<u64>) -> Copied {
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    let mut task = tokio::task::spawn_blocking(move || {
        adapter::copy_database(&source, &destination, &flag, DEFAULT_BUSY_LIMIT)
    });
    let limit = timeout.map(Duration::from_secs);
    let deadline = async {
        match limit {
            Some(limit) => tokio::time::sleep(limit).await,
            None => std::future::pending().await,
        }
    };
    let interrupted = tokio::select! {
        joined = &mut task => {
            return match joined {
                Ok(Ok(())) => Copied::Done,
                Ok(Err(SqliteError::Canceled)) => Copied::Canceled,
                Ok(Err(error)) => Copied::Failed(describe(&error)),
                Err(error) => Copied::Failed(error.to_string()),
            };
        }
        _ = tokio::signal::ctrl_c() => true,
        () = deadline => false,
    };
    cancel.store(true, Ordering::Relaxed);
    let _ = task.await;
    match limit {
        Some(limit) if !interrupted => Copied::TimedOut(limit),
        _ => Copied::Canceled,
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

fn source_failure(error: &SqliteError) -> ExitCode {
    let code = match error {
        SqliteError::NotFound(_) => exit::DATABASE_NOT_FOUND,
        _ => exit::INVALID_INPUT,
    };
    fail(code, &describe(error))
}

fn storage_failure(error: StorageError) -> Failure {
    Failure::Error(describe(&error))
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
