use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use clap::Args;
use database_backup_adapters::postgres::{
    Admin, ArchiveInfo, DatabaseState, PgTools, PgVersion, Timeouts, derived_name, test_connection,
};
use database_backup_core::{Password, PostgresConnectionConfig};
use database_backup_history::{ExecutionStatus, History, NewRestore, RestoreMode};
use database_backup_storage::{read_metadata, sha256_file};
use time::OffsetDateTime;
use time::macros::format_description;

use crate::{RunnerArg, TlsArg, describe, exit, exit_code_for, fail, read_password};

const MAINTENANCE_DATABASE: &str = "postgres";

#[derive(Debug, Args)]
pub struct RestoreArgs {
    #[arg(long, help = "Arquivo .dump gerado pelo comando backup")]
    file: PathBuf,
    #[arg(long, help = "Banco de destino")]
    target_database: String,
    #[arg(
        long,
        default_value = "localhost",
        help = "Host do servidor de destino"
    )]
    host: String,
    #[arg(long, default_value_t = 5432, help = "Porta do servidor de destino")]
    port: u16,
    #[arg(long, help = "Usuário")]
    user: String,
    #[arg(
        long,
        value_enum,
        default_value_t = TlsArg::Auto,
        help = "auto: sem TLS só em localhost; verify-full: TLS com certificado validado"
    )]
    tls: TlsArg,
    #[arg(
        long,
        value_enum,
        default_value_t = RunnerArg::Auto,
        help = "auto: pg_restore nativo se compatível, senão Docker"
    )]
    runner: RunnerArg,
    #[arg(
        long,
        help = "Permite substituir um banco que já tem dados (o anterior é mantido)"
    )]
    overwrite: bool,
    #[arg(
        long,
        value_name = "NOME",
        help = "Confirma a sobrescrita repetindo o nome do banco de destino"
    )]
    confirm: Option<String>,
    #[arg(
        long,
        help = "Aceita arquivos sem checksum conhecido (.meta.json ou histórico)"
    )]
    allow_unverified: bool,
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..), help = "Tempo máximo do restore em segundos (padrão: sem limite)")]
    timeout: Option<u64>,
}

struct Plan {
    mode: RestoreMode,
    restore_into: String,
    created_by_us: bool,
}

enum Outcome {
    Succeeded { previous: Option<String> },
    Canceled,
    Failed(String),
}

pub async fn run(args: RestoreArgs) -> ExitCode {
    let file = match std::path::absolute(&args.file) {
        Ok(path) => path,
        Err(error) => return fail(exit::INVALID_ARCHIVE, &error.to_string()),
    };
    if !file.is_file() {
        return fail(
            exit::INVALID_ARCHIVE,
            &format!("arquivo de backup não encontrado: {}", file.display()),
        );
    }

    let server = match PostgresConnectionConfig::new(
        &args.host,
        args.port,
        MAINTENANCE_DATABASE,
        &args.user,
        args.tls.into(),
    ) {
        Ok(config) => config,
        Err(error) => return fail(exit::INVALID_INPUT, &error.to_string()),
    };
    let target = match server.with_database(&args.target_database) {
        Ok(config) => config,
        Err(error) => return fail(exit::INVALID_INPUT, &error.to_string()),
    };

    let history = match History::default_path().and_then(|path| History::open(&path)) {
        Ok(history) => history,
        Err(error) => return fail(exit::FAILURE, &describe(&error)),
    };
    let (artifact_id, checksum) =
        match verify_checksum(&file, &history, args.allow_unverified).await {
            Ok(value) => value,
            Err(message) => return fail(exit::INVALID_ARCHIVE, &message),
        };

    let (password, _) = match read_password(&target) {
        Ok(value) => value,
        Err(message) => return fail(exit::INVALID_INPUT, &message),
    };
    let info = match test_connection(&server, &password, Timeouts::default()).await {
        Ok(info) => info,
        Err(error) => return fail(exit_code_for(&error), &error.to_string()),
    };
    let Some(server_version) = PgVersion::parse(&info.server_version) else {
        return fail(
            exit::FAILURE,
            &format!(
                "versão do servidor não reconhecida: {}",
                info.server_version
            ),
        );
    };
    let tools = match PgTools::detect(args.runner.into(), server_version).await {
        Ok(tools) => tools,
        Err(error) => return fail(exit::TOOLS, &error.to_string()),
    };
    let archive = match inspect_archive(&tools, &file, server_version).await {
        Ok(archive) => archive,
        Err(message) => return fail(exit::INVALID_ARCHIVE, &message),
    };

    let admin = Admin::new(&server, &password, Timeouts::default());
    let stamp = timestamp();
    let plan = match plan_target(&admin, &args, &stamp).await {
        Ok(plan) => plan,
        Err(code) => return code,
    };

    let file_location = file.display().to_string();
    let restore_id = match history.start_restore(&NewRestore {
        artifact_id: artifact_id.as_deref(),
        source_location: &file_location,
        source_sha256: Some(&checksum),
        target_host: target.host(),
        target_port: target.port(),
        target_database: target.database(),
        mode: plan.mode,
    }) {
        Ok(id) => id,
        Err(error) => return fail(exit::FAILURE, &describe(&error)),
    };

    let started = Instant::now();
    let outcome = execute(&Job {
        tools: &tools,
        admin: &admin,
        target: &target,
        password: &password,
        file: &file,
        archive: &archive,
        plan: &plan,
        stamp: &stamp,
        timeout: args.timeout.map(Duration::from_secs),
    })
    .await;

    match outcome {
        Outcome::Succeeded { previous } => {
            let _ = history.finish_restore(
                &restore_id,
                ExecutionStatus::Succeeded,
                previous.as_deref(),
                None,
            );
            println!(
                "Restore concluído em {} ({} objetos) em {:.1}s",
                target.database(),
                archive.entries,
                started.elapsed().as_secs_f64()
            );
            if let Some(previous) = previous {
                println!("Banco anterior mantido como {previous}");
            }
            ExitCode::SUCCESS
        }
        Outcome::Canceled => {
            let message = "restore cancelado; o destino não foi alterado";
            let _ =
                history.finish_restore(&restore_id, ExecutionStatus::Canceled, None, Some(message));
            fail(exit::CANCELED, message)
        }
        Outcome::Failed(message) => {
            let _ =
                history.finish_restore(&restore_id, ExecutionStatus::Failed, None, Some(&message));
            fail(exit::RESTORE_FAILED, &message)
        }
    }
}

