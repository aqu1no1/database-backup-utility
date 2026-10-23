use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use clap::Args;
use database_backup_adapters::postgres::{
    PgTools, PgVersion, Timeouts, ToolError, test_connection,
};
use database_backup_core::{DatabaseEngine, Password, PostgresConnectionConfig};
use database_backup_history::{ExecutionStatus, History, NewArtifact, NewConnection};
use database_backup_storage::{
    ArtifactMetadata, LocalTarget, ORPHAN_MIN_AGE, StoredArtifact, backup_file_name, write_metadata,
};
use serde::Serialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tracing::{info, warn};

use crate::output::{fail, success};
use crate::{
    ConnectionArgs, EngineArg, PORT_HELP, RunnerArg, TlsArg, describe, exit, exit_code_for,
    read_password,
};

const FORMAT: &str = "pg_dump-custom";

#[derive(Debug, Args)]
pub struct BackupArgs {
    #[arg(
        long,
        value_enum,
        default_value_t = EngineArg::Postgres,
        help = "SGBD de origem"
    )]
    engine: EngineArg,
    #[arg(
        long,
        value_name = "ARQUIVO",
        required_if_eq("engine", "sqlite"),
        conflicts_with_all = ["host", "port", "database", "user", "tls", "password_file", "runner"],
        help = "Arquivo do banco SQLite de origem (com --engine sqlite)"
    )]
    source: Option<PathBuf>,
    #[command(flatten)]
    server: ServerArgs,
    #[arg(
        long,
        help = "Diretório de destino (padrão: backups/ na pasta de dados do aplicativo)"
    )]
    output_dir: Option<PathBuf>,
    #[arg(
        long,
        value_enum,
        default_value_t = RunnerArg::Auto,
        help = "auto: pg_dump/mysqldump nativo se compatível, senão Docker"
    )]
    runner: RunnerArg,
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..), help = "Tempo máximo do backup em segundos (padrão: sem limite)")]
    timeout: Option<u64>,
}

#[derive(Debug, Args)]
struct ServerArgs {
    #[arg(long, default_value = "localhost", help = "Host do servidor")]
    host: String,
    #[arg(long, help = PORT_HELP)]
    port: Option<u16>,
    #[arg(long, required_unless_present = "source", help = "Nome do banco")]
    database: Option<String>,
    #[arg(long, required_unless_present = "source", help = "Usuário")]
    user: Option<String>,
    #[arg(
        long,
        value_enum,
        default_value_t = TlsArg::Auto,
        help = "auto: sem TLS só em localhost; verify-full: TLS com certificado validado"
    )]
    tls: TlsArg,
    #[arg(
        long,
        value_name = "ARQUIVO",
        help = "Arquivo com a senha (permissão 600 ou 400)"
    )]
    password_file: Option<PathBuf>,
}

impl ServerArgs {
    fn into_connection(self) -> Option<ConnectionArgs> {
        Some(ConnectionArgs {
            host: self.host,
            port: self.port,
            database: self.database?,
            user: self.user?,
            tls: self.tls,
            password_file: self.password_file,
        })
    }
}

#[derive(Serialize)]
pub struct BackupResult<'a> {
    pub execution_id: &'a str,
    pub database: &'a str,
    pub file: String,
    pub size_bytes: u64,
    pub sha256: &'a str,
    pub duration_ms: u128,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub non_transactional_tables: Option<&'a [String]>,
}

enum Failure {
    Canceled,
    Error(String),
}

struct Completed {
    artifact: StoredArtifact,
}

pub async fn run(args: BackupArgs) -> ExitCode {
    let BackupArgs {
        engine,
        source,
        server,
        output_dir,
        runner,
        timeout,
    } = args;
    match (engine, source) {
        (EngineArg::Sqlite, Some(source)) => {
            crate::sqlite::backup(&source, output_dir, timeout).await
        }
        (EngineArg::Sqlite, None) => fail(
            exit::INVALID_INPUT,
            "--source é obrigatório com --engine sqlite",
        ),
        (EngineArg::Postgres | EngineArg::Mysql, Some(_)) => {
            fail(exit::INVALID_INPUT, "--source só vale com --engine sqlite")
        }
        (EngineArg::Mysql, None) => match server.into_connection() {
            Some(connection) => crate::mysql::backup(connection, output_dir, runner, timeout).await,
            None => fail(
                exit::INVALID_INPUT,
                "--database e --user são obrigatórios com --engine mysql",
            ),
        },
        (EngineArg::Postgres, None) => match server.into_connection() {
            Some(connection) => run_postgres(connection, output_dir, runner, timeout).await,
            None => fail(
                exit::INVALID_INPUT,
                "--database e --user são obrigatórios com --engine postgres",
            ),
        },
    }
}

