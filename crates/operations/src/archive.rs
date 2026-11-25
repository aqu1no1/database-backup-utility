use std::path::Path;

use database_backup_adapters::mongo::ARCHIVE_MAGIC as MONGO_ARCHIVE_MAGIC;
use database_backup_adapters::mysql::{Flavor, dump_flavor};
use database_backup_adapters::postgres::{ArchiveInfo, PgTools, PgVersion};
use database_backup_adapters::sqlite::{SqliteError, check_database_file};
use database_backup_core::DatabaseEngine;
use database_backup_history::{ArtifactRecord, History};
use database_backup_storage::{StoredMetadata, read_metadata, sha256_file};
use tokio::io::AsyncReadExt;

use crate::{ErrorCode, OperationError};

const SQL_DUMP_HEAD_BYTES: usize = 512;
const POSTGRES_ARCHIVE_MAGIC: &[u8] = b"PGDMP";

pub struct VerifiedSource {
    pub artifact_id: Option<String>,
    pub sha256: String,
    pub matched_reference: bool,
    pub source_version: Option<PgVersion>,
    pub manifest: Option<StoredMetadata>,
}

pub async fn detect_engine(file: &Path) -> Result<Option<DatabaseEngine>, OperationError> {
    if let Some(manifest) = read_metadata(file)
        .await
        .map_err(|error| OperationError::source_file(&error))?
    {
        return manifest.engine.parse().map(Some).map_err(|_| {
            incompatible(format!(
                "o manifesto descreve um backup de {}, que esta versão não suporta",
                manifest.engine
            ))
        });
    }
    let path = file.to_owned();
    let header = tokio::task::spawn_blocking(move || check_database_file(&path))
        .await
        .map_err(|error| OperationError::internal(&error))?;
    match header {
        Ok(()) => Ok(Some(DatabaseEngine::Sqlite)),
        Err(SqliteError::NotSqlite(_)) if starts_with(file, POSTGRES_ARCHIVE_MAGIC).await? => {
            Ok(Some(DatabaseEngine::Postgres))
        }
        Err(SqliteError::NotSqlite(_)) if starts_with(file, &MONGO_ARCHIVE_MAGIC).await? => {
            Ok(Some(DatabaseEngine::Mongodb))
        }
        Err(SqliteError::NotSqlite(_)) => {
            let head = read_head(file, SQL_DUMP_HEAD_BYTES).await?;
            Ok(
                dump_flavor(&String::from_utf8_lossy(&head)).map(|flavor| match flavor {
                    Flavor::Mysql => DatabaseEngine::Mysql,
                    Flavor::Mariadb => DatabaseEngine::Mariadb,
                }),
            )
        }
        Err(error) => Err(OperationError::from_source(
            ErrorCode::BackupIncompatible,
            &error,
        )),
    }
}

async fn read_head(file: &Path, limit: usize) -> Result<Vec<u8>, OperationError> {
    let opened = tokio::fs::File::open(file)
        .await
        .map_err(|error| unreadable(file, &error))?;
    let mut head = Vec::with_capacity(limit);
    opened
        .take(limit as u64)
        .read_to_end(&mut head)
        .await
        .map_err(|error| unreadable(file, &error))?;
    Ok(head)
}

async fn starts_with(file: &Path, prefix: &[u8]) -> Result<bool, OperationError> {
    let mut head = vec![0_u8; prefix.len()];
    let mut opened = tokio::fs::File::open(file)
        .await
        .map_err(|error| unreadable(file, &error))?;
    match opened.read_exact(&mut head).await {
        Ok(_) => Ok(head == prefix),
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
        Err(error) => Err(unreadable(file, &error)),
    }
}

fn incompatible(message: impl Into<String>) -> OperationError {
    OperationError::new(ErrorCode::BackupIncompatible, message)
}

fn corrupted(message: impl Into<String>) -> OperationError {
    OperationError::new(ErrorCode::BackupCorrupted, message)
}

fn unreadable(file: &Path, error: &std::io::Error) -> OperationError {
    incompatible(format!("não foi possível ler {}: {error}", file.display()))
}

