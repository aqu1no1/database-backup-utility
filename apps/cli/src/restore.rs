use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::Args;
use database_backup_adapters::mysql::Flavor;
use database_backup_adapters::postgres::DatabaseState;
use database_backup_core::DatabaseEngine;
use database_backup_history::ConnectionProfile;
use database_backup_operations::archive::detect_engine;
use database_backup_operations::postgres::restore::{RestoreRequest, prepare};
use database_backup_storage::check_restore_source;
use serde::Serialize;

use crate::credentials::CliSecrets;
use crate::output::{fail, success};
use crate::{
    AUTH_DATABASE_ONLY_MONGODB, EngineArg, RunnerArg, TlsArg, describe, exit, fail_with, interrupt,
};

#[derive(Debug, Args)]
pub struct RestoreArgs {
    #[arg(
        short,
        long,
        value_enum,
        help = "SGBD do backup (padrão: detectado pelo arquivo; divergente é recusado)"
    )]
    engine: Option<EngineArg>,
    #[arg(
        long,
        value_name = "NOME",
        help = "Perfil do servidor de destino; --to continua obrigatório"
    )]
    profile: Option<String>,
    #[arg(
        value_name = "ARQUIVO",
        required_unless_present = "file",
        conflicts_with = "file",
        help = "Arquivo gerado pelo backup"
    )]
    path: Option<PathBuf>,
    #[arg(
        short,
        long,
        value_name = "ARQUIVO",
        help = "O mesmo que o ARQUIVO posicional"
    )]
    file: Option<PathBuf>,
    #[arg(
        long,
        value_name = "DESTINO",
        conflicts_with_all = ["target", "target_database"],
        help = "Banco de destino (ou arquivo de destino, num backup SQLite)"
    )]
    to: Option<String>,
    #[arg(
        long,
        value_name = "ARQUIVO",
        conflicts_with_all = ["target_database", "host", "port", "user", "tls", "auth_database", "runner", "password_file"],
        help = "Arquivo SQLite de destino (num backup SQLite; o mesmo que --to)"
    )]
    target: Option<PathBuf>,
    #[arg(long, help = "Banco de destino (o mesmo que --to)")]
    target_database: Option<String>,
    #[arg(short = 'H', long, help = "Host do servidor de destino")]
    host: Option<String>,
    #[arg(
        short,
        long,
        help = "Porta do servidor de destino (padrão: 5432 no PostgreSQL, 3306 no MySQL, 27017 no MongoDB)"
    )]
    port: Option<u16>,
    #[arg(short, long, help = "Usuário")]
    user: Option<String>,
    #[arg(
        long,
        value_enum,
        help = "auto: sem TLS só em localhost; verify-full: TLS com certificado validado"
    )]
    tls: Option<TlsArg>,
    #[arg(
        long,
        value_name = "NOME",
        help = "Banco de autenticação do MongoDB (padrão: admin)"
    )]
    auth_database: Option<String>,
    #[arg(
        long,
        value_enum,
        default_value_t = RunnerArg::Auto,
        help = "auto: pg_restore/mysql/mongorestore nativo se compatível, senão Docker"
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
        help = "Confirma a sobrescrita repetindo o nome do banco (ou o caminho do arquivo SQLite) de destino"
    )]
    confirm: Option<String>,
    #[arg(
        long,
        help = "Aceita arquivos sem checksum conhecido (.meta.json ou histórico)"
    )]
    allow_unverified: bool,
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..), help = "Tempo máximo do restore em segundos (padrão: sem limite)")]
    timeout: Option<u64>,
    #[arg(
        long,
        value_name = "ARQUIVO",
        help = "Arquivo com a senha (permissão 600 ou 400)"
    )]
    password_file: Option<PathBuf>,
    #[arg(
        short,
        long,
        value_name = "ARQUIVO",
        help = "Chave privada age (dbu key generate) de um backup criptografado"
    )]
    identity: Option<PathBuf>,
    #[arg(skip)]
    source: Option<PathBuf>,
}

