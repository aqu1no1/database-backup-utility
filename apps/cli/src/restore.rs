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
use database_backup_storage::check_restore_source;
use serde::Serialize;
use time::OffsetDateTime;
use time::macros::format_description;
use tracing::{info, warn};

use crate::archive::{check_target, read_archive, verify_checksum};
use crate::output::{fail, success};
use crate::{RunnerArg, TlsArg, describe, exit, exit_code_for, read_password};

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

#[derive(Serialize)]
struct RestoreResult<'a> {
    restore_id: &'a str,
    database: &'a str,
    mode: &'static str,
    previous_database: Option<&'a str>,
    objects: usize,
    checksum_verified: bool,
    duration_ms: u128,
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
    if let Err(error) = check_restore_source(&file).await {
        return fail(exit::INVALID_ARCHIVE, &describe(&error));
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
    let verified = match verify_checksum(&file, &history, args.allow_unverified).await {
        Ok(verified) => verified,
        Err(message) => return fail(exit::INVALID_ARCHIVE, &message),
    };
    if !verified.matched_reference {
        warn!(
            operation = "restore.run",
            "checksum não verificado (--allow-unverified); a integridade do arquivo não foi confirmada"
        );
    }

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
    let archive = match read_archive(&tools, &file).await {
        Ok(archive) => archive,
        Err(message) => return fail(exit::INVALID_ARCHIVE, &message),
    };
    if let Err(message) = check_target(&archive, server_version) {
        return fail(exit::INVALID_ARCHIVE, &message);
    }

    let admin = Admin::new(&server, &password, Timeouts::default());
    let stamp = timestamp();
    let plan = match plan_target(&admin, &args, &stamp).await {
        Ok(plan) => plan,
        Err(code) => return code,
    };

    let file_location = file.display().to_string();
    let restore_id = match history.start_restore(&NewRestore {
        artifact_id: verified.artifact_id.as_deref(),
        source_location: &file_location,
        source_sha256: Some(&verified.sha256),
        checksum_verified: verified.matched_reference,
        target_host: target.host(),
        target_port: target.port(),
        target_database: target.database(),
        mode: plan.mode,
    }) {
        Ok(id) => id,
        Err(error) => return fail(exit::FAILURE, &describe(&error)),
    };

    info!(
        operation = "restore.run",
        restore_id = %restore_id,
        target = %target,
        mode = plan.mode.as_str(),
        status = "running",
        "restore iniciado"
    );
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
                target.database(),
                archive.entries,
                elapsed.as_secs_f64()
            );
            if let Some(previous) = &previous {
                text.push_str(&format!("\nBanco anterior mantido como {previous}"));
            }
            success(
                &RestoreResult {
                    restore_id: &restore_id,
                    database: target.database(),
                    mode: plan.mode.as_str(),
                    previous_database: previous.as_deref(),
                    objects: archive.entries,
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
