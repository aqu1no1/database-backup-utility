use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, ValueEnum};
use database_backup_core::{MongoConnectionConfig, MysqlConnectionConfig, read_password_file};
use database_backup_history::{ConnectionProfile, History, HistoryError};
use serde::Serialize;

use crate::output::{fail, success};
use crate::{ConnectionArgs, EngineArg, TlsArg, describe, exit};

pub const SQLITE_UNSUPPORTED: &str =
    "perfis de conexão não suportam SQLite; use --source ou --target com o arquivo";

#[derive(Debug, Default, Args)]
pub struct ConnectionOptions {
    #[arg(short = 'H', long, help = "Host do servidor (padrão: localhost)")]
    pub host: Option<String>,
    #[arg(short, long, help = crate::PORT_HELP)]
    pub port: Option<u16>,
    #[arg(short, long, help = "Nome do banco")]
    pub database: Option<String>,
    #[arg(short, long, help = "Usuário")]
    pub user: Option<String>,
    #[arg(
        long,
        value_enum,
        help = "auto (padrão): sem TLS só em localhost; verify-full: TLS com certificado validado"
    )]
    pub tls: Option<TlsArg>,
    #[arg(
        long,
        value_name = "NOME",
        help = "Banco de autenticação do MongoDB (padrão: admin)"
    )]
    pub auth_database: Option<String>,
    #[arg(
        long,
        value_name = "ARQUIVO",
        help = "Arquivo com a senha (permissão 600 ou 400); substitui o caminho do perfil"
    )]
    pub password_file: Option<PathBuf>,
}

impl ConnectionOptions {
    pub fn resolve(
        self,
        engine: Option<EngineArg>,
        profile: Option<&ConnectionProfile>,
    ) -> Result<(EngineArg, ConnectionArgs), ExitCode> {
        let profile_engine = profile.map(profile_engine).transpose()?;
        let engine = engine.or(profile_engine).unwrap_or(EngineArg::Postgres);
        let same_engine_profile = profile.filter(|_| profile_engine == Some(engine));
        if engine != EngineArg::Mongodb && self.auth_database.is_some() {
            return Err(fail(exit::INVALID_INPUT, crate::AUTH_DATABASE_ONLY_MONGODB));
        }
        let database = self
            .database
            .or_else(|| profile.map(|p| p.database_name.clone()))
            .ok_or_else(|| fail(exit::INVALID_INPUT, "--database é obrigatório"))?;
        let user = self
            .user
            .or_else(|| profile.map(|p| p.username.clone()))
            .ok_or_else(|| fail(exit::INVALID_INPUT, "--user é obrigatório"))?;
        let tls = self
            .tls
            .or(profile.map(profile_tls).transpose()?)
            .unwrap_or(TlsArg::Auto);
        let connection = ConnectionArgs {
            host: self
                .host
                .or_else(|| profile.map(|p| p.host.clone()))
                .unwrap_or_else(|| "localhost".to_owned()),
            port: self.port.or_else(|| same_engine_profile.map(|p| p.port)),
            database,
            user,
            tls,
            auth_database: self
                .auth_database
                .or_else(|| same_engine_profile.and_then(|p| p.auth_database.clone())),
            password_file: self
                .password_file
                .or_else(|| profile.and_then(|p| p.password_file.as_ref().map(PathBuf::from))),
        };
        validate(engine, &connection)?;
        Ok((engine, connection))
    }
}

pub fn profile_engine(profile: &ConnectionProfile) -> Result<EngineArg, ExitCode> {
    EngineArg::from_str(&profile.engine, false)
        .map_err(|_| fail(exit::INVALID_INPUT, "o perfil contém um SGBD inválido"))
}

pub fn profile_tls(profile: &ConnectionProfile) -> Result<TlsArg, ExitCode> {
    TlsArg::from_str(&profile.tls, false).map_err(|_| {
        fail(
            exit::INVALID_INPUT,
            "o perfil contém uma configuração TLS inválida",
        )
    })
}