impl RestoreArgs {
    fn file(&self) -> &Path {
        self.file.as_deref().unwrap_or(Path::new(""))
    }

    fn source(&self) -> &Path {
        self.source.as_deref().unwrap_or_else(|| self.file())
    }
}

#[derive(Serialize)]
pub struct RestoreResult<'a> {
    pub restore_id: &'a str,
    pub database: &'a str,
    pub mode: &'static str,
    pub previous_database: Option<&'a str>,
    pub objects: usize,
    pub checksum_verified: bool,
    pub duration_ms: u128,
}

pub async fn run(mut args: RestoreArgs) -> ExitCode {
    args.file = args.path.take().or(args.file.take());
    let profile = match crate::profiles::load(args.profile.as_deref()) {
        Ok(profile) => profile,
        Err(code) => return code,
    };
    if profile.is_some() && (args.engine == Some(EngineArg::Sqlite) || args.target.is_some()) {
        return fail(exit::INVALID_INPUT, crate::profiles::SQLITE_UNSUPPORTED);
    }
    let engine = match resolve_engine(args.file(), args.engine, profile.as_ref()).await {
        Ok(engine) => engine,
        Err(code) => return code,
    };
    let original = match std::path::absolute(args.file()) {
        Ok(path) => path,
        Err(error) => return fail(exit::INVALID_ARCHIVE, &error.to_string()),
    };
    let opened =
        match crate::sealing::open(&original, args.identity.as_deref(), "restore.run").await {
            Ok(opened) => opened,
            Err(code) => return code,
        };
    args.file = Some(opened.path().to_owned());
    args.source = Some(original);
    if let Some(to) = args.to.take() {
        if engine == EngineArg::Sqlite {
            args.target = Some(PathBuf::from(to));
        } else {
            args.target_database = Some(to);
        }
    }
    if let Some(profile) = profile.as_ref() {
        let Some(database) = args.target_database.clone() else {
            return fail(
                exit::INVALID_INPUT,
                "--to (ou --target-database) é obrigatório; o perfil não define o destino do restore",
            );
        };
        let options = crate::profiles::ConnectionOptions {
            host: args.host.take(),
            port: args.port,
            database: Some(database),
            user: args.user.take(),
            tls: args.tls,
            auth_database: args.auth_database.take(),
            password_file: args.password_file.take(),
        };
        let connection = match options.resolve(Some(engine), Some(profile)) {
            Ok((_, connection)) => connection,
            Err(code) => return code,
        };
        args.host = Some(connection.host);
        args.port = connection.port;
        args.user = Some(connection.user);
        args.tls = Some(connection.tls);
        args.auth_database = connection.auth_database;
        args.password_file = connection.password_file;
    }
    if engine != EngineArg::Mongodb && args.auth_database.is_some() {
        return fail(exit::INVALID_INPUT, AUTH_DATABASE_ONLY_MONGODB);
    }
    match (engine, &args.target) {
        (EngineArg::Sqlite, Some(target)) => {
            crate::sqlite::restore(&crate::sqlite::RestoreRequest {
                file: args.file(),
                source: args.source(),
                target,
                overwrite: args.overwrite,
                confirm: args.confirm.as_deref(),
                allow_unverified: args.allow_unverified,
                timeout: args.timeout,
            })
            .await
        }
        (EngineArg::Sqlite, None) => fail(
            exit::INVALID_INPUT,
            "--to (ou --target) é obrigatório no restore de um backup SQLite",
        ),
        (
            EngineArg::Postgres | EngineArg::Mysql | EngineArg::Mariadb | EngineArg::Mongodb,
            Some(_),
        ) => fail(
            exit::INVALID_INPUT,
            "--target só vale para backups SQLite; para PostgreSQL, MySQL, MariaDB e MongoDB use --to ou --target-database",
        ),
        (EngineArg::Mongodb, None) => match (&args.target_database, &args.user) {
            (Some(target_database), Some(user)) => {
                crate::mongo::restore(&crate::mongo::RestoreRequest {
                    file: args.file(),
                    source: args.source(),
                    host: args.host.as_deref().unwrap_or("localhost"),
                    port: args.port,
                    target_database,
                    user,
                    auth_database: args.auth_database.as_deref(),
                    tls: args.tls.unwrap_or(TlsArg::Auto),
                    runner: args.runner,
                    overwrite: args.overwrite,
                    allow_unverified: args.allow_unverified,
                    timeout: args.timeout,
                    password_file: args.password_file.clone(),
                })
                .await
            }
            _ => fail(
                exit::INVALID_INPUT,
                "--to (ou --target-database) e --user são obrigatórios no restore de MongoDB",
            ),
        },
        (EngineArg::Mysql | EngineArg::Mariadb, None) => {
            match (&args.target_database, &args.user) {
                (Some(target_database), Some(user)) => {
                    crate::mysql::restore(&crate::mysql::RestoreRequest {
                        flavor: if engine == EngineArg::Mariadb {
                            Flavor::Mariadb
                        } else {
                            Flavor::Mysql
                        },
                        file: args.file(),
                        source: args.source(),
                        host: args.host.as_deref().unwrap_or("localhost"),
                        port: args.port,
                        target_database,
                        user,
                        tls: args.tls.unwrap_or(TlsArg::Auto),
                        runner: args.runner,
                        overwrite: args.overwrite,
                        allow_unverified: args.allow_unverified,
                        timeout: args.timeout,
                        password_file: args.password_file.clone(),
                    })
                    .await
                }
                _ => fail(
                    exit::INVALID_INPUT,
                    &format!(
                        "--to (ou --target-database) e --user são obrigatórios no restore de {}",
                        engine_name(engine)
                    ),
                ),
            }
        }
        (EngineArg::Postgres, None) => match (&args.target_database, &args.user) {
            (Some(target_database), Some(user)) => run_postgres(&args, target_database, user).await,
            _ => fail(
                exit::INVALID_INPUT,
                "--to (ou --target-database) e --user são obrigatórios no restore de PostgreSQL",
            ),
        },
    }
}

