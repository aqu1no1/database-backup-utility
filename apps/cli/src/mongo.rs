use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use database_backup_adapters::mongo::{
    Admin, ArchiveInfo, ConnectionError, DatabaseContents, DatabaseState, MongoServer, MongoTools,
    MongoVersion, RestoreProcess, Timeouts, ToolError, Topology, inspect_archive,
    latest_known_server, probe_server, test_connection,
};
use database_backup_core::{DatabaseEngine, MongoConnectionConfig, Operation, Password};
use database_backup_history::{
    ExecutionStatus, History, HistoryError, NewArtifact, NewConnection, NewRestore, RestoreMode,
};
use database_backup_storage::{
    ArtifactMetadata, LocalTarget, StorageError, StoredArtifact, backup_file_name,
    check_restore_source, write_metadata,
};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tracing::{info, warn};

use crate::archive::verify_checksum;
use crate::backup::{BackupResult, human_size, open_target};
use crate::output::{fail, success};
use crate::restore::RestoreResult;
use crate::{
    ConnectionArgs, ConnectionResult, MONGO_PORT, RunnerArg, TlsArg, describe, exit, exit_code_for,
    read_password, unsupported,
};

const ENGINE: DatabaseEngine = DatabaseEngine::Mongodb;
const FORMAT: &str = "mongodump-archive";
const EXTENSION: &str = "archive";
const DEFAULT_AUTH_DATABASE: &str = "admin";
const MIN_SERVER: MongoVersion = MongoVersion { major: 4, minor: 4 };
const MAX_LISTED_DIFFERENCES: usize = 5;

pub struct RestoreRequest<'a> {
    pub file: &'a Path,
    pub host: &'a str,
    pub port: Option<u16>,
    pub target_database: &'a str,
    pub user: &'a str,
    pub auth_database: Option<&'a str>,
    pub tls: TlsArg,
    pub runner: RunnerArg,
    pub overwrite: bool,
    pub allow_unverified: bool,
    pub timeout: Option<u64>,
    pub password_file: Option<PathBuf>,
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

pub async fn test(connection: &ConnectionArgs, timeout: u64) -> ExitCode {
    let config = match config_from(connection) {
        Ok(config) => config,
        Err(code) => return code,
    };
    let password = match read_password(ENGINE, &config, connection.password_file.clone()) {
        Ok((password, _)) => password,
        Err(message) => return fail(exit::INVALID_INPUT, &message),
    };
    let timeout = Duration::from_secs(timeout);
    let timeouts = Timeouts {
        connect: timeout,
        query: timeout,
    };
    match test_connection(&config, &password, timeouts).await {
        Ok(server) => {
            info!(
                operation = "connection.test",
                engine = ENGINE.as_str(),
                connection = %config,
                topology = server.topology.as_str(),
                status = "succeeded",
                "conexão testada"
            );
            let mut text = format!(
                "Conexão OK: MongoDB {} em {} ({})",
                server.server_version,
                config,
                server.topology.as_str()
            );
            if let Some(reason) = unsupported_topology(&server.topology) {
                text.push_str(&format!(
                    "\nAtenção: {reason}; backup e restore exigem um standalone ou o primário de um replica set"
                ));
            }
            success(
                &ConnectionResult {
                    server_version: &server.server_version,
                    connection: config.to_string(),
                    topology: Some(server.topology.as_str()),
                },
                &text,
            )
        }
        Err(error) => {
            info!(operation = "connection.test", engine = ENGINE.as_str(), connection = %config, status = "failed", "falha ao testar conexão");
            fail(exit_code_for(&error), &connection_message(&error))
        }
    }
}