pub fn verify_checksum<'a>(
    file: &'a Path,
    location: &Path,
    history: &History,
    allow_unverified: bool,
    engine: DatabaseEngine,
) -> impl Future<Output = Result<VerifiedSource, OperationError>> + Send + use<'a> {
    let artifact = history
        .find_artifact_by_location(&location.display().to_string())
        .map_err(|error| OperationError::from_source(ErrorCode::ChecksumMissing, &error));
    verify_against(file, artifact, allow_unverified, engine)
}

async fn verify_against(
    file: &Path,
    artifact: Result<Option<ArtifactRecord>, OperationError>,
    allow_unverified: bool,
    engine: DatabaseEngine,
) -> Result<VerifiedSource, OperationError> {
    let metadata = read_metadata(file)
        .await
        .map_err(|error| OperationError::source_file(&error))?;
    if let Some(manifest) = &metadata {
        check_manifest(file, manifest, engine)?;
    }
    let artifact = artifact?;
    let source_version = metadata
        .as_ref()
        .and_then(|metadata| PgVersion::parse(&metadata.server_version))
        .or_else(|| {
            artifact
                .as_ref()
                .and_then(|a| a.tool_version.as_deref())
                .and_then(PgVersion::parse)
        });
    let expected = metadata
        .as_ref()
        .map(|metadata| metadata.sha256.clone())
        .or_else(|| artifact.as_ref().and_then(|a| a.checksum_sha256.clone()));

    let (_, actual) = sha256_file(file)
        .await
        .map_err(|error| OperationError::source_file(&error))?;
    match expected {
        Some(expected) if expected != actual => Err(corrupted(format!(
            "checksum divergente: esperado {expected}, calculado {actual}; o arquivo foi alterado ou corrompido"
        ))),
        Some(_) => Ok(VerifiedSource {
            artifact_id: artifact.map(|a| a.id),
            sha256: actual,
            matched_reference: true,
            source_version,
            manifest: metadata,
        }),
        None if allow_unverified => Ok(VerifiedSource {
            artifact_id: None,
            sha256: actual,
            matched_reference: false,
            source_version,
            manifest: None,
        }),
        None => Err(OperationError::new(
            ErrorCode::ChecksumMissing,
            "não há checksum conhecido para este arquivo (.meta.json ou histórico); use --allow-unverified para continuar mesmo assim",
        )),
    }
}

fn check_manifest(
    file: &Path,
    manifest: &StoredMetadata,
    engine: DatabaseEngine,
) -> Result<(), OperationError> {
    let name = file
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    if manifest.file_name != name {
        return Err(incompatible(format!(
            "o manifesto pertence a outro arquivo ({}), não a {name}",
            manifest.file_name
        )));
    }
    if manifest.engine.parse::<DatabaseEngine>() != Ok(engine) {
        return Err(OperationError::new(
            ErrorCode::EngineMismatch,
            format!(
                "o manifesto descreve um backup de {}, não de {}",
                manifest.engine,
                engine.display_name()
            ),
        ));
    }
    let size = std::fs::metadata(file)
        .map(|metadata| metadata.len())
        .map_err(|error| unreadable(file, &error))?;
    if size != manifest.size_bytes {
        return Err(corrupted(format!(
            "tamanho divergente: o manifesto registra {} bytes e o arquivo tem {size}; o arquivo foi alterado ou está incompleto",
            manifest.size_bytes
        )));
    }
    Ok(())
}

pub async fn read_archive(tools: &PgTools, file: &Path) -> Result<ArchiveInfo, OperationError> {
    let archive = tools.restore_list(file).await.map_err(|error| {
        corrupted(format!(
            "o arquivo não é um backup válido do pg_dump: {error}"
        ))
    })?;
    if let Some(by) = archive.dumped_by
        && by.major > tools.version().major
    {
        return Err(incompatible(format!(
            "o backup foi gerado pelo pg_dump {by}, mais novo que o pg_restore {} disponível",
            tools.version()
        )));
    }
    Ok(archive)
}

pub fn check_target(archive: &ArchiveInfo, target: PgVersion) -> Result<(), OperationError> {
    match archive.dumped_from {
        Some(from) if from.major > target.major => Err(incompatible(format!(
            "o backup veio do PostgreSQL {from}, mais novo que o servidor de destino {target}"
        ))),
        _ => Ok(()),
    }
}
