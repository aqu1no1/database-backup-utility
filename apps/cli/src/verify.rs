use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Args;
use database_backup_adapters::postgres::PgTools;
use database_backup_core::{DatabaseEngine, Operation};
use database_backup_history::History;
use database_backup_storage::check_restore_source;
use serde::Serialize;
use tracing::{info, warn};

use crate::archive::{VerifiedSource, detect_engine, read_archive, verify_checksum};
use crate::output::{fail, success};
use crate::{RunnerArg, describe, exit};

#[derive(Debug, Args)]
pub struct VerifyArgs {
    #[arg(
        long,
        help = "Arquivo gerado pelo comando backup run (.dump, .sql, .archive ou .sqlite3)"
    )]
    file: PathBuf,
    #[arg(
        long,
        value_enum,
        help = "auto (padrão): pg_restore nativo se compatível, senão Docker (só PostgreSQL)"
    )]
    runner: Option<RunnerArg>,
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

struct Structure {
    database: Option<String>,
    objects: usize,
    tables: usize,
    dumped_from: Option<String>,
    dumped_by: Option<String>,
}

pub async fn run(args: VerifyArgs) -> ExitCode {
    let file = match std::path::absolute(&args.file) {
        Ok(path) => path,
        Err(error) => return fail(exit::INVALID_ARCHIVE, &error.to_string()),
    };
    if let Err(error) = check_restore_source(&file).await {
        return fail(exit::INVALID_ARCHIVE, &describe(&error));
    }
    let engine = match detect_engine(&file).await {
        Ok(engine) => engine,
        Err(message) => return fail(exit::INVALID_ARCHIVE, &message),
    };
    if args.runner.is_some() && engine != DatabaseEngine::Postgres {
        return fail(
            exit::INVALID_INPUT,
            &format!(
                "--runner só vale no verify de backups PostgreSQL; o verify de {} usa {}",
                engine.display_name(),
                engine.capability(Operation::Verify).note
            ),
        );
    }
    let history = match History::default_path().and_then(|path| History::open(&path)) {
        Ok(history) => history,
        Err(error) => return fail(exit::FAILURE, &describe(&error)),
    };
    let verified = match verify_checksum(&file, &history, args.allow_unverified, engine).await {
        Ok(verified) => verified,
        Err(message) => return fail(exit::INVALID_ARCHIVE, &message),
    };
    if !verified.matched_reference {
        warn!(
            operation = "backup.verify",
            "checksum não verificado (--allow-unverified); só a estrutura do arquivo será conferida"
        );
    }
    let structure = match engine {
        DatabaseEngine::Sqlite => sqlite_structure(&file).await,
        DatabaseEngine::Mysql => mysql_structure(&file).await,
        DatabaseEngine::Mongodb => mongo_structure(&file).await,
        DatabaseEngine::Postgres => {
            postgres_structure(&file, &verified, args.runner.unwrap_or(RunnerArg::Auto)).await
        }
    };
    let structure = match structure {
        Ok(structure) => structure,
        Err(code) => return code,
    };

    info!(
        operation = "backup.verify",
        engine = engine.as_str(),
        status = "succeeded",
        checksum_verified = verified.matched_reference,
        objects = structure.objects,
        "backup verificado"
    );
    let checksum = if verified.matched_reference {
        "checksum conferido"
    } else {
        "checksum NÃO verificado"
    };
    let unit = match engine {
        DatabaseEngine::Mongodb => "coleções",
        _ => "tabelas",
    };
    let mut text = format!(
        "Backup {} válido: {} ({} objetos, {} {unit}; {checksum}; sha256 {}…)",
        engine.display_name(),
        file.display(),
        structure.objects,
        structure.tables,
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
            sha256: verified.sha256.clone(),
            checksum_verified: verified.matched_reference,
            database: structure
                .database
                .or_else(|| manifest.map(|m| m.database.clone())),
            objects: structure.objects,
            tables: structure.tables,
            dumped_from: structure.dumped_from,
            dumped_by: structure.dumped_by,
            manifest_version: manifest.map(|m| m.manifest_version),
            engine: Some(engine.as_str().to_owned()),
            created_at: manifest.map(|m| m.created_at.clone()),
            tool_version: manifest.map(|m| m.tool.version.clone()),
        },
        &text,
    )
}

async fn sqlite_structure(file: &Path) -> Result<Structure, ExitCode> {
    let info = crate::sqlite::inspect(file)
        .await
        .map_err(|message| fail(exit::INVALID_ARCHIVE, &message))?;
    Ok(Structure {
        database: None,
        objects: info.objects,
        tables: info.tables,
        dumped_from: None,
        dumped_by: None,
    })
}

async fn mysql_structure(file: &Path) -> Result<Structure, ExitCode> {
    let dump = crate::mysql::inspect(file)
        .await
        .map_err(|message| fail(exit::INVALID_ARCHIVE, &message))?;
    Ok(Structure {
        objects: dump.objects(),
        tables: dump.tables,
        dumped_from: dump.dumped_from.map(|v| v.to_string()),
        dumped_by: dump.dumped_by.map(|v| v.to_string()),
        database: dump.database,
    })
}

async fn mongo_structure(file: &Path) -> Result<Structure, ExitCode> {
    let archive = crate::mongo::inspect(file)
        .await
        .map_err(|message| fail(exit::INVALID_ARCHIVE, &message))?;
    Ok(Structure {
        objects: archive.objects(),
        tables: archive.collections(),
        dumped_from: archive.server_version,
        dumped_by: archive.tool_version,
        database: Some(archive.database),
    })
}

async fn postgres_structure(
    file: &Path,
    verified: &VerifiedSource,
    runner: RunnerArg,
) -> Result<Structure, ExitCode> {
    let Some(source_version) = verified.source_version else {
        return Err(fail(
            exit::INVALID_ARCHIVE,
            "versão do PostgreSQL de origem desconhecida (sem .meta.json nem registro no histórico); não é possível escolher o pg_restore",
        ));
    };
    let tools = PgTools::detect(runner.into(), source_version)
        .await
        .map_err(|error| fail(exit::TOOLS, &error.to_string()))?;
    let archive = read_archive(&tools, file)
        .await
        .map_err(|message| fail(exit::INVALID_ARCHIVE, &message))?;
    Ok(Structure {
        database: archive.database,
        objects: archive.entries,
        tables: archive.tables,
        dumped_from: archive.dumped_from.map(|v| v.to_string()),
        dumped_by: archive.dumped_by.map(|v| v.to_string()),
    })
}
