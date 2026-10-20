use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use time::OffsetDateTime;
use time::macros::format_description;
use tokio::fs::{self, File, OpenOptions};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

const BUFFER_SIZE: usize = 64 * 1024;
const PARTIAL_SUFFIX: &str = ".partial";
pub const METADATA_SUFFIX: &str = ".meta.json";

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("falha de E/S em {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("já existe um arquivo em {0}")]
    AlreadyExists(PathBuf),
    #[error("falha ao ler o stream do backup")]
    Read(#[source] io::Error),
    #[error("falha ao gerar os metadados do backup")]
    Metadata(#[source] serde_json::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredArtifact {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArtifactMetadata<'a> {
    pub format: &'a str,
    pub sha256: &'a str,
    pub size_bytes: u64,
    pub database: &'a str,
    pub host: &'a str,
    pub port: u16,
    pub server_version: &'a str,
    pub pg_dump_version: &'a str,
    pub runner: &'a str,
    pub created_at: &'a str,
    pub execution_id: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct StoredMetadata {
    pub format: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub database: String,
    pub server_version: String,
    pub pg_dump_version: String,
}

#[derive(Debug, Clone)]
pub struct LocalTarget {
    dir: PathBuf,
}

impl LocalTarget {
    pub async fn new(dir: impl Into<PathBuf>) -> Result<Self, StorageError> {
        let dir = dir.into();
        if !fs::try_exists(&dir).await.unwrap_or(false) {
            fs::create_dir_all(&dir).await.map_err(io_error(&dir))?;
            restrict_dir(&dir).await?;
        }
        Ok(Self { dir })
    }

    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub async fn begin(&self, file_name: &str) -> Result<PendingArtifact, StorageError> {
        let final_path = self.dir.join(file_name);
        if fs::try_exists(&final_path).await.unwrap_or(false) {
            return Err(StorageError::AlreadyExists(final_path));
        }
        let partial_path = self.dir.join(format!("{file_name}{PARTIAL_SUFFIX}"));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let file = options
            .open(&partial_path)
            .await
            .map_err(io_error(&partial_path))?;
        Ok(PendingArtifact {
            file: Some(file),
            partial_path,
            final_path,
            hasher: Sha256::new(),
            size_bytes: 0,
            committed: false,
        })
    }
}

pub struct PendingArtifact {
    file: Option<File>,
    partial_path: PathBuf,
    final_path: PathBuf,
    hasher: Sha256,
    size_bytes: u64,
    committed: bool,
}

impl PendingArtifact {
    #[must_use]
    pub fn partial_path(&self) -> &Path {
        &self.partial_path
    }

    pub async fn copy_from<R>(&mut self, reader: &mut R) -> Result<u64, StorageError>
    where
        R: AsyncRead + Unpin,
    {
        let path = self.partial_path.clone();
        let file = self.file.as_mut().ok_or_else(|| StorageError::Io {
            path: path.clone(),
            source: io::Error::other("arquivo já finalizado"),
        })?;
        let mut buffer = vec![0_u8; BUFFER_SIZE];
        loop {
            let read = reader.read(&mut buffer).await.map_err(StorageError::Read)?;
            if read == 0 {
                break;
            }
            let chunk = &buffer[..read];
            self.hasher.update(chunk);
            file.write_all(chunk).await.map_err(io_error(&path))?;
            self.size_bytes += read as u64;
        }
        Ok(self.size_bytes)
    }

    pub async fn finish_writing(&mut self) -> Result<(), StorageError> {
        if let Some(mut file) = self.file.take() {
            file.flush().await.map_err(io_error(&self.partial_path))?;
            file.sync_all()
                .await
                .map_err(io_error(&self.partial_path))?;
        }
        Ok(())
    }

    pub async fn commit(mut self) -> Result<StoredArtifact, StorageError> {
        self.finish_writing().await?;
        if fs::try_exists(&self.final_path).await.unwrap_or(false) {
            return Err(StorageError::AlreadyExists(self.final_path.clone()));
        }
        fs::rename(&self.partial_path, &self.final_path)
            .await
            .map_err(io_error(&self.final_path))?;
        self.committed = true;
        sync_dir(&self.final_path).await?;
        let sha256 = to_hex(&std::mem::take(&mut self.hasher).finalize());
        Ok(StoredArtifact {
            path: self.final_path.clone(),
            size_bytes: self.size_bytes,
            sha256,
        })
    }
}

impl Drop for PendingArtifact {
    fn drop(&mut self) {
        if !self.committed {
            drop(self.file.take());
            let _ = std::fs::remove_file(&self.partial_path);
        }
    }
}

pub async fn write_metadata(
    artifact: &Path,
    metadata: &ArtifactMetadata<'_>,
) -> Result<PathBuf, StorageError> {
    let mut json = serde_json::to_vec_pretty(metadata).map_err(StorageError::Metadata)?;
    json.push(b'\n');
    let final_path = metadata_path(artifact);
    let mut partial_path = final_path.as_os_str().to_owned();
    partial_path.push(PARTIAL_SUFFIX);
    let partial_path = PathBuf::from(partial_path);

    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&partial_path)
        .await
        .map_err(io_error(&partial_path))?;
    file.write_all(&json)
        .await
        .map_err(io_error(&partial_path))?;
    file.sync_all().await.map_err(io_error(&partial_path))?;
    drop(file);
    fs::rename(&partial_path, &final_path)
        .await
        .map_err(io_error(&final_path))?;
    sync_dir(&final_path).await?;
    Ok(final_path)
}

pub fn metadata_path(artifact: &Path) -> PathBuf {
    let mut path = artifact.as_os_str().to_owned();
    path.push(METADATA_SUFFIX);
    PathBuf::from(path)
}

pub async fn read_metadata(artifact: &Path) -> Result<Option<StoredMetadata>, StorageError> {
    let path = metadata_path(artifact);
    match fs::read(&path).await {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(StorageError::Metadata),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(StorageError::Io { path, source }),
    }
}

pub async fn sha256_file(path: &Path) -> Result<(u64, String), StorageError> {
    let mut file = File::open(path).await.map_err(io_error(path))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; BUFFER_SIZE];
    let mut size = 0_u64;
    loop {
        let read = file.read(&mut buffer).await.map_err(io_error(path))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size += read as u64;
    }
    Ok((size, to_hex(&hasher.finalize())))
}

#[must_use]
pub fn backup_file_name(database: &str, at: OffsetDateTime) -> String {
    let safe: String = database
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let stamp = at
        .to_offset(time::UtcOffset::UTC)
        .format(format_description!(
            "[year][month][day]T[hour][minute][second]Z"
        ))
        .unwrap_or_else(|_| at.unix_timestamp().to_string());
    format!("{safe}-{stamp}.dump")
}

fn to_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(64), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> StorageError + '_ {
    move |source| StorageError::Io {
        path: path.to_owned(),
        source,
    }
}

