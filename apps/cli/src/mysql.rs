use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use database_backup_adapters::mysql::{Flavor, Timeouts, test_connection};
use database_backup_core::{MysqlConnectionConfig, Operation};
use database_backup_history::RestoreMode;
use database_backup_operations::Progress;
use database_backup_operations::mysql::{
    self as operation, BackupRequest, engine, prepare_backup, prepare_restore, wrong_engine,
};
use database_backup_storage::Encoding;
use tracing::info;

use crate::backup::{BackupResult, human_size};
use crate::credentials::CliSecrets;
use crate::output::{fail, success};
use crate::restore::RestoreResult;
use crate::{
    ConnectionArgs, ConnectionResult, MYSQL_PORT, RunnerArg, TlsArg, exit, exit_code_for,
    fail_with, interrupt, read_password, unsupported,
};

pub struct RestoreRequest<'a> {
    pub flavor: Flavor,
    pub file: &'a Path,
    pub source: &'a Path,
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
    pub keyring: Option<String>,
}

pub async fn test(
    flavor: Flavor,
    connection: &ConnectionArgs,
    timeout: u64,
    report: bool,
) -> ExitCode {
    let engine = engine(flavor);
    let config = match config_from(connection) {
        Ok(config) => config,
        Err(code) => return code,
    };
    let password = match read_password(
        engine,
        &config,
        connection.password_file.clone(),
        connection.keyring.clone(),
    ) {
        Ok((password, _)) => password,
        Err(message) => return fail(exit::INVALID_INPUT, &message),
    };
    let timeout = Duration::from_secs(timeout);
    let timeouts = Timeouts {
        connect: timeout,
        query: timeout,
    };
    match test_connection(&config, &password, timeouts).await {
        Ok(info) if Flavor::of(&info.server_version) != flavor => fail(
            exit::INVALID_INPUT,
            &wrong_engine(flavor, &info.server_version),
        ),
        Ok(_) if !report => ExitCode::SUCCESS,
        Ok(info) => {
            info!(
                operation = "connection.test",
                engine = engine.as_str(),
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
                &format!("Conexão OK: {flavor} {} em {}", info.server_version, config),
            )
        }
        Err(error) => {
            info!(operation = "connection.test", engine = engine.as_str(), connection = %config, status = "failed", "falha ao testar conexão");
            fail(exit_code_for(&error), &error.to_string())
        }
    }
}

pub async fn backup(
    flavor: Flavor,
    connection: ConnectionArgs,
    output_dir: Option<PathBuf>,
    runner: RunnerArg,
    timeout: Option<u64>,
    encoding: &Encoding,
) -> ExitCode {
    let engine = engine(flavor);
    let config = match config_from(&connection) {
        Ok(config) => config,
        Err(code) => return code,
    };
    let (password, source) = match read_password(
        engine,
        &config,
        connection.password_file.clone(),
        connection.keyring.clone(),
    ) {
        Ok(value) => value,
        Err(message) => return fail(exit::INVALID_INPUT, &message),
    };
    let prepared = match prepare_backup(BackupRequest {
        flavor,
        config,
        password,
        secret_ref: source.secret_ref(),
        output_dir,
        runner: runner.into(),
        timeout: timeout.map(Duration::from_secs),
        encoding: encoding.clone(),
    })
    .await
    {
        Ok(prepared) => prepared,
        Err(error) => return fail_with(&error),
    };
    let outcome = match prepared.run(&interrupt(), &|_: Progress| {}).await {
        Ok(outcome) => outcome,
        Err(error) => return fail_with(&error),
    };
    let artifact = &outcome.artifact;
    let mut text = format!(
        "Backup concluído: {} ({}, sha256 {}…) em {:.1}s",
        artifact.path.display(),
        human_size(artifact.size_bytes),
        &artifact.sha256[..12],
        outcome.elapsed.as_secs_f64()
    );
    if let Some(summary) = crate::sealing::summary(artifact) {
        text.push_str(&format!("\n{summary}"));
    }
    if let Some(tables) = outcome
        .non_transactional_tables
        .as_deref()
        .filter(|t| !t.is_empty())
    {
        text.push_str(&format!(
            "\nAtenção: {} tabela(s) fora do InnoDB, sem snapshot consistente: {}",
            tables.len(),
            tables.join(", ")
        ));
    }
    success(
        &BackupResult {
            execution_id: &outcome.execution_id,
            database: &outcome.database,
            file: artifact.path.display().to_string(),
            size_bytes: artifact.size_bytes,
            sha256: &artifact.sha256,
            duration_ms: outcome.elapsed.as_millis(),
            non_transactional_tables: outcome.non_transactional_tables.as_deref(),
            topology: None,
            compression: crate::sealing::compression_name(artifact),
            encryption: crate::sealing::encryption_name(artifact),
        },
        &text,
    )
}

pub async fn restore(request: &RestoreRequest<'_>) -> ExitCode {
    let flavor = request.flavor;
    let engine = engine(flavor);
    if request.overwrite {
        return unsupported(
            engine,
            Operation::RestoreOverwrite,
            "restaure em um banco que não existe (--target-database)",
        );
    }
    let file = match std::path::absolute(request.file) {
        Ok(path) => path,
        Err(error) => return fail(exit::INVALID_ARCHIVE, &error.to_string()),
    };
    let secrets = CliSecrets {
        password_file: request.password_file.clone(),
        keyring: request.keyring.clone(),
        engine,
    };
    let prepared = match prepare_restore(
        operation::RestoreRequest {
            flavor,
            file,
            source: request.source.to_owned(),
            host: request.host.to_owned(),
            port: request.port.unwrap_or(MYSQL_PORT),
            user: request.user.to_owned(),
            tls: request.tls.into(),
            target_database: request.target_database.to_owned(),
            runner: request.runner.into(),
            allow_unverified: request.allow_unverified,
            timeout: request.timeout.map(Duration::from_secs),
        },
        &secrets,
    )
    .await
    {
        Ok(prepared) => prepared,
        Err(error) => return fail_with(&error),
    };
    let outcome = match prepared.run(&interrupt()).await {
        Ok(outcome) => outcome,
        Err(error) => return fail_with(&error),
    };
    success(
        &RestoreResult {
            restore_id: &outcome.restore_id,
            database: &outcome.database,
            mode: RestoreMode::Create.as_str(),
            previous_database: None,
            objects: outcome.objects,
            checksum_verified: outcome.checksum_verified,
            duration_ms: outcome.elapsed.as_millis(),
        },
        &format!(
            "Restore concluído em {} ({} objetos, {} tabelas) em {:.1}s",
            outcome.database,
            outcome.objects,
            outcome.tables,
            outcome.elapsed.as_secs_f64()
        ),
    )
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
