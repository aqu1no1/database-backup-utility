use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use database_backup_adapters::mysql::{
    Admin, AdminError, ConnectionInfo, DatabaseState, DumpInfo, Flavor, MysqlTools, MysqlVersion,
    RestoreProcess, Timeouts, ToolError, inspect_dump, test_connection,
};
use database_backup_adapters::postgres::RunnerChoice;
use database_backup_core::{
    DatabaseEngine, MysqlConnectionConfig, Password, SecretProvider, TlsMode,
};
use database_backup_history::{
    ExecutionStatus, History, HistoryError, NewArtifact, NewConnection, NewRestore, RestoreMode,
};
use database_backup_storage::{
    ArtifactMetadata, Encoding, LocalTarget, StorageError, StoredArtifact, backup_file_name,
    check_restore_source, write_artifact_metadata,
};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tracing::{info, warn};

use crate::archive::{VerifiedSource, verify_checksum};
use crate::progress::CountingReader;
use crate::{
    Cancellation, ErrorCode, OperationError, Progress, ProgressSink, Stage, TOOL_NAME, commit,
    describe, open_target,
};

const EXTENSION: &str = "sql";
const CATALOG_DATABASE: &str = "information_schema";

#[derive(Debug)]
pub struct BackupRequest {
    pub flavor: Flavor,
    pub config: MysqlConnectionConfig,
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
    pub non_transactional_tables: Option<Vec<String>>,
}

pub struct PreparedBackup {
    request: BackupRequest,
    tools: MysqlTools,
    server: Server,
    history: History,
    target: LocalTarget,
    non_transactional: Option<Vec<String>>,
}

#[derive(Debug)]
pub struct RestoreRequest {
    pub flavor: Flavor,
    pub file: PathBuf,
    pub source: PathBuf,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub tls: TlsMode,

    pub target_database: String,
    pub runner: RunnerChoice,
    pub allow_unverified: bool,
    pub timeout: Option<Duration>,
}

#[derive(Debug)]
pub struct RestoreOutcome {
    pub restore_id: String,
    pub database: String,
    pub objects: usize,
    pub tables: usize,
    pub checksum_verified: bool,
    pub elapsed: Duration,
}

pub struct PreparedRestore {
    request: RestoreRequest,
    target: MysqlConnectionConfig,
    password: Password,
    tools: MysqlTools,
    admin: Admin,
    content: DumpInfo,
    verified: VerifiedSource,
    history: History,
}

struct Destination<'a> {
    target: &'a LocalTarget,
    encoding: &'a Encoding,
}

struct Control<'a> {
    cancellation: &'a Cancellation,
    progress: &'a dyn ProgressSink,
    timeout: Option<Duration>,
}

struct RestoreLimits<'a> {
    cancellation: &'a Cancellation,
    timeout: Option<Duration>,
}

enum Failure {
    Canceled,
    Error(String),
}

enum Outcome {
    Succeeded,
    Canceled,
    Failed(String),
}

struct SourceFacts<'a> {
    server_version: &'a str,
    non_transactional: Option<&'a [String]>,
}

struct Server {
    info: ConnectionInfo,
    version: MysqlVersion,
}

#[must_use]
pub const fn engine(flavor: Flavor) -> DatabaseEngine {
    match flavor {
        Flavor::Mysql => DatabaseEngine::Mysql,
        Flavor::Mariadb => DatabaseEngine::Mariadb,
    }
}

const fn format(flavor: Flavor) -> &'static str {
    match flavor {
        Flavor::Mysql => "mysqldump-sql",
        Flavor::Mariadb => "mariadb-dump-sql",
    }
}