async fn resolve_engine(
    file: &Path,
    requested: Option<EngineArg>,
    profile: Option<&ConnectionProfile>,
) -> Result<EngineArg, ExitCode> {
    let file = std::path::absolute(file)
        .map_err(|error| fail(exit::INVALID_ARCHIVE, &error.to_string()))?;
    check_restore_source(&file)
        .await
        .map_err(|error| fail(exit::INVALID_ARCHIVE, &describe(&error)))?;
    let detected = detect_engine(&file)
        .await
        .map_err(|error| fail(exit::INVALID_ARCHIVE, error.message()))?
        .map(EngineArg::from);
    let profile_engine = profile.map(crate::profiles::profile_engine).transpose()?;
    let engine = detected
        .or(requested)
        .or(profile_engine)
        .unwrap_or(EngineArg::Postgres);
    if let Some(requested) = requested.filter(|requested| *requested != engine) {
        return Err(fail(
            exit::INVALID_ARCHIVE,
            &format!(
                "o arquivo é um backup de {}, não de {}; nada foi restaurado",
                engine_name(engine),
                engine_name(requested)
            ),
        ));
    }
    match (profile, profile_engine) {
        (Some(profile), Some(profile_engine)) if profile_engine != engine => Err(match detected {
            Some(_) => fail(
                exit::INVALID_ARCHIVE,
                &format!(
                    "o perfil {} é de {}, mas o arquivo é um backup de {}; nada foi restaurado",
                    profile.name,
                    profile.engine,
                    engine_name(engine)
                ),
            ),
            None => fail(
                exit::INVALID_INPUT,
                &format!(
                    "o perfil {} é de {}, mas --engine pede {}; nada foi restaurado",
                    profile.name,
                    profile.engine,
                    engine_name(engine)
                ),
            ),
        }),
        _ => Ok(engine),
    }
}

