use std::path::Path;

use database_backup_adapters::postgres::{ArchiveInfo, PgTools, PgVersion};
use database_backup_history::History;
use database_backup_storage::{StoredMetadata, read_metadata, sha256_file};

use crate::backup::ENGINE;
use crate::describe;

pub struct VerifiedSource {
    pub artifact_id: Option<String>,
    pub sha256: String,
    pub matched_reference: bool,
    pub source_version: Option<PgVersion>,
    pub manifest: Option<StoredMetadata>,
}

pub async fn verify_checksum(
    file: &Path,
    history: &History,
    allow_unverified: bool,
) -> Result<VerifiedSource, String> {
    let metadata = read_metadata(file)
        .await
        .map_err(|error| describe(&error))?;
    if let Some(manifest) = &metadata {
        check_manifest(file, manifest)?;
    }
    let artifact = history
        .find_artifact_by_location(&file.display().to_string())
        .map_err(|error| describe(&error))?;
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

    let (_, actual) = sha256_file(file).await.map_err(|error| describe(&error))?;
    match expected {
        Some(expected) if expected != actual => Err(format!(
            "checksum divergente: esperado {expected}, calculado {actual}; o arquivo foi alterado ou corrompido"
        )),
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
        None => Err(
            "não há checksum conhecido para este arquivo (.meta.json ou histórico); use --allow-unverified para continuar mesmo assim"
                .to_owned(),
        ),
    }
}

fn check_manifest(file: &Path, manifest: &StoredMetadata) -> Result<(), String> {
    let name = file
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    if manifest.file_name != name {
        return Err(format!(
            "o manifesto pertence a outro arquivo ({}), não a {name}",
            manifest.file_name
        ));
    }
    if manifest.engine != ENGINE {
        return Err(format!(
            "o manifesto descreve um backup de {}, não de {ENGINE}",
            manifest.engine
        ));
    }
    let size = std::fs::metadata(file)
        .map(|metadata| metadata.len())
        .map_err(|error| format!("não foi possível ler {}: {error}", file.display()))?;
    if size != manifest.size_bytes {
        return Err(format!(
            "tamanho divergente: o manifesto registra {} bytes e o arquivo tem {size}; o arquivo foi alterado ou está incompleto",
            manifest.size_bytes
        ));
    }
    Ok(())
}

pub async fn read_archive(tools: &PgTools, file: &Path) -> Result<ArchiveInfo, String> {
    let archive = tools
        .restore_list(file)
        .await
        .map_err(|error| format!("o arquivo não é um backup válido do pg_dump: {error}"))?;
    if let Some(by) = archive.dumped_by
        && by.major > tools.version().major
    {
        return Err(format!(
            "o backup foi gerado pelo pg_dump {by}, mais novo que o pg_restore {} disponível",
            tools.version()
        ));
    }
    Ok(archive)
}

pub fn check_target(archive: &ArchiveInfo, target: PgVersion) -> Result<(), String> {
    match archive.dumped_from {
        Some(from) if from.major > target.major => Err(format!(
            "o backup veio do PostgreSQL {from}, mais novo que o servidor de destino {target}"
        )),
        _ => Ok(()),
    }
}
