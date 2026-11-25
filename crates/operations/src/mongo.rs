use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use database_backup_adapters::mongo::{
    Admin, ArchiveInfo, ConnectionError, DatabaseContents, DatabaseState, MongoServer, MongoTools,
    MongoVersion, RestoreProcess, Timeouts, ToolError, Topology, inspect_archive,
    latest_known_server, probe_server, test_connection,
};
use database_backup_adapters::postgres::RunnerChoice;
use database_backup_core::{
    DatabaseEngine, MongoConnectionConfig, Password, SecretProvider, TlsMode,
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

const ENGINE: DatabaseEngine = DatabaseEngine::Mongodb;
const FORMAT: &str = "mongodump-archive";
const EXTENSION: &str = "archive";
pub const DEFAULT_AUTH_DATABASE: &str = "admin";
const MIN_SERVER: MongoVersion = MongoVersion { major: 4, minor: 4 };
const MAX_LISTED_DIFFERENCES: usize = 5;

#[derive(Debug)]
pub struct BackupRequest {
    pub config: MongoConnectionConfig,
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
    pub topology: Topology,
    pub consistency: String,
}

pub struct PreparedBackup {
    request: BackupRequest,
    tools: MongoTools,
    server: MongoServer,
    history: History,
    target: LocalTarget,
}

#[derive(Debug)]
pub struct RestoreRequest {
    pub file: PathBuf,
    pub source: PathBuf,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub tls: TlsMode,
    pub auth_database: String,
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
    pub collections: usize,
    pub checksum_verified: bool,
    pub elapsed: Duration,
}

pub struct PreparedRestore {
    request: RestoreRequest,
    target: MongoConnectionConfig,
    password: Password,
    tools: MongoTools,
    admin: Admin,
    content: ArchiveInfo,
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

pub async fn prepare_backup(request: BackupRequest) -> Result<PreparedBackup, OperationError> {
    let server = connect_server(&request.config, &request.password, true).await?;
    let tools = MongoTools::detect(request.runner, server.version)
        .await
        .map_err(|error| OperationError::new(ErrorCode::ToolUnavailable, error.to_string()))?;

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

        let consistency = consistency_warning(&self.server.topology);
        warn!(
            operation = "backup.run",
            engine = ENGINE.as_str(),
            execution_id = %execution_id,
            topology = self.server.topology.as_str(),
            "{consistency}"
        );
        info!(
            operation = "backup.run",
            engine = ENGINE.as_str(),
            execution_id = %execution_id,
            source = %self.request.config,
            runner = self.tools.kind().as_str(),
            topology = self.server.topology.as_str(),
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
            &self.server,
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
                    engine = ENGINE.as_str(),
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
                    topology: self.server.topology,
                    consistency,
                })
            }
            Err(Failure::Canceled) => {
                let message = "backup cancelado pelo usuário";
                let _ = self.history.finish_execution(
                    &execution_id,
                    ExecutionStatus::Canceled,
                    Some(message),
                );
                info!(operation = "backup.run", engine = ENGINE.as_str(), execution_id = %execution_id, status = "canceled", "backup cancelado");
                Err(OperationError::new(ErrorCode::Canceled, message))
            }
            Err(Failure::Error(message)) => {
                let _ = self.history.finish_execution(
                    &execution_id,
                    ExecutionStatus::Failed,
                    Some(&message),
                );
                info!(operation = "backup.run", engine = ENGINE.as_str(), execution_id = %execution_id, status = "failed", "backup falhou");
                Err(OperationError::new(ErrorCode::BackupFailed, message))
            }
        }
    }
}