pub async fn prepare_backup(request: BackupRequest) -> Result<PreparedBackup, OperationError> {
    let server = connect_server(request.flavor, &request.config, &request.password).await?;
    let tools = MysqlTools::detect_flavor(request.flavor, request.runner, server.version)
        .await
        .map_err(|error| OperationError::new(ErrorCode::ToolUnavailable, error.to_string()))?;
    let non_transactional =
        check_non_transactional(engine(request.flavor), &request.config, &request.password).await;
    let history_path = History::default_path().map_err(|error| OperationError::internal(&error))?;
    let history = History::open(&history_path).map_err(|error| OperationError::internal(&error))?;
    let target = open_target(request.output_dir.clone(), &history_path)
        .await
        .map_err(|error| OperationError::internal(&error))?;
    Ok(PreparedBackup {
        request,
        tools,
        server,
        history,
        target,
        non_transactional,
    })
}

impl PreparedBackup {
    pub async fn run(
        self,
        cancellation: &Cancellation,
        progress: &dyn ProgressSink,
    ) -> Result<BackupOutcome, OperationError> {
        let engine = engine(self.request.flavor);
        let execution_id = start_execution(
            &self.history,
            engine,
            &self.request.config,
            &self.request.secret_ref,
        )
        .map_err(|error| OperationError::internal(&error))?;

        info!(
            operation = "backup.run",
            engine = engine.as_str(),
            execution_id = %execution_id,
            source = %self.request.config,
            runner = self.tools.kind().as_str(),
            status = "running",
            "backup iniciado"
        );
        progress.report(Progress::Stage(Stage::ConnectionValidated));
        let started = Instant::now();
        let outcome = execute_backup(
            &self.request.config,
            &self.request.password,
            &self.tools,
            &Destination {
                target: &self.target,
                encoding: &self.request.encoding,
            },
            &execution_id,
            &SourceFacts {
                server_version: &self.server.info.server_version,
                non_transactional: self.non_transactional.as_deref(),
            },
            &Control {
                cancellation,
                progress,
                timeout: self.request.timeout,
            },
        )
        .await;
        match outcome {
            Ok(artifact) => {
                if let Err(error) =
                    record_success(&self.history, &execution_id, &artifact, &self.tools)
                {
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
                    engine = engine.as_str(),
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
                    non_transactional_tables: self.non_transactional,
                })
            }
            Err(Failure::Canceled) => {
                let message = "backup cancelado pelo usuário";
                let _ = self.history.finish_execution(
                    &execution_id,
                    ExecutionStatus::Canceled,
                    Some(message),
                );
                info!(operation = "backup.run", engine = engine.as_str(), execution_id = %execution_id, status = "canceled", "backup cancelado");
                Err(OperationError::new(ErrorCode::Canceled, message))
            }
            Err(Failure::Error(message)) => {
                let _ = self.history.finish_execution(
                    &execution_id,
                    ExecutionStatus::Failed,
                    Some(&message),
                );
                info!(operation = "backup.run", engine = engine.as_str(), execution_id = %execution_id, status = "failed", "backup falhou");
                Err(OperationError::new(ErrorCode::BackupFailed, message))
            }
        }
    }
}