pub async fn backup(
    connection: ConnectionArgs,
    output_dir: Option<PathBuf>,
    runner: RunnerArg,
    timeout: Option<u64>,
) -> ExitCode {
    let config = match config_from(&connection) {
        Ok(config) => config,
        Err(code) => return code,
    };
    let (password, source) = match read_password(ENGINE, &config, connection.password_file.clone())
    {
        Ok(value) => value,
        Err(message) => return fail(exit::INVALID_INPUT, &message),
    };
    let server = match connect_server(&config, &password, true).await {
        Ok(server) => server,
        Err(code) => return code,
    };
    let tools = match MongoTools::detect(runner.into(), server.version).await {
        Ok(tools) => tools,
        Err(error) => return fail(exit::TOOLS, &error.to_string()),
    };

    let history_path = match History::default_path() {
        Ok(path) => path,
        Err(error) => return fail(exit::FAILURE, &describe(&error)),
    };
    let history = match History::open(&history_path) {
        Ok(history) => history,
        Err(error) => return fail(exit::FAILURE, &describe(&error)),
    };
    let target = match open_target(output_dir, &history_path).await {
        Ok(target) => target,
        Err(code) => return code,
    };
    let execution_id = match start_execution(&history, &config, &source.secret_ref()) {
        Ok(id) => id,
        Err(error) => return fail(exit::FAILURE, &describe(&error)),
    };

    let consistency = consistency_warning(&server.topology);
    warn!(
        operation = "backup.run",
        engine = ENGINE.as_str(),
        execution_id = %execution_id,
        topology = server.topology.as_str(),
        "{consistency}"
    );
    info!(
        operation = "backup.run",
        engine = ENGINE.as_str(),
        execution_id = %execution_id,
        source = %config,
        runner = tools.kind().as_str(),
        topology = server.topology.as_str(),
        status = "running",
        "backup iniciado"
    );
    let started = Instant::now();
    let outcome = execute_backup(
        &config,
        &password,
        &tools,
        &target,
        &execution_id,
        &server,
        timeout.map(Duration::from_secs),
    )
    .await;

    match outcome {
        Ok(artifact) => {
            if let Err(error) = record_success(&history, &execution_id, &artifact, &tools) {
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
                engine = ENGINE.as_str(),
                execution_id = %execution_id,
                status = "succeeded",
                duration_ms = elapsed.as_millis() as u64,
                size_bytes = artifact.size_bytes,
                "backup concluído"
            );
            let text = format!(
                "Backup concluído: {} ({}, sha256 {}…) em {:.1}s\n{consistency}",
                artifact.path.display(),
                human_size(artifact.size_bytes),
                &artifact.sha256[..12],
                elapsed.as_secs_f64()
            );
            success(
                &BackupResult {
                    execution_id: &execution_id,
                    database: config.database(),
                    file: artifact.path.display().to_string(),
                    size_bytes: artifact.size_bytes,
                    sha256: &artifact.sha256,
                    duration_ms: elapsed.as_millis(),
                    non_transactional_tables: None,
                    topology: Some(server.topology.as_str()),
                },
                &text,
            )
        }
        Err(Failure::Canceled) => {
            let message = "backup cancelado pelo usuário";
            let _ =
                history.finish_execution(&execution_id, ExecutionStatus::Canceled, Some(message));
            info!(operation = "backup.run", engine = ENGINE.as_str(), execution_id = %execution_id, status = "canceled", "backup cancelado");
            fail(exit::CANCELED, message)
        }
        Err(Failure::Error(message)) => {
            let _ =
                history.finish_execution(&execution_id, ExecutionStatus::Failed, Some(&message));
            info!(operation = "backup.run", engine = ENGINE.as_str(), execution_id = %execution_id, status = "failed", "backup falhou");
            fail(exit::BACKUP_FAILED, &message)
        }
    }
}