fn validate(engine: EngineArg, connection: &ConnectionArgs) -> Result<(), ExitCode> {
    let result = match engine {
        EngineArg::Postgres => return connection.config().map(|_| ()),
        EngineArg::Mysql => MysqlConnectionConfig::new(
            &connection.host,
            connection.port.unwrap_or(crate::MYSQL_PORT),
            &connection.database,
            &connection.user,
            connection.tls.into(),
        )
        .map(|_| ()),
        EngineArg::Mongodb => MongoConnectionConfig::new(
            &connection.host,
            connection.port.unwrap_or(crate::MONGO_PORT),
            &connection.database,
            &connection.user,
            connection.auth_database.as_deref().unwrap_or("admin"),
            connection.tls.into(),
        )
        .map(|_| ()),
        EngineArg::Sqlite => {
            return Err(fail(exit::INVALID_INPUT, SQLITE_UNSUPPORTED));
        }
    };
    result.map_err(|error| fail(exit::INVALID_INPUT, &error.to_string()))
}

fn valid_name(name: &str) -> Result<(), ExitCode> {
    if name.is_empty()
        || name.len() > 64
        || !name.as_bytes()[0].is_ascii_alphanumeric()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    {
        return Err(fail(
            exit::INVALID_INPUT,
            "o nome do perfil deve ter até 64 caracteres, começar com letra ou número e conter apenas letras ASCII, números, -, _ ou .",
        ));
    }
    Ok(())
}

fn history() -> Result<History, ExitCode> {
    History::default_path()
        .and_then(|path| History::open(&path))
        .map_err(|error| fail(exit::FAILURE, &describe(&error)))
}

pub fn load(name: Option<&str>) -> Result<Option<ConnectionProfile>, ExitCode> {
    let Some(name) = name else {
        return Ok(None);
    };
    valid_name(name)?;
    history()?
        .get_profile(name)
        .map_err(|error| fail(exit::FAILURE, &describe(&error)))?
        .map(Some)
        .ok_or_else(|| not_found(name))
}

#[derive(Debug, Args)]
pub struct AddArgs {
    #[arg(value_name = "NOME")]
    name: String,
    #[arg(short, long, value_enum)]
    engine: Option<EngineArg>,
    #[command(flatten)]
    connection: ConnectionOptions,
    #[arg(long, help = "Salva sem testar a conexão; mantém a validação local")]
    no_test: bool,
    #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u64).range(1..=300))]
    timeout: u64,
}

#[derive(Serialize)]
struct ProfileName<'a> {
    name: &'a str,
}

pub async fn add(args: AddArgs) -> ExitCode {
    if let Err(code) = valid_name(&args.name) {
        return code;
    }
    let history = match history() {
        Ok(history) => history,
        Err(code) => return code,
    };
    match history.get_profile(&args.name) {
        Ok(Some(_)) => {
            return fail(
                exit::INVALID_INPUT,
                &format!("o perfil {} já existe", args.name),
            );
        }
        Ok(None) => {}
        Err(error) => return fail(exit::FAILURE, &describe(&error)),
    }
    let (engine, mut connection) = match args.connection.resolve(args.engine, None) {
        Ok(value) => value,
        Err(code) => return code,
    };
    let password_file = match connection.password_file.as_deref() {
        Some(path) => {
            let path = match std::path::absolute(path) {
                Ok(path) => path,
                Err(error) => return fail(exit::INVALID_INPUT, &error.to_string()),
            };
            if let Err(error) = read_password_file(&path) {
                return fail(exit::INVALID_INPUT, &describe(&error));
            }
            let Some(value) = path.to_str() else {
                return fail(
                    exit::INVALID_INPUT,
                    "o caminho do arquivo de senha deve ser UTF-8",
                );
            };
            Some(value.to_owned())
        }
        None => None,
    };
    connection.password_file = password_file.as_ref().map(PathBuf::from);
    let profile = ConnectionProfile {
        name: args.name,
        engine: match engine {
            EngineArg::Postgres => "postgres",
            EngineArg::Mysql => "mysql",
            EngineArg::Mongodb => "mongodb",
            EngineArg::Sqlite => "sqlite",
        }
        .to_owned(),
        host: connection.host.clone(),
        port: connection.port.unwrap_or(match engine {
            EngineArg::Postgres => crate::POSTGRES_PORT,
            EngineArg::Mysql => crate::MYSQL_PORT,
            EngineArg::Mongodb => crate::MONGO_PORT,
            EngineArg::Sqlite => 0,
        }),
        database_name: connection.database.clone(),
        username: connection.user.clone(),
        tls: match connection.tls {
            TlsArg::Auto => "auto",
            TlsArg::Disable => "disable",
            TlsArg::VerifyFull => "verify-full",
        }
        .to_owned(),
        auth_database: connection.auth_database.clone(),
        password_file,
    };
    if !args.no_test {
        let code = crate::test_resolved(engine, connection, args.timeout, false).await;
        if code != ExitCode::SUCCESS {
            return code;
        }
    }
    if let Err(error) = history.add_profile(&profile) {
        let code = if matches!(error, HistoryError::ProfileExists(_)) {
            exit::INVALID_INPUT
        } else {
            exit::FAILURE
        };
        return fail(code, &describe(&error));
    }
    success(
        &ProfileName {
            name: &profile.name,
        },
        &format!("Perfil {} salvo.", profile.name),
    )
}

