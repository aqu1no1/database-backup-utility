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
    name = "dbu",
    version,
    about = "Ferramenta de backup e restauração de bancos de dados.",
    after_help = "Exemplos:
  dbu backup -d app -u postgres
  dbu backup -e mysql -d app -u backup
  dbu backup -e sqlite --source ./app.db
  dbu verify app-20261021T120000Z.dump
  dbu restore app-20261021T120000Z.dump --to app_copia -u postgres
  dbu restore -e sqlite app-20261021T120000Z.sqlite3 --to ./app_copia.db
  dbu list
  dbu list restores
  dbu test -e mysql -d app -u backup
  dbu engines

Os comandos longos continuam válidos (backup run, backup verify, restore run, connection test...).

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
    #[command(
        args_conflicts_with_subcommands = true,
        subcommand_negates_reqs = true,
        about = "Gera um backup completo de PostgreSQL, MySQL, MongoDB ou SQLite (também lista e verifica backups).",
        long_about = "Gera um backup completo de um PostgreSQL (pg_dump, formato custom), de um MySQL (mysqldump, SQL), de um MongoDB (mongodump, archive) ou de um SQLite (Online Backup API).",
        after_help = BACKUP_HELP
    )]
    Backup(Box<BackupGroup>),
    #[command(
        about = "Verifica checksum e estrutura de um arquivo de backup, sem restaurar.",
        after_help = VERIFY_EXAMPLES
    )]
    Verify(verify::VerifyArgs),
    #[command(
        args_conflicts_with_subcommands = true,
        subcommand_negates_reqs = true,
        about = "Restaura um backup validado de PostgreSQL, MySQL, MongoDB ou SQLite em um destino.",
        after_help = RESTORE_HELP
    )]
    Restore(RestoreGroup),
    #[command(
        args_conflicts_with_subcommands = true,
        about = "Lista os backups (padrão) ou as restaurações registradas no histórico.",
        after_help = "Exemplos:
  dbu list
  dbu list -d app --limit 5 --json
  dbu list restores"
    )]
    List(ListGroup),
    #[command(
        about = "Valida a configuração e testa a conexão com um PostgreSQL, MySQL ou MongoDB.",
        after_help = TEST_HELP
    )]
    Test(TestConnectionArgs),
    #[command(subcommand, about = "Operações sobre conexões.")]
    Connection(ConnectionCommand),
    #[command(
        about = "Mostra o que cada SGBD suporta: operações disponíveis, planejadas e não suportadas.",
        after_help = "Exemplos:
  dbu engines
  dbu engines --json

A mesma matriz está em docs/capacidades.md."
    )]
    Engines,
}

const BACKUP_HELP: &str = concat!(
    "Exemplos:\n",
    "  dbu backup -d app -u postgres\n",
    "  dbu backup -d app -u postgres -o /srv/backups --runner docker\n",
    "  dbu backup -e mysql -d app -u backup\n",
    "  dbu backup -e mongodb -d app -u backup --auth-database admin\n",
    "  dbu backup -e sqlite --source ./app.db\n",
    "  dbu backup list\n\n",
    "Com --engine sqlite não há servidor, usuário nem senha: o arquivo de origem é aberto só para leitura.\n",
    "No MongoDB o backup é de um banco, sem snapshot de um único instante: escritas durante o backup podem deixar\n",
    "coleções inconsistentes entre si. Só standalone e o primário de um replica set são aceitos.\n",
    password_note!()
);

const VERIFY_EXAMPLES: &str = "Exemplos:
  dbu verify app-20261021T120000Z.dump
  dbu verify app-20261021T120000Z.sqlite3
  dbu verify app-20261021T120000Z.sql
  dbu verify app-20261021T120000Z.archive
  dbu verify externo.dump --allow-unverified --json";

const RESTORE_HELP: &str = concat!(
    "Por padrão só cria um banco novo. Sobrescrever um banco com dados exige --overwrite e confirmação;\n",
    "o anterior é mantido como <destino>__previous_<data> (PostgreSQL) ou <arquivo>.previous-<data> (SQLite).\n",
    "No MySQL e no MongoDB só é possível restaurar em um banco que ainda não existe (--overwrite não é suportado).\n",
    "--to é o banco de destino (ou o arquivo de destino, com --engine sqlite).\n\n",
    "Exemplos:\n",
    "  dbu restore app-20261021T120000Z.dump --to app_copia -u postgres\n",
    "  dbu restore app-20261021T120000Z.dump --to app -u postgres --overwrite --confirm app\n",
    "  dbu restore -e mysql app-20261021T120000Z.sql --to app_copia -u backup\n",
    "  dbu restore -e mongodb app-20261021T120000Z.archive --to app_copia -u backup\n",
    "  dbu restore -e sqlite app-20261021T120000Z.sqlite3 --to ./app_copia.db\n",
    "  dbu restore list\n\n",
    password_note!()
);

