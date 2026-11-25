use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use database_backup_adapters::postgres::{
    Admin, ArchiveInfo, DatabaseState, PgTools, PgVersion, RunnerChoice, Timeouts, ToolError,
    derived_name, test_connection,
};
use database_backup_core::{
    DatabaseEngine, Password, PostgresConnectionConfig, SecretProvider, TlsMode,
};
use database_backup_history::{ExecutionStatus, History, NewRestore, RestoreMode};
use database_backup_storage::check_restore_source;
use tracing::{info, warn};

use crate::archive::{VerifiedSource, check_target, read_archive, verify_checksum};
use crate::{Cancellation, ErrorCode, OperationError, describe, timestamp};

const MAINTENANCE_DATABASE: &str = "postgres";

#[derive(Debug)]
pub struct RestoreRequest {
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
    pub mode: RestoreMode,
    pub previous_database: Option<String>,
    pub objects: usize,
    pub checksum_verified: bool,
    pub elapsed: Duration,
}

pub struct PreparedRestore {
    request: RestoreRequest,
    target: PostgresConnectionConfig,
    password: Password,
    tools: PgTools,
    admin: Admin,
    archive: ArchiveInfo,
    verified: VerifiedSource,
    history: History,
    state: DatabaseState,
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

pub async fn prepare(
    request: RestoreRequest,
    secrets: &(dyn SecretProvider + Sync),
) -> Result<PreparedRestore, OperationError> {
    check_restore_source(&request.file)
        .await
        .map_err(|error| OperationError::source_file(&error))?;
    let server = PostgresConnectionConfig::new(
        &request.host,
        request.port,
        MAINTENANCE_DATABASE,
        &request.user,
        request.tls,
    )
    .map_err(|error| OperationError::new(ErrorCode::InvalidInput, error.to_string()))?;
    let target = server
        .with_database(&request.target_database)
        .map_err(|error| OperationError::new(ErrorCode::InvalidInput, error.to_string()))?;
    let history = History::default_path()
        .and_then(|path| History::open(&path))
        .map_err(|error| OperationError::internal(&error))?;
    let verified = verify_checksum(
        &request.file,
        &request.source,
        &history,
        request.allow_unverified,
        DatabaseEngine::Postgres,
    )
    .await?;
    if !verified.matched_reference {
        warn!(
            operation = "restore.run",
            "checksum não verificado (--allow-unverified); a integridade do arquivo não foi confirmada"
        );
    }
    let (password, _) = secrets
        .password(&target)
        .map_err(|error| OperationError::from_source(ErrorCode::InvalidInput, &error))?;
    let info = test_connection(&server, &password, Timeouts::default())
        .await
        .map_err(|error| OperationError::connection(&error))?;
    let tools = super::detect_tools(request.runner, &info.server_version).await?;
    let archive = read_archive(&tools, &request.file).await?;
    if let Some(version) = PgVersion::parse(&info.server_version) {
        check_target(&archive, version)?;
    }
    let admin = Admin::new(&server, &password, Timeouts::default());
    let state = admin
        .database_state(target.database())
        .await
        .map_err(|error| OperationError::from_source(ErrorCode::RestoreFailed, &error))?;
    Ok(PreparedRestore {
        request,
        target,
        password,
        tools,
        admin,
        archive,
        verified,
        history,
        state,
    })
}

impl PreparedRestore {
    #[must_use]
    pub fn database_state(&self) -> DatabaseState {
        self.state
    }

    pub async fn run(
        self,
        confirmation: Option<&str>,
        cancellation: &Cancellation,
    ) -> Result<RestoreOutcome, OperationError> {
        let stamp = timestamp();
        let state = self
            .admin
            .database_state(self.target.database())
            .await
            .map_err(|error| OperationError::from_source(ErrorCode::RestoreFailed, &error))?;
        let plan = plan_target(state, self.target.database(), confirmation, &stamp)?;
        let file_location = self.request.source.display().to_string();
        let restore_id = self
            .history
            .start_restore(&NewRestore {
                artifact_id: self.verified.artifact_id.as_deref(),
                source_location: &file_location,
                source_sha256: Some(&self.verified.sha256),
                checksum_verified: self.verified.matched_reference,
                engine: DatabaseEngine::Postgres.as_str(),
                target_host: Some(self.target.host()),
                target_port: Some(self.target.port()),
                target_database: self.target.database(),
                mode: plan.mode,
            })
            .map_err(|error| OperationError::internal(&error))?;
        info!(
            operation = "restore.run",
            restore_id = %restore_id,
            target = %self.target,
            mode = plan.mode.as_str(),
            status = "running",
            "restore iniciado"
        );
        let started = Instant::now();
        let outcome = execute(&Job {
            tools: &self.tools,
            admin: &self.admin,
            target: &self.target,
            password: &self.password,
            file: &self.request.file,
            archive: &self.archive,
            plan: &plan,
            stamp: &stamp,
            timeout: self.request.timeout,
            cancellation,
        })
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
                    database: self.target.database().to_owned(),
                    mode: plan.mode,
                    previous_database: previous,
                    objects: self.archive.entries,
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

fn plan_target(
    state: DatabaseState,
    name: &str,
    confirmation: Option<&str>,
    stamp: &str,
) -> Result<Plan, OperationError> {
    match state {
        DatabaseState::Missing => Ok(Plan {
            mode: RestoreMode::Create,
            restore_into: name.to_owned(),
            created_by_us: true,
        }),
        DatabaseState::Empty => Ok(Plan {
            mode: RestoreMode::Create,
            restore_into: name.to_owned(),
            created_by_us: false,
        }),
        DatabaseState::HasData { .. } if confirmation == Some(name) => Ok(Plan {
            mode: RestoreMode::Overwrite,
            restore_into: derived_name(name, &format!("__restore_{stamp}")),
            created_by_us: true,
        }),
        DatabaseState::HasData { .. } => Err(OperationError::new(
            ErrorCode::ConfirmationRequired,
            format!("confirmação não confere com {name}; nada foi alterado"),
        )),
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
    cancellation: &'a Cancellation,
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
        Done(Result<(), ToolError>),
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
        () = job.cancellation.canceled() => Waited::Interrupted,
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

#[cfg(test)]
mod tests {
    use database_backup_adapters::postgres::DatabaseState;
    use database_backup_history::RestoreMode;

    use super::plan_target;
    use crate::ErrorCode;

    #[test]
    fn a_database_with_data_requires_its_exact_name() {
        let state = DatabaseState::HasData { tables: 1 };

        for confirmation in [None, Some("other"), Some("APP"), Some("app ")] {
            let error = plan_target(state, "app", confirmation, "stamp").err();
            assert_eq!(
                error.map(|error| error.code()),
                Some(ErrorCode::ConfirmationRequired)
            );
        }
        let plan = plan_target(state, "app", Some("app"), "stamp").ok();
        assert_eq!(plan.map(|plan| plan.mode), Some(RestoreMode::Overwrite));
    }
}
