use clap::{Parser, Subcommand};
use database_backup_core::PROJECT_NAME;

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
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Some(Commands::Info) | None => {
            println!(
                "{PROJECT_NAME}: estrutura inicial pronta; backup e restore em desenvolvimento."
            );
        }
    }
}
