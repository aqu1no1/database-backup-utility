use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use clap::Args;
use database_backup_core::DatabaseEngine;
use database_backup_history::History;
use database_backup_storage::{
    Compression, Encoding, Identities, PendingArtifact, Recipients, StorageError, StoredArtifact,
    StoredMetadata, WorkDir, open_encoded, read_metadata,
};
use tokio::io::AsyncReadExt;
use tracing::{info, warn};

use crate::output::fail;
use crate::{describe, exit};

const AGE_MAGIC: &[u8] = b"age-encryption.org/";
const AGE_ARMOR_MAGIC: &[u8] = b"-----BEGIN AGE ENCRYPTED FILE-----";
const ZSTD_MAGIC: &[u8] = &[0x28, 0xb5, 0x2f, 0xfd];

#[derive(Debug, Default, Clone, Args)]
pub struct EncodingArgs {
    #[arg(
        long,
        help = "Comprime com zstd quando o formato do SGBD não é comprimido (o custom do pg_dump já é)"
    )]
    pub(crate) compress: bool,
    #[arg(
        long = "recipient",
        value_name = "CHAVE_PUBLICA",
        help = "Criptografa com age para esta chave pública (age1...); repita para mais de uma"
    )]
    pub(crate) recipients: Vec<String>,
    #[arg(
        long,
        value_name = "ARQUIVO",
        help = "Arquivo com chaves públicas age, uma por linha (# comenta)"
    )]
    pub(crate) recipients_file: Vec<PathBuf>,
}

impl EncodingArgs {
    pub fn resolve(&self, engine: DatabaseEngine) -> Result<Encoding, ExitCode> {
        let compression = match (self.compress, engine) {
            (false, _) => None,
            (true, DatabaseEngine::Postgres) => {
                info!(
                    operation = "backup.run",
                    "o formato custom do pg_dump já é comprimido; o zstd não é aplicado de novo"
                );
                None
            }
            (true, _) => Some(Compression::Zstd),
        };
        let mut recipients = Recipients::default();
        for key in &self.recipients {
            recipients
                .add_key(key)
                .map_err(|error| fail(exit::INVALID_INPUT, &describe(&error)))?;
        }
        for path in &self.recipients_file {
            recipients
                .add_file(path)
                .map_err(|error| fail(exit::INVALID_INPUT, &describe(&error)))?;
        }
        Ok(Encoding::new(compression, recipients))
    }
}

pub async fn commit(
    pending: PendingArtifact,
    encoding: &Encoding,
) -> Result<StoredArtifact, StorageError> {
    let cancel = Arc::new(AtomicBool::new(false));
    let commit = pending.commit_encoded(encoding, Arc::clone(&cancel));
    tokio::pin!(commit);
    tokio::select! {
        result = &mut commit => result,
        _ = tokio::signal::ctrl_c() => {
            cancel.store(true, Ordering::Relaxed);
            commit.await
        }
    }
}

#[must_use]
pub fn summary(artifact: &StoredArtifact) -> Option<String> {
    let encoding = artifact.encoding.as_ref()?;
    let mut parts = Vec::new();
    if let Some(compression) = encoding.compression {
        parts.push(format!("comprimido com {}", compression.as_str()));
    }
    if let Some(encryption) = encoding.encryption {
        parts.push(format!("criptografado com {}", encryption.as_str()));
    }
    Some(format!(
        "{}; conteúdo original de {}",
        parts.join(", "),
        crate::backup::human_size(encoding.content.size_bytes)
    ))
}

#[must_use]
pub fn compression_name(artifact: &StoredArtifact) -> Option<&'static str> {
    artifact
        .encoding
        .as_ref()
        .and_then(|encoding| encoding.compression)
        .map(Compression::as_str)
}

#[must_use]
pub fn encryption_name(artifact: &StoredArtifact) -> Option<&'static str> {
    artifact
        .encoding
        .as_ref()
        .and_then(|encoding| encoding.encryption)
        .map(|encryption| encryption.as_str())
}