async fn execute_backup(
    config: &MongoConnectionConfig,
    password: &Password,
    tools: &MongoTools,
    target: &LocalTarget,
    execution_id: &str,
    server: &MongoServer,
    timeout: Option<Duration>,
) -> Result<StoredArtifact, Failure> {
    let file_name = backup_file_name(config.database(), OffsetDateTime::now_utc(), EXTENSION);
    let mut pending = target.begin(&file_name).await.map_err(storage_failure)?;
    let mut process = tools.dump(config, password).map_err(tool_failure)?;
    let mut stdout = process
        .take_stdout()
        .ok_or_else(|| Failure::Error("mongodump não forneceu a saída".to_owned()))?;

    enum Copy {
        Done(Result<u64, StorageError>),
        Interrupted,
        TimedOut,
    }
    let copy = tokio::select! {
        result = pending.copy_from(&mut stdout) => Copy::Done(result),
        _ = tokio::signal::ctrl_c() => Copy::Interrupted,
        () = deadline(timeout) => Copy::TimedOut,
    };
    drop(stdout);
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
        .map_err(|message| Failure::Error(format!("o archive gerado foi recusado: {message}")))?;
    if archive.database != config.database() {
        return Err(Failure::Error(format!(
            "o archive gerado descreve o banco {}, não {}",
            archive.database,
            config.database()
        )));
    }
    let artifact = pending.commit().await.map_err(storage_failure)?;

    let created_at = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default();
    let mongodump_version = tools.version().to_string();
    let metadata = ArtifactMetadata {
        tool_name: env!("CARGO_BIN_NAME"),
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
    if let Err(error) = write_metadata(&artifact.path, &metadata).await {
        let _ = std::fs::remove_file(&artifact.path);
        return Err(storage_failure(error));
    }
    Ok(artifact)
}

pub async fn restore(request: &RestoreRequest<'_>) -> ExitCode {
    if request.overwrite {
        return unsupported(
            ENGINE,
            Operation::RestoreOverwrite,
            "restaure em um banco que não existe (--target-database)",
        );
    }
    let file = match std::path::absolute(request.file) {
        Ok(path) => path,
        Err(error) => return fail(exit::INVALID_ARCHIVE, &error.to_string()),
    };
    if let Err(error) = check_restore_source(&file).await {
        return fail(exit::INVALID_ARCHIVE, &describe(&error));
    }
    let target = match MongoConnectionConfig::new(
        request.host,
        request.port.unwrap_or(MONGO_PORT),
        request.target_database,
        request.user,
        request.auth_database.unwrap_or(DEFAULT_AUTH_DATABASE),
        request.tls.into(),
    ) {
        Ok(config) => config,
        Err(error) => return fail(exit::INVALID_INPUT, &error.to_string()),
    };

    let history = match History::default_path().and_then(|path| History::open(&path)) {
        Ok(history) => history,
        Err(error) => return fail(exit::FAILURE, &describe(&error)),
    };
    let verified = match verify_checksum(&file, &history, request.allow_unverified, ENGINE).await {
        Ok(verified) => verified,
        Err(message) => return fail(exit::INVALID_ARCHIVE, &message),
    };
    if !verified.matched_reference {
        warn!(
            operation = "restore.run",
            engine = ENGINE.as_str(),
            "checksum não verificado (--allow-unverified); a integridade do arquivo não foi confirmada e os dados do archive serão gravados no banco de destino"
        );
    }
    let archive = match inspect(&file).await {
        Ok(archive) => archive,
        Err(message) => return fail(exit::INVALID_ARCHIVE, &message),
    };
    if let Err(error) = target.with_database(&archive.database) {
        return fail(
            exit::INVALID_ARCHIVE,
            &format!("o archive descreve um banco inválido: {error}"),
        );
    }

    let (password, _) = match read_password(ENGINE, &target, request.password_file.clone()) {
        Ok(value) => value,
        Err(message) => return fail(exit::INVALID_INPUT, &message),
    };
    let server = match connect_server(&target, &password, false).await {
        Ok(server) => server,
        Err(code) => return code,
    };
    if let Some(from) = archive.dumped_from
        && from > server.version
    {
        return fail(
            exit::INVALID_ARCHIVE,
            &format!(
                "o backup veio do MongoDB {from}, mais novo que o servidor de destino {}",
                server.version
            ),
        );
    }
    let tools = match MongoTools::detect(request.runner.into(), server.version).await {
        Ok(tools) => tools,
        Err(error) => return fail(exit::TOOLS, &error.to_string()),
    };

    let admin = Admin::new(&target, &password, Timeouts::default());
    match admin.database_state(target.database()).await {
        Ok(DatabaseState::Missing) => {}
        Ok(DatabaseState::Exists { collections }) => {
            return fail(
                exit::INVALID_INPUT,
                &format!(
                    "o banco {} já existe ({collections} coleção(ões)); no MongoDB o restore só cria bancos novos, escolha outro --target-database",
                    target.database()
                ),
            );
        }
        Err(error) => return fail(exit::RESTORE_FAILED, &error.to_string()),
    }

    let file_location = file.display().to_string();
    let restore_id = match history.start_restore(&NewRestore {
        artifact_id: verified.artifact_id.as_deref(),
        source_location: &file_location,
        source_sha256: Some(&verified.sha256),
        checksum_verified: verified.matched_reference,
        engine: ENGINE.as_str(),
        target_host: Some(target.host()),
        target_port: Some(target.port()),
        target_database: target.database(),
        mode: RestoreMode::Create,
    }) {
        Ok(id) => id,
        Err(error) => return fail(exit::FAILURE, &describe(&error)),
    };

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
        &file,
        &archive,
        request.timeout.map(Duration::from_secs),
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
            success(
                &RestoreResult {
                    restore_id: &restore_id,
                    database: target.database(),
                    mode: RestoreMode::Create.as_str(),
                    previous_database: None,
                    objects: archive.objects(),
                    checksum_verified: verified.matched_reference,
                    duration_ms: elapsed.as_millis(),
                },
                &format!(
                    "Restore concluído em {} ({} objetos, {} coleções) em {:.1}s",
                    target.database(),
                    archive.objects(),
                    archive.collections(),
                    elapsed.as_secs_f64()
                ),
            )
        }
        Outcome::Canceled => {
            let message = "restore cancelado; o banco criado para o restore foi apagado";
            let _ =
                history.finish_restore(&restore_id, ExecutionStatus::Canceled, None, Some(message));
            info!(operation = "restore.run", engine = ENGINE.as_str(), restore_id = %restore_id, status = "canceled", "restore cancelado");
            fail(exit::CANCELED, message)
        }
        Outcome::Failed(message) => {
            let _ =
                history.finish_restore(&restore_id, ExecutionStatus::Failed, None, Some(&message));
            info!(operation = "restore.run", engine = ENGINE.as_str(), restore_id = %restore_id, status = "failed", "restore falhou");
            fail(exit::RESTORE_FAILED, &message)
        }
    }
}

