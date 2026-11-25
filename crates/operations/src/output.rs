use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use database_backup_storage::{
    Encoding, LocalTarget, ORPHAN_MIN_AGE, PendingArtifact, StorageError, StoredArtifact,
};
use time::OffsetDateTime;
use time::macros::format_description;
use tracing::{info, warn};

use crate::{Cancellation, describe};

pub async fn open_target(
    output_dir: Option<PathBuf>,
    history_path: &Path,
) -> Result<LocalTarget, StorageError> {
    let output_dir = output_dir.unwrap_or_else(|| default_output_dir(history_path));
    let target = LocalTarget::new(output_dir).await?;
    if target.is_shared().await.unwrap_or(false) {
        warn!(
            operation = "backup.run",
            dir = %target.dir().display(),
            "o diretório de saída é acessível por grupo ou outros usuários; os backups ficam com permissão 0600, mas os nomes dos arquivos ficam visíveis (use chmod 700)"
        );
    }
    match target.remove_orphan_partials(ORPHAN_MIN_AGE).await {
        Ok(cleanup) => {
            for path in &cleanup.removed {
                warn!(
                    operation = "backup.run",
                    file = %path.display(),
                    "removido arquivo .partial de um backup interrompido"
                );
            }
            if !cleanup.in_use.is_empty() {
                info!(
                    operation = "backup.run",
                    count = cleanup.in_use.len(),
                    "há outros backups em andamento neste diretório"
                );
            }
        }
        Err(error) => warn!(
            operation = "backup.run",
            error = %describe(&error),
            "não foi possível verificar arquivos .partial antigos"
        ),
    }
    Ok(target)
}

fn default_output_dir(history_path: &Path) -> PathBuf {
    history_path
        .parent()
        .map_or_else(|| PathBuf::from("backups"), |dir| dir.join("backups"))
}

pub async fn commit(
    pending: PendingArtifact,
    encoding: &Encoding,
    cancellation: &Cancellation,
) -> Result<StoredArtifact, StorageError> {
    let cancel = Arc::new(AtomicBool::new(false));
    let commit = pending.commit_encoded(encoding, Arc::clone(&cancel));
    tokio::pin!(commit);
    tokio::select! {
        result = &mut commit => result,
        () = cancellation.canceled() => {
            cancel.store(true, Ordering::Relaxed);
            commit.await
        }
    }
}

#[must_use]
pub fn timestamp() -> String {
    OffsetDateTime::now_utc()
        .format(format_description!(
            "[year][month][day]T[hour][minute][second]Z"
        ))
        .unwrap_or_else(|_| OffsetDateTime::now_utc().unix_timestamp().to_string())
}