async fn verify_checksum(
    file: &Path,
    history: &History,
    allow_unverified: bool,
) -> Result<(Option<String>, String), String> {
    let metadata = read_metadata(file)
        .await
        .map_err(|error| describe(&error))?;
    let artifact = history
        .find_artifact_by_location(&file.display().to_string())
        .map_err(|error| describe(&error))?;
    let expected = metadata
        .map(|metadata| metadata.sha256)
        .or_else(|| artifact.as_ref().and_then(|a| a.checksum_sha256.clone()));

    let (_, actual) = sha256_file(file).await.map_err(|error| describe(&error))?;
    match expected {
        Some(expected) if expected != actual => Err(format!(
            "checksum divergente: esperado {expected}, calculado {actual}; o arquivo foi alterado ou corrompido"
        )),
        Some(_) => Ok((artifact.map(|a| a.id), actual)),
        None if allow_unverified => Ok((None, actual)),
        None => Err(
            "não há checksum conhecido para este arquivo (.meta.json ou histórico); use --allow-unverified para restaurar mesmo assim"
                .to_owned(),
        ),
    }
}

async fn inspect_archive(
    tools: &PgTools,
    file: &Path,
    server: PgVersion,
) -> Result<ArchiveInfo, String> {
    let archive = tools
        .restore_list(file)
        .await
        .map_err(|error| format!("o arquivo não é um backup válido do pg_dump: {error}"))?;
    if let Some(from) = archive.dumped_from
        && from.major > server.major
    {
        return Err(format!(
            "o backup veio do PostgreSQL {from}, mais novo que o servidor de destino {server}"
        ));
    }
    if let Some(by) = archive.dumped_by
        && by.major > tools.version().major
    {
        return Err(format!(
            "o backup foi gerado pelo pg_dump {by}, mais novo que o pg_restore {} disponível",
            tools.version()
        ));
    }
    Ok(archive)
}

async fn plan_target(admin: &Admin, args: &RestoreArgs, stamp: &str) -> Result<Plan, ExitCode> {
    let name = &args.target_database;
    let state = admin
        .database_state(name)
        .await
        .map_err(|error| fail(exit::RESTORE_FAILED, &describe(&error)))?;
    match state {
        DatabaseState::Missing => Ok(Plan {
            mode: RestoreMode::Create,
            restore_into: name.clone(),
            created_by_us: true,
        }),
        DatabaseState::Empty => Ok(Plan {
            mode: RestoreMode::Create,
            restore_into: name.clone(),
            created_by_us: false,
        }),
        DatabaseState::HasData { tables } => {
            if !args.overwrite {
                return Err(fail(
                    exit::CONFIRMATION_REQUIRED,
                    &format!(
                        "o banco {name} já existe e tem {tables} tabela(s); use --overwrite para substituí-lo (o atual será mantido como {name}__previous_<data>)"
                    ),
                ));
            }
            confirm_overwrite(name, tables, args.confirm.as_deref())?;
            Ok(Plan {
                mode: RestoreMode::Overwrite,
                restore_into: derived_name(name, &format!("__restore_{stamp}")),
                created_by_us: true,
            })
        }
    }
}

