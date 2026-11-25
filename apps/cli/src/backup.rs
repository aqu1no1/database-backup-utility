use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::Args;
use database_backup_adapters::mysql::Flavor;
use database_backup_core::DatabaseEngine;
use database_backup_operations::Progress;
use database_backup_operations::postgres::backup::{BackupRequest, prepare};
use database_backup_storage::{Encoding, LocalTarget};
use serde::Serialize;

use crate::output::{fail, success};
use crate::{
    AUTH_DATABASE_ONLY_MONGODB, ConnectionArgs, EngineArg, RunnerArg, describe, exit, fail_with,
    interrupt, read_password,
};

#[derive(Debug, Default, Args)]
pub struct BackupArgs {
    #[arg(short, long, value_enum, help = "SGBD de origem")]
    pub(crate) engine: Option<EngineArg>,
    #[arg(
        value_name = "ALVO",
        conflicts_with_all = ["profile", "source"],
        help = "Arquivo SQLite de origem ou perfil de conexão salvo"
    )]
    pub(crate) name: Option<String>,
    #[arg(
        long,
        value_name = "NOME",
        conflicts_with = "source",
        help = "Perfil de conexão salvo"
    )]
    pub(crate) profile: Option<String>,
    #[arg(
        long,
        value_name = "ARQUIVO",
        conflicts_with_all = ["host", "port", "database", "user", "tls", "auth_database", "password_file", "runner"],
        help = "Arquivo do banco SQLite de origem (o mesmo que o ALVO de um arquivo)"
    )]
    pub(crate) source: Option<PathBuf>,
    #[command(flatten)]
    pub(crate) server: crate::profiles::ConnectionOptions,
    #[arg(
        short,
        long,
        help = "Diretório de destino (padrão: backups/ na pasta de dados do aplicativo)"
    )]
    pub(crate) output_dir: Option<PathBuf>,
    #[arg(
        long,
        value_enum,
        help = "auto (padrão): pg_dump/mysqldump/mongodump nativo se compatível, senão Docker"
    )]
    pub(crate) runner: Option<RunnerArg>,
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..), help = "Tempo máximo do backup em segundos (padrão: sem limite)")]
    pub(crate) timeout: Option<u64>,
    #[command(flatten)]
    pub(crate) encoding: crate::sealing::EncodingArgs,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topology: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compression: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encryption: Option<&'static str>,
}

pub struct Destination<'a> {
    pub target: &'a LocalTarget,
    pub encoding: &'a Encoding,
}

pub async fn run(args: BackupArgs) -> ExitCode {
    let BackupArgs {
        engine,
        name,
        profile,
        source,
        server,
        output_dir,
        runner,
        timeout,
        encoding,
    } = args;
    let (engine, source, profile) = match name {
        Some(target) => match resolve_target(target, engine, &server, runner) {
            Ok(Target::File(path)) => (Some(EngineArg::Sqlite), Some(path), None),
            Ok(Target::Profile(name)) => (engine, None, Some(name)),
            Err(code) => return code,
        },
        None => (engine, source, profile),
    };
    let runner = runner.unwrap_or(RunnerArg::Auto);
    let profile = match crate::profiles::load(profile.as_deref()) {
        Ok(profile) => profile,
        Err(code) => return code,
    };
    let profile_engine = match profile
        .as_ref()
        .map(crate::profiles::profile_engine)
        .transpose()
    {
        Ok(engine) => engine,
        Err(code) => return code,
    };
    let engine = engine.or(profile_engine).unwrap_or(EngineArg::Postgres);
    let encoding = match encoding.resolve(DatabaseEngine::from(engine)) {
        Ok(encoding) => encoding,
        Err(code) => return code,
    };
    match (engine, source) {
        (EngineArg::Sqlite, _) if server.auth_database.is_some() => {
            fail(exit::INVALID_INPUT, AUTH_DATABASE_ONLY_MONGODB)
        }
        (EngineArg::Sqlite, Some(source)) => {
            crate::sqlite::backup(&source, output_dir, timeout, &encoding).await
        }
        (EngineArg::Sqlite, None) => fail(
            exit::INVALID_INPUT,
            "--source (ou o arquivo como ALVO) é obrigatório com --engine sqlite",
        ),
        (
            EngineArg::Postgres | EngineArg::Mysql | EngineArg::Mariadb | EngineArg::Mongodb,
            Some(_),
        ) => fail(exit::INVALID_INPUT, "--source só vale com --engine sqlite"),
        (engine, None) => {
            let connection = match server.resolve(Some(engine), profile.as_ref()) {
                Ok((_, connection)) => connection,
                Err(code) => return code,
            };
            match engine {
                EngineArg::Mongodb => {
                    crate::mongo::backup(connection, output_dir, runner, timeout, &encoding).await
                }
                EngineArg::Mysql => {
                    crate::mysql::backup(
                        Flavor::Mysql,
                        connection,
                        output_dir,
                        runner,
                        timeout,
                        &encoding,
                    )
                    .await
                }
                EngineArg::Mariadb => {
                    crate::mysql::backup(
                        Flavor::Mariadb,
                        connection,
                        output_dir,
                        runner,
                        timeout,
                        &encoding,
                    )
                    .await
                }
                EngineArg::Postgres | EngineArg::Sqlite => {
                    run_postgres(connection, output_dir, runner, timeout, &encoding).await
                }
            }
        }
    }
}