async fn execute_backup(
    config: &MongoConnectionConfig,
    password: &Password,
    tools: &MongoTools,
    destination: &Destination<'_>,
    execution_id: &str,
    server: &MongoServer,
    control: &Control<'_>,
) -> Result<StoredArtifact, Failure> {
    let Destination { target, encoding } = *destination;
    let timeout = control.timeout;
    let file_name = backup_file_name(config.database(), OffsetDateTime::now_utc(), EXTENSION);
    let mut pending = target.begin(&file_name).await.map_err(storage_failure)?;
    let mut process = tools.dump(config, password).map_err(tool_failure)?;
    let stdout = process
        .take_stdout()
        .ok_or_else(|| Failure::Error("mongodump não forneceu a saída".to_owned()))?;

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
    let archive = inspect(pending.partial_path())
        .await
        .map_err(|error| Failure::Error(format!("o archive gerado foi recusado: {error}")))?;
    if archive.database != config.database() {
        return Err(Failure::Error(format!(
            "o archive gerado descreve o banco {}, não {}",
            archive.database,
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
    let mongodump_version = tools.version().to_string();
    let metadata = ArtifactMetadata {
        tool_name: TOOL_NAME,
        tool_version: env!("CARGO_PKG_VERSION"),
        engine: ENGINE.as_str(),
        format: FORMAT,
        sha256: &artifact.sha256,
        size_bytes: artifact.size_bytes,
        database: config.database(),
        host: Some(config.host()),
        port: Some(config.port()),
        server_version: &server.server_version,
        pg_dump_version: None,
        mysqldump_version: None,
        mongodump_version: Some(&mongodump_version),
        non_transactional_tables: None,
        topology: Some(server.topology.as_str()),
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
    check_restore_source(&request.file)
        .await
        .map_err(|error| OperationError::source_file(&error))?;
    let target = MongoConnectionConfig::new(
        &request.host,
        request.port,
        &request.target_database,
        &request.user,
        &request.auth_database,
        request.tls,
    )
    .map_err(|error| OperationError::new(ErrorCode::InvalidInput, error.to_string()))?;

    let history = History::default_path()
        .and_then(|path| History::open(&path))
        .map_err(|error| OperationError::from_source(ErrorCode::Internal, &error))?;
    let verified = verify_checksum(
        &request.file,
        &request.source,
        &history,
        request.allow_unverified,
        ENGINE,
    )
    .await?;
    if !verified.matched_reference {
        warn!(
            operation = "restore.run",
            engine = ENGINE.as_str(),
            "checksum não verificado (--allow-unverified); a integridade do arquivo não foi confirmada e os dados do archive serão gravados no banco de destino"
        );
    }
    let archive = inspect(&request.file).await?;
    if let Err(error) = target.with_database(&archive.database) {
        return Err(OperationError::new(
            ErrorCode::BackupIncompatible,
            format!("o archive descreve um banco inválido: {error}"),
        ));
    }

    let (password, _) = secrets
        .password(&target)
        .map_err(|error| OperationError::from_source(ErrorCode::InvalidInput, &error))?;
    let server = connect_server(&target, &password, false).await?;
    if let Some(from) = archive.dumped_from
        && from > server.version
    {
        return Err(OperationError::new(
            ErrorCode::BackupIncompatible,
            format!(
                "o backup veio do MongoDB {from}, mais novo que o servidor de destino {}",
                server.version
            ),
        ));
    }
    let tools = MongoTools::detect(request.runner, server.version)
        .await
        .map_err(|error| OperationError::new(ErrorCode::ToolUnavailable, error.to_string()))?;

    let admin = Admin::new(&target, &password, Timeouts::default());
    require_missing(&admin, target.database()).await?;

    Ok(PreparedRestore {
        request,
        target,
        password,
        tools,
        admin,
        content: archive,
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
            content: archive,
            verified,
            history,
        } = self;
        require_missing(&admin, target.database()).await?;
        let file_location = request.source.display().to_string();
        let restore_id = history
            .start_restore(&NewRestore {
                artifact_id: verified.artifact_id.as_deref(),
                source_location: &file_location,
                source_sha256: Some(&verified.sha256),
                checksum_verified: verified.matched_reference,
                engine: ENGINE.as_str(),
                target_host: Some(target.host()),
                target_port: Some(target.port()),
                target_database: target.database(),
                mode: RestoreMode::Create,
            })
            .map_err(|error| OperationError::from_source(ErrorCode::Internal, &error))?;

        info!(
            operation = "restore.run",
            engine = ENGINE.as_str(),
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
            &archive,
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
                    engine = ENGINE.as_str(),
                    restore_id = %restore_id,
                    status = "succeeded",
                    duration_ms = elapsed.as_millis() as u64,
                    "restore concluído"
                );
                Ok(RestoreOutcome {
                    restore_id,
                    database: target.database().to_owned(),
                    objects: archive.objects(),
                    collections: archive.collections(),
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
                info!(operation = "restore.run", engine = ENGINE.as_str(), restore_id = %restore_id, status = "canceled", "restore cancelado");
                Err(OperationError::new(ErrorCode::Canceled, message))
            }
            Outcome::Failed(message) => {
                let _ = history.finish_restore(
                    &restore_id,
                    ExecutionStatus::Failed,
                    None,
                    Some(&message),
                );
                info!(operation = "restore.run", engine = ENGINE.as_str(), restore_id = %restore_id, status = "failed", "restore falhou");
                Err(OperationError::new(ErrorCode::RestoreFailed, message))
            }
        }
    }
}

async fn require_missing(admin: &Admin, name: &str) -> Result<(), OperationError> {
    match admin.database_state(name).await {
        Ok(DatabaseState::Missing) => Ok(()),
        Ok(DatabaseState::Exists { collections }) => Err(OperationError::new(
            ErrorCode::InvalidInput,
            format!(
                "o banco {name} já existe ({collections} coleção(ões)); no MongoDB o restore só cria bancos novos, escolha outro --target-database"
            ),
        )),
        Err(error) => Err(OperationError::new(
            ErrorCode::RestoreFailed,
            error.to_string(),
        )),
    }
}

async fn execute_restore(
    tools: &MongoTools,
    admin: &Admin,
    target: &MongoConnectionConfig,
    password: &Password,
    file: &Path,
    archive: &ArchiveInfo,
    limits: &RestoreLimits<'_>,
) -> Outcome {
    let name = target.database();
    let outcome = match tools.restore(target, password, file, &archive.database) {
        Ok(process) => wait_restore(process, limits).await,
        Err(error) => Outcome::Failed(error.to_string()),
    };
    let outcome = match outcome {
        Outcome::Succeeded => match admin.contents(name).await {
            Ok(found) if found == archive.contents => Outcome::Succeeded,
            Ok(found) => Outcome::Failed(format!(
                "o restore terminou, mas o conteúdo de {name} difere do archive: {}",
                differences(&archive.contents, &found)
            )),
            Err(error) => Outcome::Failed(error.to_string()),
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
            error
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

pub async fn inspect(file: &Path) -> Result<ArchiveInfo, OperationError> {
    let path = file.to_owned();
    tokio::task::spawn_blocking(move || inspect_archive(&path))
        .await
        .map_err(|error| OperationError::internal(&error))?
        .map_err(|error| OperationError::from_source(ErrorCode::BackupCorrupted, &error))
}

async fn connect_server(
    config: &MongoConnectionConfig,
    password: &Password,
    require_database: bool,
) -> Result<MongoServer, OperationError> {
    let timeouts = Timeouts::default();
    let server = if require_database {
        test_connection(config, password, timeouts).await
    } else {
        probe_server(config, password, timeouts).await
    }
    .map_err(|error| {
        OperationError::new(
            OperationError::connection(&error).code(),
            connection_message(&error),
        )
    })?;
    if server.version < MIN_SERVER {
        return Err(OperationError::new(
            ErrorCode::Internal,
            format!(
                "o MongoDB {} não é suportado; a versão mínima é {MIN_SERVER}",
                server.version
            ),
        ));
    }
    if let Some(reason) = unsupported_topology(&server.topology) {
        return Err(OperationError::new(
            ErrorCode::InvalidInput,
            format!(
                "{reason}; informe um standalone ou o primário de um replica set (cluster sharded e leitura de secundário não são suportados)"
            ),
        ));
    }
    if server.version > latest_known_server() {
        warn!(
            engine = ENGINE.as_str(),
            server = %server.version,
            known = %latest_known_server(),
            "o servidor é mais novo que a matriz de compatibilidade das ferramentas; será exigida a versão das ferramentas da última versão conhecida"
        );
    }
    Ok(server)
}

#[must_use]
pub fn connection_message(error: &ConnectionError) -> String {
    match error {
        ConnectionError::Other { .. } => describe(error),
        _ => error.to_string(),
    }
}

#[must_use]
pub fn unsupported_topology(topology: &Topology) -> Option<String> {
    match topology {
        Topology::Standalone | Topology::ReplicaSet { primary: true, .. } => None,
        Topology::ReplicaSet { name, .. } => {
            Some(format!("o servidor não é o primário do replica set {name}"))
        }
        Topology::Sharded => Some("o servidor é um mongos de um cluster sharded".to_owned()),
    }
}

fn consistency_warning(topology: &Topology) -> String {
    format!(
        "Atenção: o backup de um banco MongoDB não é um snapshot de um único instante (topologia: {}); escritas durante o backup podem deixar coleções inconsistentes entre si",
        topology.as_str()
    )
}

fn differences(expected: &DatabaseContents, found: &DatabaseContents) -> String {
    let mut lines = Vec::new();
    for (name, count) in &expected.documents {
        match found.documents.get(name) {
            Some(restored) if restored == count => {}
            Some(restored) => lines.push(format!(
                "a coleção {name} tem {restored} documento(s), o archive tem {count}"
            )),
            None => lines.push(format!("a coleção {name} não foi criada")),
        }
    }
    for name in found.documents.keys() {
        if !expected.documents.contains_key(name) {
            lines.push(format!("a coleção {name} não consta no archive"));
        }
    }
    for name in expected.views.difference(&found.views) {
        lines.push(format!("a view {name} não foi criada"));
    }
    for name in found.views.difference(&expected.views) {
        lines.push(format!("a view {name} não consta no archive"));
    }
    let total = lines.len();
    lines.truncate(MAX_LISTED_DIFFERENCES);
    let mut text = lines.join("; ");
    if total > MAX_LISTED_DIFFERENCES {
        text.push_str(&format!(
            "; e mais {} diferença(s)",
            total - MAX_LISTED_DIFFERENCES
        ));
    }
    text
}

fn start_execution(
    history: &History,
    config: &MongoConnectionConfig,
    secret_ref: &str,
) -> Result<String, HistoryError> {
    let display_name = config.to_string();
    let connection_id = history.upsert_connection(&NewConnection {
        display_name: &display_name,
        engine: ENGINE.as_str(),
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
    tools: &MongoTools,
) -> Result<(), HistoryError> {
    let location = artifact.path.display().to_string();
    let tool_version = format!("mongodump {} ({})", tools.version(), tools.kind().as_str());
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

    fn contents(documents: &[(&str, u64)], views: &[&str]) -> DatabaseContents {
        DatabaseContents {
            documents: documents
                .iter()
                .map(|(name, count)| ((*name).to_owned(), *count))
                .collect(),
            views: views.iter().map(|name| (*name).to_owned()).collect(),
        }
    }

    #[test]
    fn restore_differences_name_the_collections_that_disagree() {
        let archive = contents(&[("a", 3), ("b", 0), ("c", 1)], &["v"]);
        let restored = contents(&[("a", 2), ("c", 1), ("x", 9)], &[]);

        assert_eq!(
            differences(&archive, &restored),
            "a coleção a tem 2 documento(s), o archive tem 3; a coleção b não foi criada; a coleção x não consta no archive; a view v não foi criada"
        );
        assert_eq!(differences(&archive, &archive), "");
    }

    #[test]
    fn only_standalone_and_primary_are_supported() {
        let replica = |primary| Topology::ReplicaSet {
            name: "rs0".to_owned(),
            primary,
        };

        assert_eq!(unsupported_topology(&Topology::Standalone), None);
        assert_eq!(unsupported_topology(&replica(true)), None);
        assert!(unsupported_topology(&replica(false)).is_some_and(|reason| reason.contains("rs0")));
        assert!(unsupported_topology(&Topology::Sharded).is_some());
    }

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