const TEST_HELP: &str = concat!(
    "Exemplos:\n",
    "  dbu test -d app -u postgres\n",
    "  dbu test -e mysql -d app -u backup\n",
    "  dbu test -e mongodb -d app -u backup --auth-database admin\n",
    "  dbu test -H db.interno --tls verify-full -d app -u backup\n\n",
    password_note!()
);

#[derive(Debug, Args)]
struct BackupGroup {
    #[command(subcommand)]
    command: Option<BackupCommand>,
    #[command(flatten)]
    run: backup::BackupArgs,
}

#[derive(Debug, Args)]
struct RestoreGroup {
    #[command(subcommand)]
    command: Option<RestoreCommand>,
    #[command(flatten)]
    run: Box<restore::RestoreArgs>,
}

#[derive(Debug, Args)]
struct ListGroup {
    #[command(subcommand)]
    command: Option<ListCommand>,
    #[command(flatten)]
    backups: listing::BackupListArgs,
}

#[derive(Debug, Subcommand)]
enum ListCommand {
    #[command(about = "Lista os backups registrados no histórico.")]
    Backups(listing::BackupListArgs),
    #[command(about = "Lista as restaurações registradas no histórico.")]
    Restores(listing::RestoreListArgs),
}

#[derive(Debug, Subcommand)]
enum ConnectionCommand {
    #[command(
        about = "Valida a configuração e testa a conexão com um PostgreSQL, MySQL ou MongoDB.",
        after_help = TEST_HELP
    )]
    Test(TestConnectionArgs),
}

#[derive(Debug, Subcommand)]
enum BackupCommand {
    #[command(
        about = "Gera um backup completo (o mesmo que dbu backup).",
        after_help = BACKUP_HELP
    )]
    Run(backup::BackupArgs),
    #[command(
        about = "Lista os backups registrados no histórico.",
        after_help = "Exemplos:
  dbu backup list
  dbu backup list -d app --limit 5 --json"
    )]
    List(listing::BackupListArgs),
    #[command(
        about = "Verifica checksum e estrutura de um arquivo de backup, sem restaurar (o mesmo que dbu verify).",
        after_help = VERIFY_EXAMPLES
    )]
    Verify(verify::VerifyArgs),
}

#[derive(Debug, Subcommand)]
enum RestoreCommand {
    #[command(
        about = "Restaura um backup validado (o mesmo que dbu restore).",
        after_help = RESTORE_HELP
    )]
    Run(Box<restore::RestoreArgs>),
    #[command(
        about = "Lista as restaurações registradas no histórico.",
        after_help = "Exemplos:
  dbu restore list
  dbu restore list --limit 5 --json"
    )]
    List(listing::RestoreListArgs),
}

#[derive(Debug, Args)]
struct ConnectionArgs {
    #[arg(
        short = 'H',
        long,
        default_value = "localhost",
        help = "Host do servidor"
    )]
    host: String,
    #[arg(short, long, help = PORT_HELP)]
    port: Option<u16>,
    #[arg(short, long, help = "Nome do banco")]
    database: String,
    #[arg(short, long, help = "Usuário")]
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
        short,
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

pub const TOOL_NAME: &str = "database-backup";

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
        Commands::Backup(group) => match group.command {
            None => backup::run(group.run).await,
            Some(BackupCommand::Run(args)) => backup::run(args).await,
            Some(BackupCommand::List(args)) => listing::backups(&args),
            Some(BackupCommand::Verify(args)) => verify::run(args).await,
        },
        Commands::Verify(args) => verify::run(args).await,
        Commands::Restore(group) => match group.command {
            None => restore::run(*group.run).await,
            Some(RestoreCommand::Run(args)) => restore::run(*args).await,
            Some(RestoreCommand::List(args)) => listing::restores(&args),
        },
        Commands::List(group) => match group.command {
            None => listing::backups(&group.backups),
            Some(ListCommand::Backups(args)) => listing::backups(&args),
            Some(ListCommand::Restores(args)) => listing::restores(&args),
        },
        Commands::Test(args) | Commands::Connection(ConnectionCommand::Test(args)) => {
            run_test_connection(args).await
        }
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
