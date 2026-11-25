use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use database_backup_adapters::mongo::{Timeouts, test_connection};
use database_backup_core::{DatabaseEngine, MongoConnectionConfig, Operation};
use database_backup_history::RestoreMode;
use database_backup_operations::Progress;
use database_backup_operations::mongo::{
    self as operation, BackupRequest, DEFAULT_AUTH_DATABASE, connection_message, prepare_backup,
    prepare_restore, unsupported_topology,
};
use database_backup_storage::Encoding;
use tracing::info;

use crate::backup::{BackupResult, human_size};
use crate::credentials::CliSecrets;
use crate::output::{fail, success};
use crate::restore::RestoreResult;
use crate::{
    ConnectionArgs, ConnectionResult, MONGO_PORT, RunnerArg, TlsArg, exit, exit_code_for,
    fail_with, interrupt, read_password, unsupported,
};

const ENGINE: DatabaseEngine = DatabaseEngine::Mongodb;

pub struct RestoreRequest<'a> {
    pub file: &'a Path,
    pub source: &'a Path,
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

pub async fn test(connection: &ConnectionArgs, timeout: u64, report: bool) -> ExitCode {
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
        Ok(_) if !report => ExitCode::SUCCESS,
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
    encoding: &Encoding,
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
    let prepared = match prepare_backup(BackupRequest {
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
    text.push_str(&format!("\n{}", outcome.consistency));
    success(
        &BackupResult {
            execution_id: &outcome.execution_id,
            database: &outcome.database,
            file: artifact.path.display().to_string(),
            size_bytes: artifact.size_bytes,
            sha256: &artifact.sha256,
            duration_ms: outcome.elapsed.as_millis(),
            non_transactional_tables: None,
            topology: Some(outcome.topology.as_str()),
            compression: crate::sealing::compression_name(artifact),
            encryption: crate::sealing::encryption_name(artifact),
        },
        &text,
    )
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
    let secrets = CliSecrets {
        password_file: request.password_file.clone(),
        engine: ENGINE,
    };
    let prepared = match prepare_restore(
        operation::RestoreRequest {
            file,
            source: request.source.to_owned(),
            host: request.host.to_owned(),
            port: request.port.unwrap_or(MONGO_PORT),
            user: request.user.to_owned(),
            tls: request.tls.into(),
            auth_database: request
                .auth_database
                .unwrap_or(DEFAULT_AUTH_DATABASE)
                .to_owned(),
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
            "Restore concluído em {} ({} objetos, {} coleções) em {:.1}s",
            outcome.database,
            outcome.objects,
            outcome.collections,
            outcome.elapsed.as_secs_f64()
        ),
    )
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