async fn execute_backup(
    config: &MysqlConnectionConfig,
    password: &Password,
    tools: &MysqlTools,
    destination: &Destination<'_>,
    execution_id: &str,
    source: &SourceFacts<'_>,
    control: &Control<'_>,
) -> Result<StoredArtifact, Failure> {
    let Destination { target, encoding } = *destination;
    let timeout = control.timeout;
    let file_name = backup_file_name(config.database(), OffsetDateTime::now_utc(), EXTENSION);
    let mut pending = target.begin(&file_name).await.map_err(storage_failure)?;
    let mut process = tools.dump(config, password).map_err(tool_failure)?;
    let stdout = process.take_stdout().ok_or_else(|| {
        Failure::Error(format!(
            "{} não forneceu a saída",
            tools.flavor().dump_tool()
        ))
    })?;

    control.progress.report(Progress::Stage(Stage::Dump));
    let mut reader = CountingReader::new(stdout, control.progress);

    enum Copy {
        Done(Result<u64, StorageError>),
        Interrupted,
        TimedOut,
    }
    let copy = tokio::select! {
        result = pending.copy_from(&mut reader) => Copy::Done(result),
        () = control.cancellation.canceled() => Copy::Interrupted,
        () = deadline(timeout) => Copy::TimedOut,
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
            return Err(Failure::Error(timed_out(timeout)));
        }
    }

    process.wait().await.map_err(tool_failure)?;
    pending.finish_writing().await.map_err(storage_failure)?;
    let dump = inspect(pending.partial_path())
        .await
        .map_err(|error| Failure::Error(format!("o dump gerado foi recusado: {error}")))?;
    if dump.database.as_deref() != Some(config.database()) {
        return Err(Failure::Error(format!(
            "o dump gerado descreve o banco {}, não {}",
            dump.database.as_deref().unwrap_or("desconhecido"),
            config.database()
        )));
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
    let mysqldump_version = tools.version().to_string();
    let metadata = ArtifactMetadata {
        tool_name: TOOL_NAME,
        tool_version: env!("CARGO_PKG_VERSION"),
        engine: engine(tools.flavor()).as_str(),
        format: format(tools.flavor()),
        sha256: &artifact.sha256,
        size_bytes: artifact.size_bytes,
        database: config.database(),
        host: Some(config.host()),
        port: Some(config.port()),
        server_version: source.server_version,
        pg_dump_version: None,
        mysqldump_version: Some(&mysqldump_version),
        mongodump_version: None,
        non_transactional_tables: source.non_transactional,
        topology: None,
        runner: Some(tools.kind().as_str()),
        created_at: &created_at,
        execution_id,
    };
    if let Err(error) = write_artifact_metadata(&artifact, &metadata).await {
        let _ = std::fs::remove_file(&artifact.path);
        return Err(storage_failure(error));
    }
    Ok(artifact)
}

pub async fn prepare_restore(
    request: RestoreRequest,
    secrets: &(dyn SecretProvider + Sync),
) -> Result<PreparedRestore, OperationError> {
    let flavor = request.flavor;
    let engine = engine(flavor);
    check_restore_source(&request.file)
        .await
        .map_err(|error| OperationError::source_file(&error))?;
    let server = MysqlConnectionConfig::new(
        &request.host,
        request.port,
        CATALOG_DATABASE,
        &request.user,
        request.tls,
    )
    .map_err(|error| OperationError::new(ErrorCode::InvalidInput, error.to_string()))?;
    let target = server
        .with_database(&request.target_database)
        .map_err(|error| OperationError::new(ErrorCode::InvalidInput, error.to_string()))?;

    let history = History::default_path()
        .and_then(|path| History::open(&path))
        .map_err(|error| OperationError::from_source(ErrorCode::Internal, &error))?;
    let verified = verify_checksum(
        &request.file,
        &request.source,
        &history,
        request.allow_unverified,
        engine,
    )
    .await?;
    if !verified.matched_reference {
        warn!(
            operation = "restore.run",
            engine = engine.as_str(),
            "checksum não verificado (--allow-unverified); a integridade do arquivo não foi confirmada e o SQL do arquivo será executado com as permissões do usuário"
        );
    }
    let dump = inspect(&request.file).await?;
    if dump.flavor != flavor {
        return Err(OperationError::new(
            ErrorCode::EngineMismatch,
            format!(
                "o arquivo é um dump do {}, não do {flavor}; restaure com --engine {}",
                dump.flavor,
                self::engine(dump.flavor).as_str()
            ),
        ));
    }

    let (password, _) = secrets
        .password(&target)
        .map_err(|error| OperationError::from_source(ErrorCode::InvalidInput, &error))?;
    let connected = connect_server(flavor, &server, &password).await?;
    if let Some(from) = dump.dumped_from
        && from > connected.version
    {
        return Err(OperationError::new(
            ErrorCode::BackupIncompatible,
            format!(
                "o backup veio do {flavor} {from}, mais novo que o servidor de destino {}",
                connected.version
            ),
        ));
    }
    let tools = MysqlTools::detect_flavor(flavor, request.runner, connected.version)
        .await
        .map_err(|error| OperationError::new(ErrorCode::ToolUnavailable, error.to_string()))?;

    let admin = Admin::new(&server, &password, Timeouts::default());
    match admin.database_state(target.database()).await {
        Ok(DatabaseState::Missing) => {}
        Ok(DatabaseState::Exists { tables }) => {
            return Err(OperationError::new(
                ErrorCode::InvalidInput,
                format!(
                    "o banco {} já existe ({tables} tabela(s)); no {flavor} o restore só cria bancos novos, escolha outro --target-database",
                    target.database()
                ),
            ));
        }
        Err(error) => {
            return Err(OperationError::new(
                ErrorCode::RestoreFailed,
                describe(&error),
            ));
        }
    }
    Ok(PreparedRestore {
        request,
        target,
        password,
        tools,
        admin,
        content: dump,
        verified,
        history,
    })
}