async fn restrict_dir(dir: &Path) -> Result<(), StorageError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .await
            .map_err(io_error(dir))?;
    }
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

async fn sync_dir(file: &Path) -> Result<(), StorageError> {
    #[cfg(unix)]
    if let Some(dir) = file.parent() {
        let handle = File::open(dir).await.map_err(io_error(dir))?;
        handle.sync_all().await.map_err(io_error(dir))?;
    }
    #[cfg(not(unix))]
    let _ = file;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use time::macros::datetime;

    async fn target() -> (TempDir, LocalTarget) {
        let dir = TempDir::new().expect("diretório temporário");
        let target = LocalTarget::new(dir.path().join("backups"))
            .await
            .expect("destino");
        (dir, target)
    }

    #[tokio::test]
    async fn commit_publishes_file_with_checksum() {
        let (_dir, target) = target().await;
        let mut pending = target.begin("db.dump").await.expect("início");
        let partial = pending.partial_path().to_owned();

        pending
            .copy_from(&mut &b"hello world"[..])
            .await
            .expect("cópia");
        let stored = pending.commit().await.expect("commit");

        assert_eq!(stored.path, target.dir().join("db.dump"));
        assert_eq!(stored.size_bytes, 11);
        assert_eq!(
            stored.sha256,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
        assert_eq!(
            std::fs::read(&stored.path).expect("arquivo"),
            b"hello world"
        );
        assert!(!partial.exists());
    }

    #[tokio::test]
    async fn dropping_without_commit_removes_partial_file() {
        let (_dir, target) = target().await;
        let mut pending = target.begin("db.dump").await.expect("início");
        pending
            .copy_from(&mut &b"parcial"[..])
            .await
            .expect("cópia");
        let partial = pending.partial_path().to_owned();
        assert!(partial.exists());

        drop(pending);

        assert!(!partial.exists());
        assert!(!target.dir().join("db.dump").exists());
    }

    #[tokio::test]
    async fn refuses_to_overwrite_existing_backup() {
        let (_dir, target) = target().await;
        std::fs::write(target.dir().join("db.dump"), b"anterior").expect("arquivo existente");

        let result = target.begin("db.dump").await;

        assert!(matches!(result, Err(StorageError::AlreadyExists(_))));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn created_directory_and_partial_are_private() {
        use std::os::unix::fs::PermissionsExt;

        let (_dir, target) = target().await;
        let pending = target.begin("db.dump").await.expect("início");

        let dir_mode = std::fs::metadata(target.dir())
            .expect("dir")
            .permissions()
            .mode();
        let file_mode = std::fs::metadata(pending.partial_path())
            .expect("partial")
            .permissions()
            .mode();
        assert_eq!(dir_mode & 0o777, 0o700);
        assert_eq!(file_mode & 0o777, 0o600);
    }

    #[tokio::test]
    async fn metadata_is_written_next_to_artifact() {
        let (_dir, target) = target().await;
        let artifact = target.dir().join("db.dump");
        let metadata = ArtifactMetadata {
            format: "pg_dump-custom",
            sha256: "abc",
            size_bytes: 3,
            database: "db",
            host: "localhost",
            port: 5432,
            server_version: "17.11",
            pg_dump_version: "17.11",
            runner: "docker",
            created_at: "2026-10-20T19:00:00Z",
            execution_id: "exec-1",
        };

        let path = write_metadata(&artifact, &metadata)
            .await
            .expect("metadados");
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).expect("arquivo")).expect("json");

        assert_eq!(path, target.dir().join("db.dump.meta.json"));
        assert_eq!(json["sha256"], "abc");
        assert_eq!(json["runner"], "docker");
        let keys: Vec<&String> = json.as_object().expect("objeto").keys().collect();
        assert!(
            keys.iter()
                .all(|key| !key.contains("password") && !key.contains("secret"))
        );
    }

    #[tokio::test]
    async fn metadata_and_checksum_round_trip() {
        let (_dir, target) = target().await;
        let mut pending = target.begin("db.dump").await.expect("início");
        pending
            .copy_from(&mut &b"hello world"[..])
            .await
            .expect("cópia");
        let stored = pending.commit().await.expect("commit");
        let metadata = ArtifactMetadata {
            format: "pg_dump-custom",
            sha256: &stored.sha256,
            size_bytes: stored.size_bytes,
            database: "db",
            host: "localhost",
            port: 5432,
            server_version: "17.11",
            pg_dump_version: "17.11",
            runner: "docker",
            created_at: "2026-10-20T19:00:00Z",
            execution_id: "exec-1",
        };
        write_metadata(&stored.path, &metadata)
            .await
            .expect("metadados");

        let read = read_metadata(&stored.path)
            .await
            .expect("leitura")
            .expect("metadados presentes");
        let (size, sha256) = sha256_file(&stored.path).await.expect("checksum");

        assert_eq!(read.sha256, stored.sha256);
        assert_eq!(read.database, "db");
        assert_eq!((size, sha256), (stored.size_bytes, stored.sha256));
        assert_eq!(
            read_metadata(&target.dir().join("outro.dump"))
                .await
                .expect("leitura"),
            None
        );
    }

    #[test]
    fn file_name_is_sanitized_and_timestamped() {
        let at = datetime!(2026-10-20 19:05:09 UTC);
        assert_eq!(
            backup_file_name("my_database", at),
            "my_database-20261020T190509Z.dump"
        );
        assert_eq!(
            backup_file_name("../etc/passwd db", at),
            "___etc_passwd_db-20261020T190509Z.dump"
        );
    }
}
