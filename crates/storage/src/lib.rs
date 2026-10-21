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
pub const MANIFEST_VERSION: u32 = 1;

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
    #[error("arquivo de backup não encontrado: {0}")]
    SourceNotFound(PathBuf),
    #[error("arquivo recusado ({reason}): {path}")]
    UnsafeSource { path: PathBuf, reason: &'static str },
    #[error("{}", unsupported_manifest(*.found))]
    UnsupportedManifest { found: Option<u32> },
}

fn unsupported_manifest(found: Option<u32>) -> String {
    match found {
        None => format!(
            "o manifesto não informa manifest_version (esperado {MANIFEST_VERSION}); gere o backup de novo ou use --allow-unverified"
        ),
        Some(version) => format!(
            "versão {version} do manifesto não é suportada por esta versão do utilitário (suporta {MANIFEST_VERSION})"
        ),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredArtifact {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone)]
pub struct ArtifactMetadata<'a> {
    pub tool_name: &'a str,
    pub tool_version: &'a str,
    pub engine: &'a str,
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

#[derive(Serialize)]
struct ManifestTool<'a> {
    name: &'a str,
    version: &'a str,
}

#[derive(Serialize)]
struct Manifest<'a> {
    manifest_version: u32,
    tool: ManifestTool<'a>,
    engine: &'a str,
    format: &'a str,
    file_name: &'a str,
    sha256: &'a str,
    size_bytes: u64,
    database: &'a str,
    host: &'a str,
    port: u16,
    server_version: &'a str,
    pg_dump_version: &'a str,
    runner: &'a str,
    created_at: &'a str,
    execution_id: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct StoredTool {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct StoredMetadata {
    pub manifest_version: u32,
    pub tool: StoredTool,
    pub engine: String,
    pub format: String,
    pub file_name: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub database: String,
    pub server_version: String,
    pub pg_dump_version: String,
    pub created_at: String,
}

#[derive(Deserialize)]
struct ManifestVersion {
    manifest_version: Option<u32>,
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

    pub async fn is_shared(&self) -> Result<bool, StorageError> {
        let metadata = fs::metadata(&self.dir).await.map_err(io_error(&self.dir))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            Ok(metadata.permissions().mode() & 0o077 != 0)
        }
        #[cfg(not(unix))]
        {
            let _ = metadata;
            Ok(false)
        }
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
    let file_name = artifact
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let manifest = Manifest {
        manifest_version: MANIFEST_VERSION,
        tool: ManifestTool {
            name: metadata.tool_name,
            version: metadata.tool_version,
        },
        engine: metadata.engine,
        format: metadata.format,
        file_name: &file_name,
        sha256: metadata.sha256,
        size_bytes: metadata.size_bytes,
        database: metadata.database,
        host: metadata.host,
        port: metadata.port,
        server_version: metadata.server_version,
        pg_dump_version: metadata.pg_dump_version,
        runner: metadata.runner,
        created_at: metadata.created_at,
        execution_id: metadata.execution_id,
    };
    let mut json = serde_json::to_vec_pretty(&manifest).map_err(StorageError::Metadata)?;
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
        Ok(bytes) => parse_manifest(&bytes).map(Some),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(StorageError::Io { path, source }),
    }
}

/// Recusa origens de restore que poderiam ter sido trocadas por outro usuário:
/// symlinks, arquivos não regulares e, no unix, arquivos graváveis por grupo ou
/// outros. Aplica a mesma regra ao `.meta.json`, que fornece o checksum esperado.
pub async fn check_restore_source(artifact: &Path) -> Result<(), StorageError> {
    if !check_source_file(artifact).await? {
        return Err(StorageError::SourceNotFound(artifact.to_owned()));
    }
    check_source_file(&metadata_path(artifact)).await?;
    Ok(())
}