impl PreparedRestore {
    pub async fn run(self, cancellation: &Cancellation) -> Result<RestoreOutcome, OperationError> {
        let Self {
            request,
            target,
            password,
            tools,
            admin,
            content: dump,
            verified,
            history,
        } = self;
        let engine = engine(request.flavor);
        if let Err(error) = admin.create_database(target.database()).await {
            return Err(OperationError::new(
                ErrorCode::RestoreFailed,
                describe(&error),
            ));
        }
        if let Err(error) = admin
            .probe_definers(target.database(), &dump.definers)
            .await
        {
            let cleanup = match admin.drop_database(target.database()).await {
                Ok(()) => format!(
                    "o banco {} criado pelo restore foi apagado",
                    target.database()
                ),
                Err(drop_error) => format!(
                    "ATENÇÃO: não foi possível apagar o banco vazio {}: {}",
                    target.database(),
                    describe(&drop_error)
                ),
            };
            let code = match error {
                AdminError::DefinerRefused { .. } => ErrorCode::InvalidInput,
                _ => ErrorCode::RestoreFailed,
            };
            return Err(OperationError::new(
                code,
                format!(
                    "{}; nenhum dado foi restaurado e {cleanup}",
                    describe(&error)
                ),
            ));
        }

        let file_location = request.source.display().to_string();
        let restore_id = history
            .start_restore(&NewRestore {
                artifact_id: verified.artifact_id.as_deref(),
                source_location: &file_location,
                source_sha256: Some(&verified.sha256),
                checksum_verified: verified.matched_reference,
                engine: engine.as_str(),
                target_host: Some(target.host()),
                target_port: Some(target.port()),
                target_database: target.database(),
                mode: RestoreMode::Create,
            })
            .map_err(|error| OperationError::from_source(ErrorCode::Internal, &error))?;

        info!(
            operation = "restore.run",
            engine = engine.as_str(),
            restore_id = %restore_id,
            target = %target,
            mode = RestoreMode::Create.as_str(),
            status = "running",
            "restore iniciado"
        );
        let started = Instant::now();
        let outcome = execute_restore(
            &tools,
            &admin,
            &target,
            &password,
            &request.file,
            &dump,
            &RestoreLimits {
                cancellation,
                timeout: request.timeout,
            },
        )
        .await;

        match outcome {
            Outcome::Succeeded => {
                let _ = history.finish_restore(&restore_id, ExecutionStatus::Succeeded, None, None);
                let elapsed = started.elapsed();
                info!(
                    operation = "restore.run",
                    engine = engine.as_str(),
                    restore_id = %restore_id,
                    status = "succeeded",
                    duration_ms = elapsed.as_millis() as u64,
                    "restore concluído"
                );
                Ok(RestoreOutcome {
                    restore_id,
                    database: target.database().to_owned(),
                    objects: dump.objects(),
                    tables: dump.tables,
                    checksum_verified: verified.matched_reference,
                    elapsed,
                })
            }
            Outcome::Canceled => {
                let message = "restore cancelado; o banco criado para o restore foi apagado";
                let _ = history.finish_restore(
                    &restore_id,
                    ExecutionStatus::Canceled,
                    None,
                    Some(message),
                );
                info!(operation = "restore.run", engine = engine.as_str(), restore_id = %restore_id, status = "canceled", "restore cancelado");
                Err(OperationError::new(ErrorCode::Canceled, message))
            }
            Outcome::Failed(message) => {
                let _ = history.finish_restore(
                    &restore_id,
                    ExecutionStatus::Failed,
                    None,
                    Some(&message),
                );
                info!(operation = "restore.run", engine = engine.as_str(), restore_id = %restore_id, status = "failed", "restore falhou");
                Err(OperationError::new(ErrorCode::RestoreFailed, message))
            }
        }
    }
}