fn confirm_overwrite(name: &str, tables: i64, confirm: Option<&str>) -> Result<(), ExitCode> {
    let typed = match confirm {
        Some(value) => value.to_owned(),
        None if std::io::stdin().is_terminal() => {
            eprint!(
                "O banco {name} tem {tables} tabela(s) e será substituído; as conexões ativas nele serão encerradas.\nDigite o nome do banco para confirmar: "
            );
            let _ = std::io::stderr().flush();
            let mut line = String::new();
            if std::io::stdin().lock().read_line(&mut line).is_err() {
                return Err(fail(exit::CONFIRMATION_REQUIRED, "confirmação não lida"));
            }
            line.trim().to_owned()
        }
        None => {
            return Err(fail(
                exit::CONFIRMATION_REQUIRED,
                &format!(
                    "sobrescrever {name} exige --confirm {name} quando não há terminal interativo"
                ),
            ));
        }
    };
    if typed == name {
        Ok(())
    } else {
        Err(fail(
            exit::CONFIRMATION_REQUIRED,
            &format!("confirmação não confere com {name}; nada foi alterado"),
        ))
    }
}

struct Job<'a> {
    tools: &'a PgTools,
    admin: &'a Admin,
    target: &'a PostgresConnectionConfig,
    password: &'a Password,
    file: &'a Path,
    archive: &'a ArchiveInfo,
    plan: &'a Plan,
    stamp: &'a str,
    timeout: Option<Duration>,
}

async fn execute(job: &Job<'_>) -> Outcome {
    if job.plan.created_by_us
        && let Err(error) = job.admin.create_database(&job.plan.restore_into).await
    {
        return Outcome::Failed(describe(&error));
    }

    let outcome = run_pg_restore(job).await;
    if !matches!(outcome, Outcome::Succeeded { .. }) {
        if job.plan.created_by_us {
            let _ = job.admin.drop_database(&job.plan.restore_into).await;
        }
        return outcome;
    }

    if job.plan.mode == RestoreMode::Create {
        return Outcome::Succeeded { previous: None };
    }
    swap(
        job.admin,
        job.target.database(),
        &job.plan.restore_into,
        job.stamp,
    )
    .await
}

async fn run_pg_restore(job: &Job<'_>) -> Outcome {
    let config = match job.target.with_database(&job.plan.restore_into) {
        Ok(config) => config,
        Err(error) => return Outcome::Failed(error.to_string()),
    };
    let mut process = match job.tools.restore(&config, job.password, job.file) {
        Ok(process) => process,
        Err(error) => return Outcome::Failed(error.to_string()),
    };

    enum Waited {
        Done(Result<(), database_backup_adapters::postgres::ToolError>),
        Interrupted,
        TimedOut,
    }
    let timeout = job.timeout;
    let deadline = async {
        match timeout {
            Some(limit) => tokio::time::sleep(limit).await,
            None => std::future::pending().await,
        }
    };
    let waited = tokio::select! {
        result = process.wait() => Waited::Done(result),
        _ = tokio::signal::ctrl_c() => Waited::Interrupted,
        () = deadline => Waited::TimedOut,
    };
    match waited {
        Waited::Done(Ok(())) => {}
        Waited::Done(Err(error)) => return Outcome::Failed(error.to_string()),
        Waited::Interrupted => {
            process.cancel().await;
            return Outcome::Canceled;
        }
        Waited::TimedOut => {
            process.cancel().await;
            return Outcome::Failed(format!(
                "tempo esgotado após {}s",
                timeout.map_or(0, |limit| limit.as_secs())
            ));
        }
    }

    match job.admin.count_tables(&job.plan.restore_into).await {
        Ok(0) if job.archive.tables > 0 => Outcome::Failed(format!(
            "o pg_restore terminou, mas {} ficou sem tabelas",
            job.plan.restore_into
        )),
        Ok(_) => Outcome::Succeeded { previous: None },
        Err(error) => Outcome::Failed(describe(&error)),
    }
}

async fn swap(admin: &Admin, target: &str, restored: &str, stamp: &str) -> Outcome {
    let previous = derived_name(target, &format!("__previous_{stamp}"));
    if let Err(error) = admin.terminate_connections(target).await {
        let _ = admin.drop_database(restored).await;
        return Outcome::Failed(describe(&error));
    }
    if let Err(error) = admin.rename_database(target, &previous).await {
        let _ = admin.drop_database(restored).await;
        return Outcome::Failed(format!(
            "{}; o banco {target} não foi alterado",
            describe(&error)
        ));
    }
    if let Err(error) = admin.rename_database(restored, target).await {
        let rollback = admin.rename_database(&previous, target).await;
        let state = match rollback {
            Ok(()) => format!("o banco {target} foi restaurado ao estado anterior"),
            Err(_) => {
                format!("ATENÇÃO: o banco original está em {previous} e o restaurado em {restored}")
            }
        };
        return Outcome::Failed(format!("{}; {state}", describe(&error)));
    }
    Outcome::Succeeded {
        previous: Some(previous),
    }
}

fn timestamp() -> String {
    OffsetDateTime::now_utc()
        .format(format_description!(
            "[year][month][day]T[hour][minute][second]Z"
        ))
        .unwrap_or_else(|_| OffsetDateTime::now_utc().unix_timestamp().to_string())
}