async fn run_postgres(
    connection: ConnectionArgs,
    output_dir: Option<PathBuf>,
    runner: RunnerArg,
    timeout: Option<u64>,
) -> ExitCode {
    let config = match connection.config() {
        Ok(config) => config,
        Err(code) => return code,
    };
    let (password, source) = match read_password(
        DatabaseEngine::Postgres,
        &config,
        connection.password_file.clone(),
    ) {
        Ok(value) => value,
        Err(message) => return fail(exit::INVALID_INPUT, &message),
    };

    let info = match test_connection(&config, &password, Timeouts::default()).await {
        Ok(info) => info,
        Err(error) => return fail(exit_code_for(&error), &error.to_string()),
    };
    let Some(server) = PgVersion::parse(&info.server_version) else {
        return fail(
            exit::FAILURE,
            &format!(
                "versão do servidor não reconhecida: {}",
                info.server_version
            ),
        );
    };
    let tools = match PgTools::detect(runner.into(), server).await {
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

    info!(
        operation = "backup.run",
        execution_id = %execution_id,
        source = %config,
        runner = tools.kind().as_str(),
        status = "running",
        "backup iniciado"
    );
    let started = Instant::now();
    let timeout = timeout.map(Duration::from_secs);
    let outcome = execute(
        &config,
        &password,
        &tools,
        &target,
        &execution_id,
        &info.server_version,
        timeout,
    )
    .await;

    match outcome {
        Ok(Completed { artifact }) => {
            let recorded = record_success(&history, &execution_id, &artifact, &tools);
            if let Err(error) = recorded {
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
                    database: config.database(),
                    file: artifact.path.display().to_string(),
                    size_bytes: artifact.size_bytes,
                    sha256: &artifact.sha256,
                    duration_ms: elapsed.as_millis(),
                    non_transactional_tables: None,
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

async fn execute(
    config: &PostgresConnectionConfig,
    password: &Password,
    tools: &PgTools,
    target: &LocalTarget,
    execution_id: &str,
    server_version: &str,
    timeout: Option<Duration>,
) -> Result<Completed, Failure> {
    let file_name = backup_file_name(config.database(), OffsetDateTime::now_utc(), "dump");
    let mut pending = target.begin(&file_name).await.map_err(storage_failure)?;
    let mut process = tools.dump(config, password).map_err(tool_failure)?;
    let mut stdout = process
        .take_stdout()
        .ok_or_else(|| Failure::Error("pg_dump não forneceu a saída".to_owned()))?;

    enum Copy {
        Done(Result<u64, database_backup_storage::StorageError>),
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
        result = pending.copy_from(&mut stdout) => Copy::Done(result),
        _ = tokio::signal::ctrl_c() => Copy::Interrupted,
        () = deadline => Copy::TimedOut,
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
            return Err(Failure::Error(format!(
                "tempo esgotado após {}s",
                timeout.map_or(0, |limit| limit.as_secs())
            )));
        }
    }

    process.wait().await.map_err(tool_failure)?;
    pending.finish_writing().await.map_err(storage_failure)?;
    tools
        .restore_list(pending.partial_path())
        .await
        .map_err(|error| {
            Failure::Error(format!("o arquivo gerado não é um archive válido: {error}"))
        })?;
    let artifact = pending.commit().await.map_err(storage_failure)?;

    let created_at = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default();
    let pg_dump_version = tools.version().to_string();
    let metadata = ArtifactMetadata {
        tool_name: env!("CARGO_BIN_NAME"),
        tool_version: env!("CARGO_PKG_VERSION"),
        engine: DatabaseEngine::Postgres.as_str(),
        format: FORMAT,
        sha256: &artifact.sha256,
        size_bytes: artifact.size_bytes,
        database: config.database(),
        host: Some(config.host()),
        port: Some(config.port()),
        server_version,
        pg_dump_version: Some(&pg_dump_version),
        mysqldump_version: None,
        non_transactional_tables: None,
        runner: Some(tools.kind().as_str()),
        created_at: &created_at,
        execution_id,
    };
    if let Err(error) = write_metadata(&artifact.path, &metadata).await {
        let _ = std::fs::remove_file(&artifact.path);
        return Err(storage_failure(error));
    }
    Ok(Completed { artifact })
}

fn start_execution(
    history: &History,
    config: &PostgresConnectionConfig,
    secret_ref: &str,
) -> Result<String, database_backup_history::HistoryError> {
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
) -> Result<(), database_backup_history::HistoryError> {
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

pub async fn open_target(
    output_dir: Option<PathBuf>,
    history_path: &Path,
) -> Result<LocalTarget, ExitCode> {
    let output_dir = output_dir.unwrap_or_else(|| default_output_dir(history_path));
    let target = LocalTarget::new(output_dir)
        .await
        .map_err(|error| fail(exit::FAILURE, &describe(&error)))?;
    if target.is_shared().await.unwrap_or(false) {
        warn!(
            operation = "backup.run",
            dir = %target.dir().display(),
            "o diretório de saída é acessível por grupo ou outros usuários; os backups ficam com permissão 0600, mas os nomes dos arquivos ficam visíveis (use chmod 700)"
        );
    }
    match target.remove_orphan_partials(ORPHAN_MIN_AGE).await {
        Ok(cleanup) => {
            for path in &cleanup.removed {
                warn!(
                    operation = "backup.run",
                    file = %path.display(),
                    "removido arquivo .partial de um backup interrompido"
                );
            }
            if !cleanup.in_use.is_empty() {
                info!(
                    operation = "backup.run",
                    count = cleanup.in_use.len(),
                    "há outros backups em andamento neste diretório"
                );
            }
        }
        Err(error) => warn!(
            operation = "backup.run",
            error = %describe(&error),
            "não foi possível verificar arquivos .partial antigos"
        ),
    }
    Ok(target)
}

fn default_output_dir(history_path: &Path) -> PathBuf {
    history_path
        .parent()
        .map_or_else(|| PathBuf::from("backups"), |dir| dir.join("backups"))
}

fn tool_failure(error: ToolError) -> Failure {
    Failure::Error(error.to_string())
}

fn storage_failure(error: database_backup_storage::StorageError) -> Failure {
    Failure::Error(describe(&error))
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