async fn execute_restore(
    tools: &MysqlTools,
    admin: &Admin,
    target: &MysqlConnectionConfig,
    password: &Password,
    file: &Path,
    dump: &DumpInfo,
    limits: &RestoreLimits<'_>,
) -> Outcome {
    let name = target.database();
    let outcome = match tools.restore(target, password, file) {
        Ok(process) => wait_restore(process, limits).await,
        Err(error) => Outcome::Failed(error.to_string()),
    };
    let outcome = match outcome {
        Outcome::Succeeded => match admin.count_tables(name).await {
            Ok(tables) if tables == dump.tables as u64 => Outcome::Succeeded,
            Ok(tables) => Outcome::Failed(format!(
                "o restore terminou com {tables} tabela(s) em {name}, mas o dump tem {}",
                dump.tables
            )),
            Err(error) => Outcome::Failed(describe(&error)),
        },
        other => other,
    };
    if matches!(outcome, Outcome::Succeeded) {
        return outcome;
    }
    match admin.drop_database(name).await {
        Ok(()) => match outcome {
            Outcome::Failed(message) => Outcome::Failed(format!(
                "{message}; o banco {name} criado pelo restore foi apagado"
            )),
            other => other,
        },
        Err(error) => Outcome::Failed(format!(
            "{}; ATENÇÃO: não foi possível apagar o banco incompleto {name}: {}",
            match outcome {
                Outcome::Failed(message) => message,
                _ => "restore cancelado".to_owned(),
            },
            describe(&error)
        )),
    }
}

async fn wait_restore(mut process: RestoreProcess, limits: &RestoreLimits<'_>) -> Outcome {
    let RestoreLimits {
        cancellation,
        timeout,
    } = *limits;
    enum Waited {
        Done(Result<(), ToolError>),
        Interrupted,
        TimedOut,
    }
    let waited = tokio::select! {
        result = process.wait() => Waited::Done(result),
        () = cancellation.canceled() => Waited::Interrupted,
        () = deadline(timeout) => Waited::TimedOut,
    };
    match waited {
        Waited::Done(Ok(())) => Outcome::Succeeded,
        Waited::Done(Err(error)) => Outcome::Failed(error.to_string()),
        Waited::Interrupted => {
            process.cancel().await;
            Outcome::Canceled
        }
        Waited::TimedOut => {
            process.cancel().await;
            Outcome::Failed(timed_out(timeout))
        }
    }
}

pub async fn inspect(file: &Path) -> Result<DumpInfo, OperationError> {
    let path = file.to_owned();
    tokio::task::spawn_blocking(move || inspect_dump(&path))
        .await
        .map_err(|error| OperationError::internal(&error))?
        .map_err(|error| OperationError::from_source(ErrorCode::BackupCorrupted, &error))
}

