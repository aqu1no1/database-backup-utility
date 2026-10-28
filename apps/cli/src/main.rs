mod archive;
mod backup;
mod credentials;
mod engines;
mod listing;
mod mongo;
mod mysql;
mod output;
mod restore;
mod sqlite;
mod verify;

use std::error::Error;
use std::fmt;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Args, Parser, Subcommand, ValueEnum};
use database_backup_adapters::postgres::{
    ConnectionError, RunnerChoice, Timeouts, test_connection,
};
use database_backup_core::{
    DatabaseEngine, Operation, Password, PostgresConnectionConfig, SecretProvider, SecretSource,
    TlsMode,
};
use serde::Serialize;
use tracing::info;

use output::{LogLevel, fail, success};

macro_rules! password_note {
    () => {
        "A senha vem de --password-file, DBU_PG_PASSWORD_FILE ou DBU_PG_PASSWORD (MySQL: DBU_MYSQL_PASSWORD_FILE ou\nDBU_MYSQL_PASSWORD; MongoDB: DBU_MONGO_PASSWORD_FILE ou DBU_MONGO_PASSWORD), só uma delas (mais de uma é erro),\nou, sem nenhuma, de um prompt sem eco. O arquivo de senha precisa ter permissão 600 ou 400."
    };
}

#[derive(Debug, Parser)]
#[command(
    name = "database-backup",
    version,
    about = "Ferramenta de backup e restauração de bancos de dados.",
    after_help = "Exemplos:
  database-backup connection test --database app --user postgres
  database-backup backup run --database app --user postgres
  database-backup backup run --engine mysql --database app --user backup
  database-backup backup run --engine mongodb --database app --user backup
  database-backup backup run --engine sqlite --source ./app.db
  database-backup backup list --json
  database-backup backup verify --file app-20261021T120000Z.dump
  database-backup restore run --file app-20261021T120000Z.dump --target-database app_copia --user postgres
  database-backup restore run --engine mysql --file app-20261021T120000Z.sql --target-database app_copia --user backup
  database-backup restore run --engine mongodb --file app-20261021T120000Z.archive --target-database app_copia --user backup
  database-backup restore run --engine sqlite --file app-20261021T120000Z.sqlite3 --target ./app_copia.db
  database-backup engines

Exit codes: 0 ok, 1 falha geral, 2 parâmetro inválido, 3 autenticação, 4 banco inexistente,
5 inacessível/timeout, 6 TLS, 7 ferramentas ausentes, 8 backup falhou, 9 cancelado,
10 arquivo inválido, 11 restore falhou, 12 confirmação necessária.

Logs vão para o stderr; ajuste com --log-level ou RUST_LOG."
)]
struct Cli {
    #[arg(
        long,
        global = true,
        help = "Imprime o resultado em JSON (uma linha) no stdout"
    )]
    json: bool,
    #[arg(
        long,
        global = true,
        value_enum,
        help = "Nível dos logs no stderr (padrão: warn, ou RUST_LOG)"
    )]
    log_level: Option<LogLevel>,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    #[command(subcommand, about = "Operações sobre conexões.")]
    Connection(ConnectionCommand),
    #[command(subcommand, about = "Gera, lista e verifica backups.")]
    Backup(BackupCommand),
    #[command(subcommand, about = "Restaura backups e lista restaurações.")]
    Restore(RestoreCommand),
    #[command(
        about = "Mostra o que cada SGBD suporta: operações disponíveis, planejadas e não suportadas.",
        after_help = "Exemplos:
  database-backup engines
  database-backup engines --json

A mesma matriz está em docs/capacidades.md."
    )]
    Engines,
}

#[derive(Debug, Subcommand)]
enum ConnectionCommand {
    #[command(
        about = "Valida a configuração e testa a conexão com um PostgreSQL, MySQL ou MongoDB.",
        after_help = concat!(
            "Exemplos:\n",
            "  database-backup connection test --database app --user postgres\n",
            "  database-backup connection test --engine mysql --database app --user backup\n",
            "  database-backup connection test --engine mongodb --database app --user backup --auth-database admin\n",
            "  database-backup connection test --host db.interno --tls verify-full --database app --user backup\n\n",
            password_note!()
        )
    )]
    Test(TestConnectionArgs),
}