async fn execute_restore(
    tools: &MongoTools,
    admin: &Admin,
    target: &MongoConnectionConfig,
    password: &Password,
    file: &Path,
    archive: &ArchiveInfo,
    timeout: Option<Duration>,
) -> Outcome {
    let name = target.database();
    let outcome = match tools.restore(target, password, file, &archive.database) {
        Ok(process) => wait_restore(process, timeout).await,
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

async fn wait_restore(mut process: RestoreProcess, timeout: Option<Duration>) -> Outcome {
    enum Waited {
        Done(Result<(), ToolError>),
        Interrupted,
        TimedOut,
    }
    let waited = tokio::select! {
        result = process.wait() => Waited::Done(result),
        _ = tokio::signal::ctrl_c() => Waited::Interrupted,
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

pub async fn inspect(file: &Path) -> Result<ArchiveInfo, String> {
    let path = file.to_owned();
    tokio::task::spawn_blocking(move || inspect_archive(&path))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| describe(&error))
}

fn config_from(connection: &ConnectionArgs) -> Result<MongoConnectionConfig, ExitCode> {
    MongoConnectionConfig::new(
        &connection.host,
        connection.port.unwrap_or(MONGO_PORT),
        &connection.database,
        &connection.user,
        connection
            .auth_database
            .as_deref()
            .unwrap_or(DEFAULT_AUTH_DATABASE),
        connection.tls.into(),
    )
    .map_err(|error| fail(exit::INVALID_INPUT, &error.to_string()))
}

async fn connect_server(
    config: &MongoConnectionConfig,
    password: &Password,
    require_database: bool,
) -> Result<MongoServer, ExitCode> {
    let timeouts = Timeouts::default();
    let server = if require_database {
        test_connection(config, password, timeouts).await
    } else {
        probe_server(config, password, timeouts).await
    }
    .map_err(|error| fail(exit_code_for(&error), &connection_message(&error)))?;
    if server.version < MIN_SERVER {
        return Err(fail(
            exit::FAILURE,
            &format!(
                "o MongoDB {} não é suportado; a versão mínima é {MIN_SERVER}",
                server.version
            ),
        ));
    }
    if let Some(reason) = unsupported_topology(&server.topology) {
        return Err(fail(
            exit::INVALID_INPUT,
            &format!(
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

fn connection_message(error: &ConnectionError) -> String {
    match error {
        ConnectionError::Other { .. } => describe(error),
        _ => error.to_string(),
    }
}

fn unsupported_topology(topology: &Topology) -> Option<String> {
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
    Failure::Error(describe(&error))
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
}