pub struct OpenedBackup {
    path: PathBuf,
    manifest: Option<StoredMetadata>,
    _work: Option<WorkDir>,
}

impl OpenedBackup {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn encoded_manifest(&self) -> Option<&StoredMetadata> {
        self.manifest.as_ref()
    }
}

pub async fn open(
    file: &Path,
    identity: Option<&Path>,
    operation: &'static str,
) -> Result<OpenedBackup, ExitCode> {
    let manifest = read_metadata(file)
        .await
        .map_err(|error| fail(exit::INVALID_ARCHIVE, &describe(&error)))?;
    let Some(manifest) = manifest.filter(|manifest| manifest.content.is_some()) else {
        if looks_encoded(file).await {
            return Err(fail(
                exit::INVALID_ARCHIVE,
                "o arquivo parece comprimido (zstd) ou criptografado (age), mas não tem o manifesto .meta.json; abra-o com age -d e zstd -d e verifique o resultado com --allow-unverified",
            ));
        }
        if identity.is_some() {
            warn!(
                operation,
                "--identity ignorado: o backup não é criptografado"
            );
        }
        return Ok(OpenedBackup {
            path: file.to_owned(),
            manifest: None,
            _work: None,
        });
    };
    let identities = match (manifest.encryption, identity) {
        (Some(_), Some(path)) => Some(
            Identities::read_file(path)
                .map_err(|error| fail(exit::INVALID_INPUT, &describe(&error)))?,
        ),
        (Some(_), None) => {
            return Err(fail(
                exit::INVALID_INPUT,
                &describe(&StorageError::MissingIdentity),
            ));
        }
        (None, Some(_)) => {
            warn!(
                operation,
                "--identity ignorado: o backup não é criptografado"
            );
            None
        }
        (None, None) => None,
    };
    let base = History::default_path()
        .map(|path| {
            path.parent()
                .map_or_else(|| PathBuf::from("work"), |dir| dir.join("work"))
        })
        .map_err(|error| fail(exit::FAILURE, &describe(&error)))?;
    let work = WorkDir::create(&base).map_err(|error| fail(exit::FAILURE, &describe(&error)))?;
    let cancel = Arc::new(AtomicBool::new(false));
    let opened = {
        let opening = open_encoded(file, &manifest, identities, &work, Arc::clone(&cancel));
        tokio::pin!(opening);
        tokio::select! {
            result = &mut opening => result,
            _ = tokio::signal::ctrl_c() => {
                cancel.store(true, Ordering::Relaxed);
                opening.await
            }
        }
    };
    let path = opened.map_err(|error| {
        let code = match error {
            StorageError::Canceled => exit::CANCELED,
            StorageError::MissingIdentity => exit::INVALID_INPUT,
            StorageError::Io { .. } => exit::FAILURE,
            _ => exit::INVALID_ARCHIVE,
        };
        fail(code, &describe(&error))
    })?;
    info!(
        operation,
        compression = manifest.compression.map(Compression::as_str),
        encryption = manifest.encryption.map(|encryption| encryption.as_str()),
        "backup aberto num diretório temporário privado"
    );
    Ok(OpenedBackup {
        path,
        manifest: Some(manifest),
        _work: Some(work),
    })
}

async fn looks_encoded(file: &Path) -> bool {
    let Ok(opened) = tokio::fs::File::open(file).await else {
        return false;
    };
    let mut head = Vec::with_capacity(AGE_ARMOR_MAGIC.len());
    if opened
        .take(AGE_ARMOR_MAGIC.len() as u64)
        .read_to_end(&mut head)
        .await
        .is_err()
    {
        return false;
    }
    [AGE_MAGIC, AGE_ARMOR_MAGIC, ZSTD_MAGIC]
        .iter()
        .any(|magic| head.starts_with(magic))
}