#[derive(Debug, Subcommand)]
enum BackupCommand {
    #[command(
        about = "Gera um backup completo de um PostgreSQL (pg_dump, formato custom), de um MySQL (mysqldump, SQL), de um MongoDB (mongodump, archive) ou de um SQLite (Online Backup API).",
        after_help = concat!(
            "Exemplos:\n",
            "  database-backup backup run --database app --user postgres\n",
            "  database-backup backup run --database app --user postgres --output-dir /srv/backups --runner docker\n",
            "  database-backup backup run --engine mysql --database app --user backup\n",
            "  database-backup backup run --engine mongodb --database app --user backup --auth-database admin\n",
            "  database-backup backup run --engine sqlite --source ./app.db\n\n",
            "Com --engine sqlite não há servidor, usuário nem senha: o arquivo de origem é aberto só para leitura.\n",
            "No MongoDB o backup é de um banco, sem snapshot de um único instante: escritas durante o backup podem deixar\n",
            "coleções inconsistentes entre si. Só standalone e o primário de um replica set são aceitos.\n",
            password_note!()
        )
    )]
    Run(backup::BackupArgs),
    #[command(
        about = "Lista os backups registrados no histórico.",
        after_help = "Exemplos:
  database-backup backup list
  database-backup backup list --database app --limit 5 --json"
    )]
    List(listing::BackupListArgs),
    #[command(
        about = "Verifica checksum e estrutura de um arquivo de backup, sem restaurar.",
        after_help = "Exemplos:
  database-backup backup verify --file app-20261021T120000Z.dump
  database-backup backup verify --file app-20261021T120000Z.sqlite3
  database-backup backup verify --file app-20261021T120000Z.sql
  database-backup backup verify --file app-20261021T120000Z.archive
  database-backup backup verify --file externo.dump --allow-unverified --json"
    )]
    Verify(verify::VerifyArgs),
}

#[derive(Debug, Subcommand)]
enum RestoreCommand {
    #[command(
        about = "Restaura um backup validado de PostgreSQL, MySQL, MongoDB ou SQLite em um destino.",
        after_help = concat!(
            "Por padrão só cria um banco novo. Sobrescrever um banco com dados exige --overwrite e confirmação;\n",
            "o anterior é mantido como <destino>__previous_<data> (PostgreSQL) ou <arquivo>.previous-<data> (SQLite).\n",
            "No MySQL e no MongoDB só é possível restaurar em um banco que ainda não existe (--overwrite não é suportado).\n\n",
            "Exemplos:\n",
            "  database-backup restore run --file app-20261021T120000Z.dump --target-database app_copia --user postgres\n",
            "  database-backup restore run --file app-20261021T120000Z.dump --target-database app --user postgres --overwrite --confirm app\n",
            "  database-backup restore run --engine mysql --file app-20261021T120000Z.sql --target-database app_copia --user backup\n",
            "  database-backup restore run --engine mongodb --file app-20261021T120000Z.archive --target-database app_copia --user backup\n",
            "  database-backup restore run --engine sqlite --file app-20261021T120000Z.sqlite3 --target ./app_copia.db\n\n",
            password_note!()
        )
    )]
    Run(Box<restore::RestoreArgs>),
    #[command(
        about = "Lista as restaurações registradas no histórico.",
        after_help = "Exemplos:
  database-backup restore list
  database-backup restore list --limit 5 --json"
    )]
    List(listing::RestoreListArgs),
}

#[derive(Debug, Args)]
struct ConnectionArgs {
    #[arg(long, default_value = "localhost", help = "Host do servidor")]
    host: String,
    #[arg(long, help = PORT_HELP)]
    port: Option<u16>,
    #[arg(long, help = "Nome do banco")]
    database: String,
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
        value_name = "NOME",
        help = "Banco de autenticação do MongoDB (padrão: admin)"
    )]
    auth_database: Option<String>,
    #[arg(
        long,
        value_name = "ARQUIVO",
        help = "Arquivo com a senha (permissão 600 ou 400)"
    )]
    password_file: Option<PathBuf>,
}

impl ConnectionArgs {
    fn config(&self) -> Result<PostgresConnectionConfig, ExitCode> {
        PostgresConnectionConfig::new(
            &self.host,
            self.port.unwrap_or(POSTGRES_PORT),
            &self.database,
            &self.user,
            self.tls.into(),
        )
        .map_err(|error| fail(exit::INVALID_INPUT, &error.to_string()))
    }
}

const POSTGRES_PORT: u16 = 5432;
const MYSQL_PORT: u16 = 3306;
const MONGO_PORT: u16 = 27017;
const PORT_HELP: &str =
    "Porta do servidor (padrão: 5432 no PostgreSQL, 3306 no MySQL, 27017 no MongoDB)";
const AUTH_DATABASE_ONLY_MONGODB: &str = "--auth-database só vale com --engine mongodb";

