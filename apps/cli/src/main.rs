mod backup;

use std::io::IsTerminal;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Args, Parser, Subcommand, ValueEnum};
use database_backup_adapters::postgres::{ConnectionError, Timeouts, test_connection};
use database_backup_core::{PROJECT_NAME, Password, PostgresConnectionConfig, TlsMode};

const PASSWORD_ENV: &str = "DBU_PG_PASSWORD";

#[derive(Debug, Parser)]
#[command(
    name = "database-backup",
    version,
    about = "Ferramenta de backup e restauração de bancos de dados."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    #[command(about = "Mostra o estado atual do bootstrap (sem operações de backup).")]
    Info,
    #[command(
        about = "Valida a configuração e testa a conexão com um PostgreSQL.",
        after_help = "A senha é lida de DBU_PG_PASSWORD ou, sem a variável, por um prompt sem eco."
    )]
    TestConnection(TestConnectionArgs),
    #[command(
        about = "Gera um backup completo (pg_dump, formato custom) de um PostgreSQL.",
        after_help = "A senha é lida de DBU_PG_PASSWORD ou, sem a variável, por um prompt sem eco."
    )]
    Backup(backup::BackupArgs),
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
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Some(Commands::Info) | None => {
            println!(
                "{PROJECT_NAME}: estrutura inicial pronta; backup e restore em desenvolvimento."
            );
            ExitCode::SUCCESS
        }
        Some(Commands::TestConnection(args)) => run_test_connection(args).await,
        Some(Commands::Backup(args)) => backup::run(args).await,
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
            println!(
                "Conexão OK: PostgreSQL {} em {}",
                info.server_version, config
            );
            ExitCode::SUCCESS
        }
        Err(error) => fail(exit_code_for(&error), &error.to_string()),
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

fn fail(code: u8, message: &str) -> ExitCode {
    eprintln!("Erro: {message}");
    ExitCode::from(code)
}