fn engine_name(engine: EngineArg) -> &'static str {
    match engine {
        EngineArg::Postgres => DatabaseEngine::Postgres.display_name(),
        EngineArg::Mysql => DatabaseEngine::Mysql.display_name(),
        EngineArg::Mariadb => DatabaseEngine::Mariadb.display_name(),
        EngineArg::Mongodb => DatabaseEngine::Mongodb.display_name(),
        EngineArg::Sqlite => DatabaseEngine::Sqlite.display_name(),
    }
}

async fn run_postgres(args: &RestoreArgs, target_database: &str, user: &str) -> ExitCode {
    let file = match std::path::absolute(args.file()) {
        Ok(path) => path,
        Err(error) => return fail(exit::INVALID_ARCHIVE, &error.to_string()),
    };
    let secrets = CliSecrets {
        password_file: args.password_file.clone(),
        engine: DatabaseEngine::Postgres,
    };
    let prepared = match prepare(
        RestoreRequest {
            file,
            source: args.source().to_owned(),
            host: args.host.as_deref().unwrap_or("localhost").to_owned(),
            port: args.port.unwrap_or(crate::POSTGRES_PORT),
            user: user.to_owned(),
            tls: args.tls.unwrap_or(TlsArg::Auto).into(),
            target_database: target_database.to_owned(),
            runner: args.runner.into(),
            allow_unverified: args.allow_unverified,
            timeout: args.timeout.map(Duration::from_secs),
        },
        &secrets,
    )
    .await
    {
        Ok(prepared) => prepared,
        Err(error) => return fail_with(&error),
    };
    let confirmation = match prepared.database_state() {
        DatabaseState::HasData { tables } => match confirm_target(target_database, tables, args) {
            Ok(()) => Some(target_database),
            Err(code) => return code,
        },
        DatabaseState::Missing | DatabaseState::Empty => None,
    };
    let outcome = match prepared.run(confirmation, &interrupt()).await {
        Ok(outcome) => outcome,
        Err(error) => return fail_with(&error),
    };
    let mut text = format!(
        "Restore concluído em {} ({} objetos) em {:.1}s",
        outcome.database,
        outcome.objects,
        outcome.elapsed.as_secs_f64()
    );
    if let Some(previous) = &outcome.previous_database {
        text.push_str(&format!("\nBanco anterior mantido como {previous}"));
    }
    success(
        &RestoreResult {
            restore_id: &outcome.restore_id,
            database: &outcome.database,
            mode: outcome.mode.as_str(),
            previous_database: outcome.previous_database.as_deref(),
            objects: outcome.objects,
            checksum_verified: outcome.checksum_verified,
            duration_ms: outcome.elapsed.as_millis(),
        },
        &text,
    )
}

fn confirm_target(name: &str, tables: i64, args: &RestoreArgs) -> Result<(), ExitCode> {
    if !args.overwrite {
        return Err(fail(
            exit::CONFIRMATION_REQUIRED,
            &format!(
                "o banco {name} já existe e tem {tables} tabela(s); use --overwrite para substituí-lo (o atual será mantido como {name}__previous_<data>)"
            ),
        ));
    }
    confirm_overwrite(
        name,
        &format!(
            "O banco {name} tem {tables} tabela(s) e será substituído; as conexões ativas nele serão encerradas.\nDigite o nome do banco para confirmar: "
        ),
        args.confirm.as_deref(),
    )
}

pub fn confirm_overwrite(name: &str, prompt: &str, confirm: Option<&str>) -> Result<(), ExitCode> {
    let typed = match confirm {
        Some(value) => value.to_owned(),
        None if std::io::stdin().is_terminal() => {
            eprint!("{prompt}");
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
