use std::path::PathBuf;
use std::process::ExitCode;

use clap::Args;
use database_backup_adapters::postgres::PgTools;
use database_backup_history::History;
use database_backup_storage::check_restore_source;
use serde::Serialize;
use tracing::{info, warn};

use crate::archive::{read_archive, verify_checksum};
use crate::output::{fail, success};
use crate::{RunnerArg, describe, exit};

#[derive(Debug, Args)]
pub struct VerifyArgs {
    #[arg(long, help = "Arquivo .dump gerado pelo comando backup run")]
    file: PathBuf,
    #[arg(
        long,
        value_enum,
        default_value_t = RunnerArg::Auto,
        help = "auto: pg_restore nativo se compatível, senão Docker"
    )]
    runner: RunnerArg,
    #[arg(
        long,
        help = "Aceita arquivos sem checksum conhecido (.meta.json ou histórico)"
    )]
    allow_unverified: bool,
}

#[derive(Serialize)]
struct VerifyResult {
    file: String,
    sha256: String,
    checksum_verified: bool,
    database: Option<String>,
    objects: usize,
    tables: usize,
    dumped_from: Option<String>,
    dumped_by: Option<String>,
    manifest_version: Option<u32>,
    engine: Option<String>,
    created_at: Option<String>,
    tool_version: Option<String>,
}

pub async fn run(args: VerifyArgs) -> ExitCode {
    let file = match std::path::absolute(&args.file) {
        Ok(path) => path,
        Err(error) => return fail(exit::INVALID_ARCHIVE, &error.to_string()),
    };
    if let Err(error) = check_restore_source(&file).await {
        return fail(exit::INVALID_ARCHIVE, &describe(&error));
    }
    let history = match History::default_path().and_then(|path| History::open(&path)) {
        Ok(history) => history,
        Err(error) => return fail(exit::FAILURE, &describe(&error)),
    };
    let verified = match verify_checksum(&file, &history, args.allow_unverified).await {
        Ok(verified) => verified,
        Err(message) => return fail(exit::INVALID_ARCHIVE, &message),
    };
    if !verified.matched_reference {
        warn!(
            operation = "backup.verify",
            "checksum não verificado (--allow-unverified); só a estrutura do arquivo será conferida"
        );
    }
    let Some(source_version) = verified.source_version else {
        return fail(
            exit::INVALID_ARCHIVE,
            "versão do PostgreSQL de origem desconhecida (sem .meta.json nem registro no histórico); não é possível escolher o pg_restore",
        );
    };
    let tools = match PgTools::detect(args.runner.into(), source_version).await {
        Ok(tools) => tools,
        Err(error) => return fail(exit::TOOLS, &error.to_string()),
    };
    let archive = match read_archive(&tools, &file).await {
        Ok(archive) => archive,
        Err(message) => return fail(exit::INVALID_ARCHIVE, &message),
    };

    info!(
        operation = "backup.verify",
        status = "succeeded",
        checksum_verified = verified.matched_reference,
        objects = archive.entries,
        "backup verificado"
    );
    let checksum = if verified.matched_reference {
        "checksum conferido"
    } else {
        "checksum NÃO verificado"
    };
    let mut text = format!(
        "Backup válido: {} ({} objetos, {} tabelas; {checksum}; sha256 {}…)",
        file.display(),
        archive.entries,
        archive.tables,
        &verified.sha256[..12]
    );
    if let Some(manifest) = &verified.manifest {
        text.push_str(&format!(
            "\nManifesto v{}: {} {}, criado em {} por {} {}",
            manifest.manifest_version,
            manifest.engine,
            manifest.database,
            manifest.created_at,
            manifest.tool.name,
            manifest.tool.version
        ));
    }
    let manifest = verified.manifest.as_ref();
    success(
        &VerifyResult {
            file: file.display().to_string(),
            sha256: verified.sha256,
            checksum_verified: verified.matched_reference,
            database: archive.database,
            objects: archive.entries,
            tables: archive.tables,
            dumped_from: archive.dumped_from.map(|v| v.to_string()),
            dumped_by: archive.dumped_by.map(|v| v.to_string()),
            manifest_version: manifest.map(|m| m.manifest_version),
            engine: manifest.map(|m| m.engine.clone()),
            created_at: manifest.map(|m| m.created_at.clone()),
            tool_version: manifest.map(|m| m.tool.version.clone()),
        },
        &text,
    )
}
