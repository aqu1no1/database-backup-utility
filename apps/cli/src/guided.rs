use std::io::IsTerminal;
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use clap::ValueEnum;
use database_backup_core::DatabaseEngine;
use database_backup_history::ConnectionProfile;
use dialoguer::console::Term;
use dialoguer::theme::ColorfulTheme;
use dialoguer::{Input, Select};

use crate::backup::BackupArgs;
use crate::output::fail;
use crate::{EngineArg, exit};

const DEFAULT_HOST: &str = "localhost";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Target,
    SqliteFile,
    Database,
    User,
}

enum Choice {
    Profile(String),
    Connection,
    SqliteFile,
}

enum Stop {
    Canceled,
    Failed(String),
}

impl From<dialoguer::Error> for Stop {
    fn from(error: dialoguer::Error) -> Self {
        Self::Failed(error.to_string())
    }
}

pub fn prompt_allowed(json: bool, no_input: bool, stdin_tty: bool, stderr_tty: bool) -> bool {
    !json && !no_input && stdin_tty && stderr_tty
}

pub fn missing(args: &BackupArgs) -> Vec<Field> {
    if args.name.is_some() || args.profile.is_some() || args.source.is_some() {
        return Vec::new();
    }
    match args.engine {
        Some(EngineArg::Sqlite) => vec![Field::SqliteFile],
        None if args.server.is_empty() => vec![Field::Target],
        _ => [
            (args.server.database.is_none(), Field::Database),
            (args.server.user.is_none(), Field::User),
        ]
        .into_iter()
        .filter_map(|(absent, field)| absent.then_some(field))
        .collect(),
    }
}

pub fn fill(mut args: BackupArgs, json: bool, no_input: bool) -> Result<BackupArgs, ExitCode> {
    let fields = missing(&args);
    let interactive = prompt_allowed(
        json,
        no_input,
        std::io::stdin().is_terminal(),
        std::io::stderr().is_terminal(),
    );
    if fields.is_empty() || !interactive {
        return Ok(args);
    }
    let profiles = if fields.contains(&Field::Target) {
        crate::profiles::list_all()?
    } else {
        Vec::new()
    };
    let prompter = Prompter {
        term: Term::stderr(),
        theme: ColorfulTheme::default(),
    };
    match prompter.complete(&mut args, &fields, &profiles) {
        Ok(()) => {
            let _ = prompter
                .term
                .write_line(&format!("Comando equivalente: {}", render(&args)));
            Ok(args)
        }
        Err(Stop::Canceled) => Err(fail(exit::CANCELED, "modo guiado cancelado")),
        Err(Stop::Failed(message)) => Err(fail(
            exit::INVALID_INPUT,
            &format!("modo guiado interrompido: {message}"),
        )),
    }
}

struct Prompter {
    term: Term,
    theme: ColorfulTheme,
}

impl Prompter {
    fn complete(
        &self,
        args: &mut BackupArgs,
        fields: &[Field],
        profiles: &[ConnectionProfile],
    ) -> Result<(), Stop> {
        for field in fields {
            match field {
                Field::Target => match self.target(profiles)? {
                    Choice::Profile(name) => args.profile = Some(name),
                    Choice::SqliteFile => args.source = Some(self.sqlite_file()?),
                    Choice::Connection => self.connection(args)?,
                },
                Field::SqliteFile => args.source = Some(self.sqlite_file()?),
                Field::Database => args.server.database = Some(self.required("Banco")?),
                Field::User => args.server.user = Some(self.required("Usuário")?),
            }
        }
        Ok(())
    }

    fn target(&self, profiles: &[ConnectionProfile]) -> Result<Choice, Stop> {
        let mut items: Vec<String> = profiles
            .iter()
            .map(|profile| {
                format!(
                    "Perfil {} ({} {} em {})",
                    profile.name, profile.engine, profile.database_name, profile.host
                )
            })
            .collect();
        items.push("Nova conexão com um servidor".to_owned());
        items.push("Arquivo SQLite".to_owned());
        let index = self.select("O que você quer copiar?", &items)?;
        Ok(match profiles.get(index) {
            Some(profile) => Choice::Profile(profile.name.clone()),
            None if index == profiles.len() => Choice::Connection,
            None => Choice::SqliteFile,
        })
    }