enum Target {
    File(PathBuf),
    Profile(String),
}

fn resolve_target(
    target: String,
    engine: Option<EngineArg>,
    server: &crate::profiles::ConnectionOptions,
    runner: Option<RunnerArg>,
) -> Result<Target, ExitCode> {
    let path = PathBuf::from(&target);
    let looks_like_path = path.components().count() > 1 || path.is_absolute();
    let is_file = std::fs::symlink_metadata(&path).is_ok_and(|metadata| !metadata.is_dir());
    let is_source_file = match engine {
        Some(EngineArg::Sqlite) => true,
        Some(_) if looks_like_path => {
            return Err(fail(
                exit::INVALID_INPUT,
                &format!(
                    "{target} é um caminho de arquivo; só backups SQLite usam um arquivo de origem"
                ),
            ));
        }
        Some(_) => false,
        None if looks_like_path => true,
        None if is_file && crate::profiles::exists(&target)? => {
            return Err(fail(
                exit::INVALID_INPUT,
                &format!(
                    "{target} é um arquivo e também um perfil salvo; use --source {target} ou --profile {target}"
                ),
            ));
        }
        None => is_file,
    };
    if !is_source_file {
        return Ok(Target::Profile(target));
    }
    if !server.is_empty() || runner.is_some() {
        return Err(fail(
            exit::INVALID_INPUT,
            "opções de conexão e --runner não valem no backup de um arquivo SQLite",
        ));
    }
    Ok(Target::File(path))
}

async fn run_postgres(
    connection: ConnectionArgs,
    output_dir: Option<PathBuf>,
    runner: RunnerArg,
    timeout: Option<u64>,
    encoding: &Encoding,
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

    let prepared = match prepare(BackupRequest {
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
    success(
        &BackupResult {
            execution_id: &outcome.execution_id,
            database: &outcome.database,
            file: artifact.path.display().to_string(),
            size_bytes: artifact.size_bytes,
            sha256: &artifact.sha256,
            duration_ms: outcome.elapsed.as_millis(),
            non_transactional_tables: None,
            topology: None,
            compression: crate::sealing::compression_name(artifact),
            encryption: crate::sealing::encryption_name(artifact),
        },
        &text,
    )
}

pub async fn open_target(
    output_dir: Option<PathBuf>,
    history_path: &Path,
) -> Result<LocalTarget, ExitCode> {
    database_backup_operations::open_target(output_dir, history_path)
        .await
        .map_err(|error| fail(exit::FAILURE, &describe(&error)))
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