/// Retorna `false` quando o arquivo não existe.
async fn check_source_file(path: &Path) -> Result<bool, StorageError> {
    let metadata = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(source) => {
            return Err(StorageError::Io {
                path: path.to_owned(),
                source,
            });
        }
    };
    let unsafe_source = |reason| StorageError::UnsafeSource {
        path: path.to_owned(),
        reason,
    };
    if metadata.file_type().is_symlink() {
        return Err(unsafe_source("link simbólico"));
    }
    if !metadata.is_file() {
        return Err(unsafe_source("não é um arquivo regular"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o022 != 0 {
            return Err(unsafe_source("gravável por grupo ou outros usuários"));
        }
    }
    Ok(true)
}

fn parse_manifest(bytes: &[u8]) -> Result<StoredMetadata, StorageError> {
    let probe: ManifestVersion = serde_json::from_slice(bytes).map_err(StorageError::Metadata)?;
    if probe.manifest_version != Some(MANIFEST_VERSION) {
        return Err(StorageError::UnsupportedManifest {
            found: probe.manifest_version,
        });
    }
    serde_json::from_slice(bytes).map_err(StorageError::Metadata)
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

    #[cfg(unix)]
    #[tokio::test]
    async fn reports_output_directory_shared_with_others() {
        use std::os::unix::fs::PermissionsExt;

        let (_dir, target) = target().await;
        assert!(!target.is_shared().await.expect("modo"));

        std::fs::set_permissions(target.dir(), std::fs::Permissions::from_mode(0o755))
            .expect("modo");
        let reopened = LocalTarget::new(target.dir()).await.expect("destino");

        assert!(reopened.is_shared().await.expect("modo"));
        assert_eq!(
            std::fs::metadata(reopened.dir())
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
    }

    #[tokio::test]
    async fn metadata_is_written_next_to_artifact() {
        let (_dir, target) = target().await;
        let artifact = target.dir().join("db.dump");
        let metadata = ArtifactMetadata {
            tool_name: "database-backup",
            tool_version: "0.1.0",
            engine: "postgresql",
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
        assert_eq!(json["manifest_version"], MANIFEST_VERSION);
        assert_eq!(json["tool"]["name"], "database-backup");
        assert_eq!(json["tool"]["version"], "0.1.0");
        assert_eq!(json["engine"], "postgresql");
        assert_eq!(json["file_name"], "db.dump");
        assert_eq!(json["sha256"], "abc");
        assert_eq!(json["runner"], "docker");
        let allowed = [
            "manifest_version",
            "tool",
            "engine",
            "format",
            "file_name",
            "sha256",
            "size_bytes",
            "database",
            "host",
            "port",
            "server_version",
            "pg_dump_version",
            "runner",
            "created_at",
            "execution_id",
        ];
        let keys: Vec<&String> = json.as_object().expect("objeto").keys().collect();
        assert!(
            keys.iter().all(|key| allowed.contains(&key.as_str())),
            "{keys:?}"
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
            tool_name: "database-backup",
            tool_version: "0.1.0",
            engine: "postgresql",
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

        assert_eq!(read.manifest_version, MANIFEST_VERSION);
        assert_eq!(read.engine, "postgresql");
        assert_eq!(read.file_name, "db.dump");
        assert_eq!(read.tool.version, "0.1.0");
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

    #[tokio::test]
    async fn manifest_without_or_with_unknown_version_is_rejected() {
        let (_dir, target) = target().await;
        let artifact = target.dir().join("db.dump");
        let cases = [
            (r#"{"sha256":"abc"}"#, None),
            (r#"{"manifest_version":2,"sha256":"abc"}"#, Some(2)),
        ];

        for (body, expected) in cases {
            std::fs::write(metadata_path(&artifact), body).expect("manifesto");
            let result = read_metadata(&artifact).await;
            assert!(
                matches!(result, Err(StorageError::UnsupportedManifest { found }) if found == expected),
                "{body}"
            );
        }
        std::fs::write(metadata_path(&artifact), "{não é json").expect("manifesto");
        assert!(matches!(
            read_metadata(&artifact).await,
            Err(StorageError::Metadata(_))
        ));
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

    #[cfg(unix)]
    fn source_file(dir: &Path, name: &str, mode: u32) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let path = dir.join(name);
        std::fs::write(&path, b"dump").expect("arquivo");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).expect("modo");
        path
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn restore_source_accepts_private_file() {
        let dir = TempDir::new().expect("diretório temporário");
        let dump = source_file(dir.path(), "db.dump", 0o600);
        source_file(dir.path(), "db.dump.meta.json", 0o644);

        check_restore_source(&dump).await.expect("origem válida");
    }

    #[tokio::test]
    async fn restore_source_reports_missing_file() {
        let dir = TempDir::new().expect("diretório temporário");

        let result = check_restore_source(&dir.path().join("ausente.dump")).await;

        assert!(matches!(result, Err(StorageError::SourceNotFound(_))));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn restore_source_rejects_symlink_and_directory() {
        let dir = TempDir::new().expect("diretório temporário");
        let dump = source_file(dir.path(), "db.dump", 0o600);
        let link = dir.path().join("link.dump");
        std::os::unix::fs::symlink(&dump, &link).expect("symlink");

        let from_link = check_restore_source(&link).await;
        let from_dir = check_restore_source(dir.path()).await;

        assert!(matches!(from_link, Err(StorageError::UnsafeSource { .. })));
        assert!(matches!(from_dir, Err(StorageError::UnsafeSource { .. })));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn restore_source_rejects_writable_by_others() {
        let dir = TempDir::new().expect("diretório temporário");
        let shared = source_file(dir.path(), "shared.dump", 0o666);
        let group = source_file(dir.path(), "group.dump", 0o620);
        let with_shared_meta = source_file(dir.path(), "db.dump", 0o600);
        source_file(dir.path(), "db.dump.meta.json", 0o666);

        for path in [shared, group, with_shared_meta] {
            let result = check_restore_source(&path).await;
            assert!(
                matches!(result, Err(StorageError::UnsafeSource { .. })),
                "{} deveria ser recusado",
                path.display()
            );
        }
    }
}