#[derive(Serialize)]
struct ProfileView<'a> {
    name: &'a str,
    engine: &'a str,
    host: &'a str,
    port: u16,
    database_name: &'a str,
    username: &'a str,
    tls: &'a str,
    auth_database: Option<&'a str>,
    password_file: Option<&'a str>,
}

#[derive(Serialize)]
struct ProfileList<'a> {
    profiles: Vec<ProfileView<'a>>,
}

pub fn list() -> ExitCode {
    let history = match history() {
        Ok(history) => history,
        Err(code) => return code,
    };
    let profiles = match history.list_profiles() {
        Ok(profiles) => profiles,
        Err(error) => return fail(exit::FAILURE, &describe(&error)),
    };
    let text = if profiles.is_empty() {
        "Nenhum perfil salvo.".to_owned()
    } else {
        profiles
            .iter()
            .map(|p| {
                format!(
                    "{}\t{}\t{}@{}:{}/{}\t{}",
                    p.name, p.engine, p.username, p.host, p.port, p.database_name, p.tls
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let views = profiles
        .iter()
        .map(|p| ProfileView {
            name: &p.name,
            engine: &p.engine,
            host: &p.host,
            port: p.port,
            database_name: &p.database_name,
            username: &p.username,
            tls: &p.tls,
            auth_database: p.auth_database.as_deref(),
            password_file: p.password_file.as_deref(),
        })
        .collect();
    success(&ProfileList { profiles: views }, &text)
}

#[derive(Debug, Args)]
pub struct RemoveArgs {
    #[arg(value_name = "NOME")]
    name: String,
    #[arg(short = 'y', long, help = "Confirma a remoção do perfil sem perguntar")]
    yes: bool,
}

pub fn remove(args: RemoveArgs) -> ExitCode {
    if let Err(code) = valid_name(&args.name) {
        return code;
    }
    let history = match history() {
        Ok(history) => history,
        Err(code) => return code,
    };
    match history.get_profile(&args.name) {
        Ok(Some(_)) => {}
        Ok(None) => return not_found(&args.name),
        Err(error) => return fail(exit::FAILURE, &describe(&error)),
    }
    if !args.yes {
        if !std::io::stdin().is_terminal() {
            return fail(
                exit::CONFIRMATION_REQUIRED,
                "a remoção do perfil exige confirmação; use -y",
            );
        }
        eprint!("Para remover o perfil {}, repita o nome: ", args.name);
        if std::io::stderr().flush().is_err() {
            return fail(exit::FAILURE, "não foi possível solicitar confirmação");
        }
        let mut answer = String::new();
        if std::io::stdin().lock().read_line(&mut answer).is_err()
            || answer.trim_end_matches(['\r', '\n']) != args.name
        {
            return fail(exit::CONFIRMATION_REQUIRED, "remoção não confirmada");
        }
    }
    match history.remove_profile(&args.name) {
        Ok(true) => success(
            &ProfileName { name: &args.name },
            &format!("Perfil {} removido.", args.name),
        ),
        Ok(false) => not_found(&args.name),
        Err(error) => fail(exit::FAILURE, &describe(&error)),
    }
}

fn not_found(name: &str) -> ExitCode {
    fail(exit::INVALID_INPUT, &format!("o perfil {name} não existe"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> ConnectionProfile {
        ConnectionProfile {
            name: "dev".to_owned(),
            engine: "mysql".to_owned(),
            host: "db.example.org".to_owned(),
            port: 3307,
            database_name: "app".to_owned(),
            username: "backup".to_owned(),
            tls: "verify-full".to_owned(),
            auth_database: None,
            password_file: Some("/secrets/profile.pass".to_owned()),
        }
    }

    #[test]
    fn omitted_flags_keep_profile_values_and_explicit_flags_replace_them() {
        let profile = profile();
        let (engine, inherited) = ConnectionOptions::default()
            .resolve(None, Some(&profile))
            .unwrap();
        assert_eq!(engine, EngineArg::Mysql);
        assert_eq!(inherited.host, profile.host);
        assert_eq!(inherited.port, Some(3307));
        assert!(matches!(inherited.tls, TlsArg::VerifyFull));
        assert_eq!(
            inherited.password_file,
            Some(PathBuf::from("/secrets/profile.pass"))
        );

        let options = ConnectionOptions {
            host: Some("localhost".to_owned()),
            port: Some(5432),
            database: Some("copy".to_owned()),
            user: Some("other".to_owned()),
            tls: Some(TlsArg::Auto),
            password_file: Some(PathBuf::from("/secrets/override.pass")),
            ..Default::default()
        };
        let (engine, explicit) = options
            .resolve(Some(EngineArg::Postgres), Some(&profile))
            .unwrap();
        assert_eq!(engine, EngineArg::Postgres);
        assert_eq!(explicit.host, "localhost");
        assert_eq!(explicit.port, Some(5432));
        assert_eq!(explicit.database, "copy");
        assert_eq!(explicit.user, "other");
        assert!(matches!(explicit.tls, TlsArg::Auto));
        assert_eq!(
            explicit.password_file,
            Some(PathBuf::from("/secrets/override.pass"))
        );
    }

    #[test]
    fn switching_engine_drops_profile_settings_specific_to_the_profile_engine() {
        let (engine, switched) = ConnectionOptions::default()
            .resolve(Some(EngineArg::Postgres), Some(&profile()))
            .unwrap();
        assert_eq!(engine, EngineArg::Postgres);
        assert_eq!(switched.port, None);
        assert_eq!(switched.host, "db.example.org");
        assert_eq!(switched.database, "app");

        let mongo = ConnectionProfile {
            engine: "mongodb".to_owned(),
            port: 27018,
            auth_database: Some("users".to_owned()),
            ..profile()
        };
        let (_, switched) = ConnectionOptions::default()
            .resolve(Some(EngineArg::Mysql), Some(&mongo))
            .unwrap();
        assert_eq!(switched.port, None);
        assert_eq!(switched.auth_database, None);

        let (_, same) = ConnectionOptions::default()
            .resolve(Some(EngineArg::Mongodb), Some(&mongo))
            .unwrap();
        assert_eq!(same.port, Some(27018));
        assert_eq!(same.auth_database.as_deref(), Some("users"));
    }

    #[test]
    fn effective_configuration_is_revalidated_after_overrides() {
        let profile = profile();
        let options = ConnectionOptions {
            tls: Some(TlsArg::Disable),
            ..Default::default()
        };
        assert!(options.resolve(None, Some(&profile)).is_err());
        let options = ConnectionOptions {
            database: Some("bad/name".to_owned()),
            ..Default::default()
        };
        assert!(options.resolve(None, Some(&profile)).is_err());
    }
}