    fn connection(&self, args: &mut BackupArgs) -> Result<(), Stop> {
        let engines = server_engines();
        let names: Vec<&str> = engines
            .iter()
            .map(|(engine, _)| engine.display_name())
            .collect();
        let engine = engines[self.select("SGBD", &names)?].1;
        args.engine = Some(engine);

        let host = self.with_default("Host", DEFAULT_HOST)?;
        args.server.host = (host != DEFAULT_HOST).then_some(host);
        let default_port = default_port(engine);
        let port: u16 = Input::with_theme(&self.theme)
            .with_prompt("Porta")
            .default(default_port)
            .interact_text_on(&self.term)?;
        args.server.port = (port != default_port).then_some(port);
        args.server.database = Some(self.required("Banco")?);
        args.server.user = Some(self.required("Usuário")?);
        if args.output_dir.is_none() {
            let output: String = Input::with_theme(&self.theme)
                .with_prompt("Pasta de saída (Enter para a pasta padrão)")
                .allow_empty(true)
                .interact_text_on(&self.term)?;
            let output = output.trim();
            if !output.is_empty() {
                args.output_dir = Some(PathBuf::from(output));
            }
        }
        Ok(())
    }

    fn sqlite_file(&self) -> Result<PathBuf, Stop> {
        self.required("Arquivo SQLite de origem").map(PathBuf::from)
    }

    fn select<T: std::fmt::Display>(&self, prompt: &str, items: &[T]) -> Result<usize, Stop> {
        Select::with_theme(&self.theme)
            .with_prompt(prompt)
            .items(items)
            .default(0)
            .interact_on_opt(&self.term)?
            .ok_or(Stop::Canceled)
    }

    fn required(&self, prompt: &str) -> Result<String, Stop> {
        let value: String = Input::with_theme(&self.theme)
            .with_prompt(prompt)
            .validate_with(|value: &String| {
                if value.trim().is_empty() {
                    Err("obrigatório")
                } else {
                    Ok(())
                }
            })
            .interact_text_on(&self.term)?;
        Ok(value.trim().to_owned())
    }

    fn with_default(&self, prompt: &str, default: &str) -> Result<String, Stop> {
        let value: String = Input::with_theme(&self.theme)
            .with_prompt(prompt)
            .default(default.to_owned())
            .interact_text_on(&self.term)?;
        Ok(value.trim().to_owned())
    }
}

fn server_engines() -> Vec<(DatabaseEngine, EngineArg)> {
    DatabaseEngine::ALL
        .into_iter()
        .filter(|engine| *engine != DatabaseEngine::Sqlite)
        .map(|engine| (engine, EngineArg::from(engine)))
        .collect()
}

fn default_port(engine: EngineArg) -> u16 {
    match engine {
        EngineArg::Postgres | EngineArg::Sqlite => crate::POSTGRES_PORT,
        EngineArg::Mysql | EngineArg::Mariadb => crate::MYSQL_PORT,
        EngineArg::Mongodb => crate::MONGO_PORT,
    }
}

fn value_name<T: ValueEnum>(value: &T) -> Option<String> {
    value
        .to_possible_value()
        .map(|possible| possible.get_name().to_owned())
}

fn path_word(path: &Path) -> String {
    let single_relative = path.is_relative()
        && path.components().count() == 1
        && matches!(path.components().next(), Some(Component::Normal(_)));
    if single_relative {
        Path::new(".").join(path).display().to_string()
    } else {
        path.display().to_string()
    }
}

pub fn render(args: &BackupArgs) -> String {
    let mut words = vec!["dbu".to_owned(), "backup".to_owned()];
    if let Some(name) = &args.name {
        words.push(name.clone());
    } else if let Some(profile) = &args.profile {
        if Path::new(profile).exists() {
            push_flag(&mut words, "--profile", Some(profile.clone()));
        } else {
            words.push(profile.clone());
        }
    } else if let Some(source) = &args.source {
        words.push(path_word(source));
    } else {
        let server = &args.server;
        let engine = args
            .engine
            .filter(|engine| *engine != EngineArg::Postgres)
            .as_ref()
            .and_then(value_name);
        push_flag(&mut words, "-e", engine);
        push_flag(&mut words, "-H", server.host.clone());
        push_flag(&mut words, "-p", server.port.map(|port| port.to_string()));
        push_flag(&mut words, "-d", server.database.clone());
        push_flag(&mut words, "-u", server.user.clone());
        push_flag(
            &mut words,
            "--tls",
            server.tls.as_ref().and_then(value_name),
        );
        push_flag(&mut words, "--auth-database", server.auth_database.clone());
        let password_file = server
            .password_file
            .as_ref()
            .map(|path| path.display().to_string());
        push_flag(&mut words, "--password-file", password_file);
    }
    let output = args
        .output_dir
        .as_ref()
        .map(|path| path.display().to_string());
    push_flag(&mut words, "-o", output);
    push_flag(
        &mut words,
        "--runner",
        args.runner.as_ref().and_then(value_name),
    );
    push_flag(
        &mut words,
        "--timeout",
        args.timeout.map(|limit| limit.to_string()),
    );
    if args.encoding.compress {
        words.push("--compress".to_owned());
    }
    for key in &args.encoding.recipients {
        push_flag(&mut words, "--recipient", Some(key.clone()));
    }
    for path in &args.encoding.recipients_file {
        push_flag(
            &mut words,
            "--recipients-file",
            Some(path.display().to_string()),
        );
    }
    shell_words::join(words)
}

