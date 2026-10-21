mod archive;
mod backup;
mod listing;
mod output;
mod restore;
mod verify;

use std::error::Error;
use std::io::IsTerminal;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Args, Parser, Subcommand, ValueEnum};
use database_backup_adapters::postgres::{
    ConnectionError, RunnerChoice, Timeouts, test_connection,
};
use database_backup_core::{Password, PostgresConnectionConfig, TlsMode};
use serde::Serialize;
use tracing::info;

use output::{LogLevel, fail, success};

const PASSWORD_ENV: &str = "DBU_PG_PASSWORD";

macro_rules! password_note {
    () => {
        "A senha é lida de DBU_PG_PASSWORD ou, sem a variável, por um prompt sem eco."
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
  database-backup backup list --json
  database-backup backup verify --file app-20261021T120000Z.dump
  database-backup restore run --file app-20261021T120000Z.dump --target-database app_copia --user postgres

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
}

#[derive(Debug, Subcommand)]
enum ConnectionCommand {
    #[command(
        about = "Valida a configuração e testa a conexão com um PostgreSQL.",
        after_help = concat!(
            "Exemplos:\n",
            "  database-backup connection test --database app --user postgres\n",
            "  database-backup connection test --host db.interno --tls verify-full --database app --user backup\n\n",
            password_note!()
        )
    )]
    Test(TestConnectionArgs),
}

#[derive(Debug, Subcommand)]
enum BackupCommand {
    #[command(
        about = "Gera um backup completo (pg_dump, formato custom) de um PostgreSQL.",
        after_help = concat!(
            "Exemplos:\n",
            "  database-backup backup run --database app --user postgres\n",
            "  database-backup backup run --database app --user postgres --output-dir /srv/backups --runner docker\n\n",
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
  database-backup backup verify --file externo.dump --allow-unverified --json"
    )]
    Verify(verify::VerifyArgs),
}

#[derive(Debug, Subcommand)]
enum RestoreCommand {
    #[command(
        about = "Restaura um backup PostgreSQL validado em um banco de destino.",
        after_help = concat!(
            "Por padrão só cria um banco novo. Sobrescrever um banco com dados exige --overwrite e confirmação;\n",
            "o banco anterior é mantido como <destino>__previous_<data>.\n\n",
            "Exemplos:\n",
            "  database-backup restore run --file app-20261021T120000Z.dump --target-database app_copia --user postgres\n",
            "  database-backup restore run --file app-20261021T120000Z.dump --target-database app --user postgres --overwrite --confirm app\n\n",
            password_note!()
        )
    )]
    Run(restore::RestoreArgs),
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
    #[arg(long, default_value_t = 5432, help = "Porta do servidor")]
    port: u16,
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
}

impl ConnectionArgs {
    fn config(&self) -> Result<PostgresConnectionConfig, ExitCode> {
        PostgresConnectionConfig::new(
            &self.host,
            self.port,
            &self.database,
            &self.user,
            self.tls.into(),
        )
        .map_err(|error| fail(exit::INVALID_INPUT, &error.to_string()))
    }
}

#[derive(Debug, Args)]
struct TestConnectionArgs {
    #[command(flatten)]
    connection: ConnectionArgs,
    #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u64).range(1..=300), help = "Timeout em segundos para conexão e consulta")]
    timeout: u64,
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
        Commands::Restore(RestoreCommand::Run(args)) => restore::run(args).await,
        Commands::Restore(RestoreCommand::List(args)) => listing::restores(&args),
    }
}

async fn run_test_connection(args: TestConnectionArgs) -> ExitCode {
    let config = match args.connection.config() {
        Ok(config) => config,
        Err(code) => return code,
    };
    let password = match read_password(&config) {
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

fn read_password(config: &PostgresConnectionConfig) -> Result<(Password, &'static str), String> {
    if let Ok(value) = std::env::var(PASSWORD_ENV) {
        return Ok((Password::new(value), "env:DBU_PG_PASSWORD"));
    }
    if !std::io::stdin().is_terminal() {
        return Err(format!(
            "senha não informada: defina {PASSWORD_ENV} ou execute em um terminal interativo"
        ));
    }
    rpassword::prompt_password(format!("Senha para {config}: "))
        .map(|value| (Password::new(value), "prompt"))
        .map_err(|_| "não foi possível ler a senha do terminal".to_owned())
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
