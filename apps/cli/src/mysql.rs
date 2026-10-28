use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use database_backup_adapters::mysql::{
    Admin, AdminError, ConnectionInfo, DatabaseState, DumpInfo, MysqlTools, MysqlVersion,
    RestoreProcess, Timeouts, ToolError, inspect_dump, test_connection,
};
use database_backup_core::{DatabaseEngine, MysqlConnectionConfig, Operation, Password};
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
    ConnectionArgs, ConnectionResult, MYSQL_PORT, RunnerArg, TlsArg, describe, exit, exit_code_for,
    read_password, unsupported,
};

const ENGINE: DatabaseEngine = DatabaseEngine::Mysql;
const FORMAT: &str = "mysqldump-sql";
const EXTENSION: &str = "sql";
const CATALOG_DATABASE: &str = "information_schema";

pub struct RestoreRequest<'a> {
    pub file: &'a Path,
    pub host: &'a str,
    pub port: Option<u16>,
    pub target_database: &'a str,
    pub user: &'a str,
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

struct SourceFacts<'a> {
    server_version: &'a str,
    non_transactional: Option<&'a [String]>,
}

struct Server {
    info: ConnectionInfo,
    version: MysqlVersion,
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
        Ok(info) => {
            info!(
                operation = "connection.test",
                engine = ENGINE.as_str(),
                connection = %config,
                status = "succeeded",
                "conexão testada"
            );
            success(
                &ConnectionResult {
                    server_version: &info.server_version,
                    connection: config.to_string(),
                    topology: None,
                },
                &format!("Conexão OK: MySQL {} em {}", info.server_version, config),
            )
        }
        Err(error) => {
            info!(operation = "connection.test", engine = ENGINE.as_str(), connection = %config, status = "failed", "falha ao testar conexão");
            fail(exit_code_for(&error), &error.to_string())
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
    let server = match connect_server(&config, &password).await {
        Ok(server) => server,
        Err(code) => return code,
    };
    let tools = match MysqlTools::detect(runner.into(), server.version).await {
        Ok(tools) => tools,
        Err(error) => return fail(exit::TOOLS, &error.to_string()),
    };
    let non_transactional = check_non_transactional(&config, &password).await;

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

    info!(
        operation = "backup.run",
        engine = ENGINE.as_str(),
        execution_id = %execution_id,
        source = %config,
        runner = tools.kind().as_str(),
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
        &SourceFacts {
            server_version: &server.info.server_version,
            non_transactional: non_transactional.as_deref(),
        },
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
            let mut text = format!(
                "Backup concluído: {} ({}, sha256 {}…) em {:.1}s",
                artifact.path.display(),
                human_size(artifact.size_bytes),
                &artifact.sha256[..12],
                elapsed.as_secs_f64()
            );
            if let Some(tables) = non_transactional.as_deref().filter(|t| !t.is_empty()) {
                text.push_str(&format!(
                    "\nAtenção: {} tabela(s) fora do InnoDB, sem snapshot consistente: {}",
                    tables.len(),
                    tables.join(", ")
                ));
            }
            success(
                &BackupResult {
                    execution_id: &execution_id,
                    database: config.database(),
                    file: artifact.path.display().to_string(),
                    size_bytes: artifact.size_bytes,
                    sha256: &artifact.sha256,
                    duration_ms: elapsed.as_millis(),
                    non_transactional_tables: non_transactional.as_deref(),
                    topology: None,
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
    config: &MysqlConnectionConfig,
    password: &Password,
    tools: &MysqlTools,
    target: &LocalTarget,
    execution_id: &str,
    source: &SourceFacts<'_>,
    timeout: Option<Duration>,
) -> Result<StoredArtifact, Failure> {
    let file_name = backup_file_name(config.database(), OffsetDateTime::now_utc(), EXTENSION);
    let mut pending = target.begin(&file_name).await.map_err(storage_failure)?;
    let mut process = tools.dump(config, password).map_err(tool_failure)?;
    let mut stdout = process
        .take_stdout()
        .ok_or_else(|| Failure::Error("mysqldump não forneceu a saída".to_owned()))?;

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
    let dump = inspect(pending.partial_path())
        .await
        .map_err(|message| Failure::Error(format!("o dump gerado foi recusado: {message}")))?;
    if dump.database.as_deref() != Some(config.database()) {
        return Err(Failure::Error(format!(
            "o dump gerado descreve o banco {}, não {}",
            dump.database.as_deref().unwrap_or("desconhecido"),
            config.database()
        )));
    }
    let artifact = pending.commit().await.map_err(storage_failure)?;

    let created_at = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default();
    let mysqldump_version = tools.version().to_string();
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
    let server = match MysqlConnectionConfig::new(
        request.host,
        request.port.unwrap_or(MYSQL_PORT),
        CATALOG_DATABASE,
        request.user,
        request.tls.into(),
    ) {
        Ok(config) => config,
        Err(error) => return fail(exit::INVALID_INPUT, &error.to_string()),
    };
    let target = match server.with_database(request.target_database) {
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
            "checksum não verificado (--allow-unverified); a integridade do arquivo não foi confirmada e o SQL do arquivo será executado com as permissões do usuário"
        );
    }
    let dump = match inspect(&file).await {
        Ok(dump) => dump,
        Err(message) => return fail(exit::INVALID_ARCHIVE, &message),
    };

    let (password, _) = match read_password(ENGINE, &target, request.password_file.clone()) {
        Ok(value) => value,
        Err(message) => return fail(exit::INVALID_INPUT, &message),
    };
    let connected = match connect_server(&server, &password).await {
        Ok(server) => server,
        Err(code) => return code,
    };
    if let Some(from) = dump.dumped_from
        && from > connected.version
    {
        return fail(
            exit::INVALID_ARCHIVE,
            &format!(
                "o backup veio do MySQL {from}, mais novo que o servidor de destino {}",
                connected.version
            ),
        );
    }
    let tools = match MysqlTools::detect(request.runner.into(), connected.version).await {
        Ok(tools) => tools,
        Err(error) => return fail(exit::TOOLS, &error.to_string()),
    };

    let admin = Admin::new(&server, &password, Timeouts::default());
    match admin.database_state(target.database()).await {
        Ok(DatabaseState::Missing) => {}
        Ok(DatabaseState::Exists { tables }) => {
            return fail(
                exit::INVALID_INPUT,
                &format!(
                    "o banco {} já existe ({tables} tabela(s)); no MySQL o restore só cria bancos novos, escolha outro --target-database",
                    target.database()
                ),
            );
        }
        Err(error) => return fail(exit::RESTORE_FAILED, &describe(&error)),
    }
    if let Err(error) = admin.create_database(target.database()).await {
        return fail(exit::RESTORE_FAILED, &describe(&error));
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
            AdminError::DefinerRefused { .. } => exit::INVALID_INPUT,
            _ => exit::RESTORE_FAILED,
        };
        return fail(
            code,
            &format!(
                "{}; nenhum dado foi restaurado e {cleanup}",
                describe(&error)
            ),
        );
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
        &dump,
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
                    objects: dump.objects(),
                    checksum_verified: verified.matched_reference,
                    duration_ms: elapsed.as_millis(),
                },
                &format!(
                    "Restore concluído em {} ({} objetos, {} tabelas) em {:.1}s",
                    target.database(),
                    dump.objects(),
                    dump.tables,
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
    tools: &MysqlTools,
    admin: &Admin,
    target: &MysqlConnectionConfig,
    password: &Password,
    file: &Path,
    dump: &DumpInfo,
    timeout: Option<Duration>,
) -> Outcome {
    let name = target.database();
    let outcome = match tools.restore(target, password, file) {
        Ok(process) => wait_restore(process, timeout).await,
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

pub async fn inspect(file: &Path) -> Result<DumpInfo, String> {
    let path = file.to_owned();
    tokio::task::spawn_blocking(move || inspect_dump(&path))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| describe(&error))
}

fn config_from(connection: &ConnectionArgs) -> Result<MysqlConnectionConfig, ExitCode> {
    MysqlConnectionConfig::new(
        &connection.host,
        connection.port.unwrap_or(MYSQL_PORT),
        &connection.database,
        &connection.user,
        connection.tls.into(),
    )
    .map_err(|error| fail(exit::INVALID_INPUT, &error.to_string()))
}

async fn connect_server(
    config: &MysqlConnectionConfig,
    password: &Password,
) -> Result<Server, ExitCode> {
    let info = test_connection(config, password, Timeouts::default())
        .await
        .map_err(|error| fail(exit_code_for(&error), &error.to_string()))?;
    let Some(version) = MysqlVersion::parse(&info.server_version) else {
        return Err(fail(
            exit::FAILURE,
            &format!(
                "versão do servidor não reconhecida ou não suportada (MariaDB não é suportado): {}",
                info.server_version
            ),
        ));
    };
    Ok(Server { info, version })
}

async fn check_non_transactional(
    config: &MysqlConnectionConfig,
    password: &Password,
) -> Option<Vec<String>> {
    let admin = Admin::new(config, password, Timeouts::default());
    match admin.non_transactional_tables(config.database()).await {
        Ok(tables) => {
            if !tables.is_empty() {
                warn!(
                    operation = "backup.run",
                    engine = ENGINE.as_str(),
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
                engine = ENGINE.as_str(),
                error = %describe(&error),
                "não foi possível verificar o motor de armazenamento das tabelas"
            );
            None
        }
    }
}

fn start_execution(
    history: &History,
    config: &MysqlConnectionConfig,
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
    tools: &MysqlTools,
) -> Result<(), HistoryError> {
    let location = artifact.path.display().to_string();
    let tool_version = format!("mysqldump {} ({})", tools.version(), tools.kind().as_str());
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