#[derive(Debug, Args)]
struct TestConnectionArgs {
    #[arg(
        long,
        value_enum,
        default_value_t = EngineArg::Postgres,
        help = "SGBD do servidor (postgres, mysql ou mongodb)"
    )]
    engine: EngineArg,
    #[command(flatten)]
    connection: ConnectionArgs,
    #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u64).range(1..=300), help = "Timeout em segundos para conexão e consulta")]
    timeout: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum EngineArg {
    Postgres,
    Mysql,
    Mongodb,
    Sqlite,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum RunnerArg {
    Auto,
    Native,
    Docker,
}

impl From<RunnerArg> for RunnerChoice {
    fn from(value: RunnerArg) -> Self {
        match value {
            RunnerArg::Auto => Self::Auto,
            RunnerArg::Native => Self::Native,
            RunnerArg::Docker => Self::Docker,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum TlsArg {
    Auto,
    Disable,
    VerifyFull,
}

impl From<TlsArg> for TlsMode {
    fn from(value: TlsArg) -> Self {
        match value {
            TlsArg::Auto => Self::Auto,
            TlsArg::Disable => Self::Disable,
            TlsArg::VerifyFull => Self::VerifyFull,
        }
    }
}

mod exit {
    pub const FAILURE: u8 = 1;
    pub const INVALID_INPUT: u8 = 2;
    pub const AUTHENTICATION: u8 = 3;
    pub const DATABASE_NOT_FOUND: u8 = 4;
    pub const UNREACHABLE: u8 = 5;
    pub const TLS: u8 = 6;
    pub const TOOLS: u8 = 7;
    pub const BACKUP_FAILED: u8 = 8;
    pub const CANCELED: u8 = 9;
    pub const INVALID_ARCHIVE: u8 = 10;
    pub const RESTORE_FAILED: u8 = 11;
    pub const CONFIRMATION_REQUIRED: u8 = 12;
}

#[derive(Serialize)]
struct ConnectionResult<'a> {
    server_version: &'a str,
    connection: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    topology: Option<&'a str>,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    output::init(cli.json, cli.log_level);
    match cli.command {
        Commands::Connection(ConnectionCommand::Test(args)) => run_test_connection(args).await,
        Commands::Backup(BackupCommand::Run(args)) => backup::run(args).await,
        Commands::Backup(BackupCommand::List(args)) => listing::backups(&args),
        Commands::Backup(BackupCommand::Verify(args)) => verify::run(args).await,
        Commands::Restore(RestoreCommand::Run(args)) => restore::run(*args).await,
        Commands::Restore(RestoreCommand::List(args)) => listing::restores(&args),
        Commands::Engines => engines::run(),
    }
}

async fn run_test_connection(args: TestConnectionArgs) -> ExitCode {
    if args.engine != EngineArg::Mongodb && args.connection.auth_database.is_some() {
        return fail(exit::INVALID_INPUT, AUTH_DATABASE_ONLY_MONGODB);
    }
    match args.engine {
        EngineArg::Postgres => {}
        EngineArg::Mysql => return mysql::test(&args.connection, args.timeout).await,
        EngineArg::Mongodb => return mongo::test(&args.connection, args.timeout).await,
        EngineArg::Sqlite => {
            return unsupported(DatabaseEngine::Sqlite, Operation::ConnectionTest, "");
        }
    }
    let config = match args.connection.config() {
        Ok(config) => config,
        Err(code) => return code,
    };
    let password = match read_password(
        DatabaseEngine::Postgres,
        &config,
        args.connection.password_file.clone(),
    ) {
        Ok((password, _)) => password,
        Err(message) => return fail(exit::INVALID_INPUT, &message),
    };

    let timeout = Duration::from_secs(args.timeout);
    let timeouts = Timeouts {
        connect: timeout,
        query: timeout,
    };

    match test_connection(&config, &password, timeouts).await {
        Ok(info) => {
            info!(
                operation = "connection.test",
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
                &format!(
                    "Conexão OK: PostgreSQL {} em {}",
                    info.server_version, config
                ),
            )
        }
        Err(error) => {
            info!(operation = "connection.test", connection = %config, status = "failed", "falha ao testar conexão");
            fail(exit_code_for(&error), &error.to_string())
        }
    }
}

fn unsupported(engine: DatabaseEngine, operation: Operation, hint: &str) -> ExitCode {
    let mut message = engine
        .unsupported_message(operation)
        .unwrap_or_else(|| operation.description().to_owned());
    if !hint.is_empty() {
        message.push_str("; ");
        message.push_str(hint);
    }
    fail(exit::INVALID_INPUT, &message)
}

fn read_password(
    engine: DatabaseEngine,
    target: &dyn fmt::Display,
    password_file: Option<PathBuf>,
) -> Result<(Password, SecretSource), String> {
    credentials::CliSecrets {
        password_file,
        engine,
    }
    .password(target)
    .map_err(|error| describe(&error))
}

fn exit_code_for(error: &ConnectionError) -> u8 {
    match error {
        ConnectionError::AuthenticationFailed { .. } => exit::AUTHENTICATION,
        ConnectionError::DatabaseNotFound { .. } => exit::DATABASE_NOT_FOUND,
        ConnectionError::Unreachable { .. } | ConnectionError::Timeout { .. } => exit::UNREACHABLE,
        ConnectionError::Tls { .. } => exit::TLS,
        ConnectionError::Other { .. } => exit::FAILURE,
    }
}

fn describe(error: &dyn Error) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}
