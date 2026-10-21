use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use clap::Args;
use database_backup_adapters::postgres::{
    PgTools, PgVersion, Timeouts, ToolError, test_connection,
};
use database_backup_core::{Password, PostgresConnectionConfig};
use database_backup_history::{ExecutionStatus, History, NewArtifact, NewConnection};
use database_backup_storage::{
    ArtifactMetadata, LocalTarget, StoredArtifact, backup_file_name, write_metadata,
};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::{ConnectionArgs, RunnerArg, describe, exit, exit_code_for, fail, read_password};

const FORMAT: &str = "pg_dump-custom";

#[derive(Debug, Args)]
pub struct BackupArgs {
    #[command(flatten)]
    connection: ConnectionArgs,
    #[arg(
        long,
        help = "Diretório de destino (padrão: backups/ na pasta de dados do aplicativo)"
    )]
    output_dir: Option<PathBuf>,
    #[arg(
        long,
        value_enum,
        default_value_t = RunnerArg::Auto,
        help = "auto: pg_dump nativo se compatível, senão Docker"
    )]
    runner: RunnerArg,
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..), help = "Tempo máximo do backup em segundos (padrão: sem limite)")]
    timeout: Option<u64>,
}

enum Failure {
    Canceled,
    Error(String),
}

struct Completed {
    artifact: StoredArtifact,
}

pub async fn run(args: BackupArgs) -> ExitCode {
    let config = match args.connection.config() {
        Ok(config) => config,
        Err(code) => return code,
    };
    let (password, secret_ref) = match read_password(&config) {
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
    let tools = match PgTools::detect(args.runner.into(), server).await {
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
    let output_dir = args
        .output_dir
        .unwrap_or_else(|| default_output_dir(&history_path));
    let target = match LocalTarget::new(output_dir).await {
        Ok(target) => target,
        Err(error) => return fail(exit::FAILURE, &describe(&error)),
    };

    let execution_id = match start_execution(&history, &config, secret_ref) {
        Ok(id) => id,
        Err(error) => return fail(exit::FAILURE, &describe(&error)),
    };

    let started = Instant::now();
    let timeout = args.timeout.map(Duration::from_secs);
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
            println!(
                "Backup concluído: {} ({}, sha256 {}…) em {:.1}s",
                artifact.path.display(),
                human_size(artifact.size_bytes),
                &artifact.sha256[..12],
                started.elapsed().as_secs_f64()
            );
            ExitCode::SUCCESS
        }
        Err(Failure::Canceled) => {
            let message = "backup cancelado pelo usuário";
            let _ =
                history.finish_execution(&execution_id, ExecutionStatus::Canceled, Some(message));
            fail(exit::CANCELED, message)
        }
        Err(Failure::Error(message)) => {
            let _ =
                history.finish_execution(&execution_id, ExecutionStatus::Failed, Some(&message));
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
    let file_name = backup_file_name(config.database(), OffsetDateTime::now_utc());
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
        format: FORMAT,
        sha256: &artifact.sha256,
        size_bytes: artifact.size_bytes,
        database: config.database(),
        host: config.host(),
        port: config.port(),
        server_version,
        pg_dump_version: &pg_dump_version,
        runner: tools.kind().as_str(),
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
        engine: "postgres",
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

fn human_size(bytes: u64) -> String {
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