fn push_flag(words: &mut Vec<String>, flag: &str, value: Option<String>) {
    if let Some(value) = value {
        words.push(flag.to_owned());
        words.push(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiles::ConnectionOptions;
    use crate::{RunnerArg, TlsArg};

    fn server(database: Option<&str>, user: Option<&str>) -> ConnectionOptions {
        ConnectionOptions {
            database: database.map(str::to_owned),
            user: user.map(str::to_owned),
            ..ConnectionOptions::default()
        }
    }

    #[test]
    fn prompts_only_with_both_terminals_and_without_json_or_no_input() {
        assert!(prompt_allowed(false, false, true, true));
        for (json, no_input, stdin_tty, stderr_tty) in [
            (true, false, true, true),
            (false, true, true, true),
            (false, false, false, true),
            (false, false, true, false),
            (false, false, false, false),
        ] {
            assert!(
                !prompt_allowed(json, no_input, stdin_tty, stderr_tty),
                "{json} {no_input} {stdin_tty} {stderr_tty}"
            );
        }
    }

    #[test]
    fn nothing_is_missing_when_a_target_profile_or_source_is_given() {
        for args in [
            BackupArgs {
                name: Some("prod".to_owned()),
                ..BackupArgs::default()
            },
            BackupArgs {
                profile: Some("prod".to_owned()),
                ..BackupArgs::default()
            },
            BackupArgs {
                source: Some(PathBuf::from("./app.db")),
                ..BackupArgs::default()
            },
            BackupArgs {
                engine: Some(EngineArg::Mysql),
                server: server(Some("app"), Some("backup")),
                ..BackupArgs::default()
            },
        ] {
            assert_eq!(missing(&args), Vec::new(), "{args:?}");
        }
    }

    #[test]
    fn bare_backup_asks_for_the_target_and_partial_flags_ask_only_what_is_left() {
        assert_eq!(missing(&BackupArgs::default()), [Field::Target]);
        let only_output = BackupArgs {
            output_dir: Some(PathBuf::from("/srv/backups")),
            ..BackupArgs::default()
        };
        assert_eq!(missing(&only_output), [Field::Target]);

        let only_database = BackupArgs {
            server: server(Some("app"), None),
            ..BackupArgs::default()
        };
        assert_eq!(missing(&only_database), [Field::User]);

        let engine_only = BackupArgs {
            engine: Some(EngineArg::Mongodb),
            ..BackupArgs::default()
        };
        assert_eq!(missing(&engine_only), [Field::Database, Field::User]);

        let sqlite = BackupArgs {
            engine: Some(EngineArg::Sqlite),
            ..BackupArgs::default()
        };
        assert_eq!(missing(&sqlite), [Field::SqliteFile]);
    }

    #[test]
    fn renders_the_short_command_with_quoting() {
        let args = BackupArgs {
            engine: Some(EngineArg::Mysql),
            server: ConnectionOptions {
                host: Some("db.interno".to_owned()),
                port: Some(3307),
                tls: Some(TlsArg::VerifyFull),
                password_file: Some(PathBuf::from("/secrets/my sql.pass")),
                ..server(Some("app"), Some("backup"))
            },
            output_dir: Some(PathBuf::from("/srv/backups")),
            runner: Some(RunnerArg::Docker),
            ..BackupArgs::default()
        };

        assert_eq!(
            render(&args),
            "dbu backup -e mysql -H db.interno -p 3307 -d app -u backup --tls verify-full --password-file '/secrets/my sql.pass' -o /srv/backups --runner docker"
        );
    }

    #[test]
    fn renders_postgres_profile_and_sqlite_forms() {
        let postgres = BackupArgs {
            engine: Some(EngineArg::Postgres),
            server: server(Some("app"), Some("postgres")),
            ..BackupArgs::default()
        };
        assert_eq!(render(&postgres), "dbu backup -d app -u postgres");

        let profile = BackupArgs {
            profile: Some("prod".to_owned()),
            ..BackupArgs::default()
        };
        assert_eq!(render(&profile), "dbu backup prod");

        for (source, expected) in [
            ("app.db", "dbu backup ./app.db"),
            ("dados/app.db", "dbu backup dados/app.db"),
            ("/srv/app db.sqlite", "dbu backup '/srv/app db.sqlite'"),
        ] {
            let args = BackupArgs {
                source: Some(PathBuf::from(source)),
                ..BackupArgs::default()
            };
            assert_eq!(render(&args), expected);
        }
    }

    #[test]
    fn server_engines_never_offer_sqlite() {
        assert!(
            server_engines()
                .iter()
                .all(|(engine, _)| *engine != DatabaseEngine::Sqlite)
        );
        assert_eq!(server_engines().len(), DatabaseEngine::ALL.len() - 1);
    }
}