async fn connect_server(
    flavor: Flavor,
    config: &MysqlConnectionConfig,
    password: &Password,
) -> Result<Server, OperationError> {
    let info = test_connection(config, password, Timeouts::default())
        .await
        .map_err(|error| OperationError::connection(&error))?;
    if Flavor::of(&info.server_version) != flavor {
        return Err(OperationError::new(
            ErrorCode::ServerMismatch,
            wrong_engine(flavor, &info.server_version),
        ));
    }
    let Some(version) = MysqlVersion::parse_as(flavor, &info.server_version) else {
        return Err(OperationError::new(
            ErrorCode::Internal,
            format!(
                "versão do servidor {flavor} não reconhecida: {}",
                info.server_version
            ),
        ));
    };
    Ok(Server { info, version })
}

#[must_use]
pub fn wrong_engine(flavor: Flavor, server_version: &str) -> String {
    let found = Flavor::of(server_version);
    format!(
        "o servidor é {found} ({server_version}), não {flavor}; use --engine {}",
        engine(found).as_str()
    )
}

async fn check_non_transactional(
    engine: DatabaseEngine,
    config: &MysqlConnectionConfig,
    password: &Password,
) -> Option<Vec<String>> {
    let admin = Admin::new(config, password, Timeouts::default());
    match admin.non_transactional_tables(config.database()).await {
        Ok(tables) => {
            if !tables.is_empty() {
                warn!(
                    operation = "backup.run",
                    engine = engine.as_str(),
                    count = tables.len(),
                    tables = %tables.join(", "),
                    "tabelas fora do InnoDB não entram no snapshot consistente do --single-transaction; escritas durante o backup podem deixá-las inconsistentes"
                );
            }
            Some(tables)
        }
        Err(error) => {
            warn!(
                operation = "backup.run",
                engine = engine.as_str(),
                error = %describe(&error),
                "não foi possível verificar o motor de armazenamento das tabelas"
            );
            None
        }
    }
}

fn start_execution(
    history: &History,
    engine: DatabaseEngine,
    config: &MysqlConnectionConfig,
    secret_ref: &str,
) -> Result<String, HistoryError> {
    let display_name = config.to_string();
    let connection_id = history.upsert_connection(&NewConnection {
        display_name: &display_name,
        engine: engine.as_str(),
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
    tools: &MysqlTools,
) -> Result<(), HistoryError> {
    let location = artifact.path.display().to_string();
    let tool_version = format!(
        "{} {} ({})",
        tools.flavor().dump_tool(),
        tools.version(),
        tools.kind().as_str()
    );
    history.add_artifact(
        execution_id,
        &NewArtifact {
            location: &location,
            format: format(tools.flavor()),
            size_bytes: artifact.size_bytes,
            checksum_sha256: Some(&artifact.sha256),
            tool_version: Some(&tool_version),
        },
    )?;
    history.finish_execution(execution_id, ExecutionStatus::Succeeded, None)
}

async fn deadline(timeout: Option<Duration>) {
    match timeout {
        Some(limit) => tokio::time::sleep(limit).await,
        None => std::future::pending().await,
    }
}

fn timed_out(timeout: Option<Duration>) -> String {
    format!(
        "tempo esgotado após {}s",
        timeout.map_or(0, |limit| limit.as_secs())
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send<T: Send>(_: T) {}

    #[allow(dead_code)]
    fn futures_are_send(
        backup: PreparedBackup,
        restore: PreparedRestore,
        backup_request: BackupRequest,
        restore_request: RestoreRequest,
        secrets: &(dyn SecretProvider + Sync),
        cancellation: &Cancellation,
        progress: &dyn ProgressSink,
    ) {
        assert_send(prepare_backup(backup_request));
        assert_send(prepare_restore(restore_request, secrets));
        assert_send(backup.run(cancellation, progress));
        assert_send(restore.run(cancellation));
    }
}
