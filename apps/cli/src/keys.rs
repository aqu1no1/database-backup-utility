use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Subcommand};
use database_backup_storage::{Identities, generate_identity};
use serde::Serialize;
use tracing::info;

use crate::output::{fail, success};
use crate::{describe, exit};

#[derive(Debug, Subcommand)]
pub enum KeyCommand {
    #[command(
        about = "Gera uma chave age nova: a privada vai para o arquivo (0600), a pública sai no stdout.",
        after_help = "Exemplos:
  dbu key generate -o ~/.config/dbu/backup.key
  dbu key generate -o /mnt/cofre/recuperacao.key --json

Guarde uma cópia da chave privada fora da máquina dos backups. Sem ela, um backup criptografado
não pode ser recuperado por ninguém. Ver docs/criptografia.md."
    )]
    Generate(GenerateArgs),
    #[command(
        about = "Mostra a chave pública de um arquivo de chave privada age.",
        after_help = "Exemplos:
  dbu key public ~/.config/dbu/backup.key"
    )]
    Public(PublicArgs),
}

#[derive(Debug, Args)]
pub struct GenerateArgs {
    #[arg(
        short,
        long,
        value_name = "ARQUIVO",
        help = "Arquivo da chave privada (não pode existir)"
    )]
    output: PathBuf,
}

#[derive(Debug, Args)]
pub struct PublicArgs {
    #[arg(value_name = "ARQUIVO", help = "Arquivo da chave privada age")]
    identity: PathBuf,
}

#[derive(Serialize)]
struct KeyResult {
    identity_file: String,
    public_keys: Vec<String>,
}

pub fn run(command: KeyCommand) -> ExitCode {
    match command {
        KeyCommand::Generate(args) => generate(&args),
        KeyCommand::Public(args) => public(&args),
    }
}

fn generate(args: &GenerateArgs) -> ExitCode {
    let public = match generate_identity(&args.output) {
        Ok(public) => public,
        Err(error) => return fail(exit::FAILURE, &describe(&error)),
    };
    info!(
        operation = "key.generate",
        file = %args.output.display(),
        status = "succeeded",
        "chave age gerada"
    );
    let text = format!(
        "{public}\nChave privada gravada em {} (0600). Faça uma cópia offline dela: sem a chave privada, os backups criptografados não podem ser recuperados.",
        args.output.display()
    );
    success(
        &KeyResult {
            identity_file: args.output.display().to_string(),
            public_keys: vec![public],
        },
        &text,
    )
}

fn public(args: &PublicArgs) -> ExitCode {
    let identities = match Identities::read_file(&args.identity) {
        Ok(identities) => identities,
        Err(error) => return fail(exit::INVALID_INPUT, &describe(&error)),
    };
    let public_keys = identities.public_keys();
    let text = public_keys.join("\n");
    success(
        &KeyResult {
            identity_file: args.identity.display().to_string(),
            public_keys,
        },
        &text,
    )
}
