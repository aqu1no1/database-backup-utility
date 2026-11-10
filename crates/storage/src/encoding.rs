use std::fmt;
use std::io::{self, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};

use age::secrecy::ExposeSecret;
use age::x25519;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{BUFFER_SIZE, StorageError, io_error, lock_partial, to_hex};

pub const MAX_KEY_FILE_BYTES: u64 = 64 * 1024;
const ZSTD_LEVEL: i32 = 3;
const WORK_DIR_PREFIX: &str = "open-";
const WORK_LOCK_FILE: &str = ".lock";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Compression {
    Zstd,
}

impl Compression {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Zstd => "zstd",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Zstd => "zst",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Encryption {
    Age,
}

impl Encryption {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Age => "age",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Age => "age",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredContent {
    pub file_name: String,
    pub sha256: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactEncoding {
    pub compression: Option<Compression>,
    pub encryption: Option<Encryption>,
    pub content: StoredContent,
}

#[derive(Clone, Default)]
pub struct Recipients(Vec<x25519::Recipient>);

impl fmt::Debug for Recipients {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Recipients").field(&self.0.len()).finish()
    }
}

impl Recipients {
    pub fn add_key(&mut self, key: &str) -> Result<(), StorageError> {
        let recipient = x25519::Recipient::from_str(key.trim())
            .map_err(|_| StorageError::InvalidRecipient(key.trim().to_owned()))?;
        self.0.push(recipient);
        Ok(())
    }

    pub fn add_file(&mut self, path: &Path) -> Result<(), StorageError> {
        let text = read_key_file(path, KeyFile::Recipients)?;
        let before = self.0.len();
        for line in text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            self.add_key(line)?;
        }
        if self.0.len() == before {
            return Err(StorageError::EmptyKeyFile(path.to_owned()));
        }
        Ok(())
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }
}

pub struct Identities(Vec<x25519::Identity>);

impl fmt::Debug for Identities {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Identities").field(&self.0.len()).finish()
    }
}

impl Identities {
    pub fn read_file(path: &Path) -> Result<Self, StorageError> {
        let text = Zeroizing(read_key_file(path, KeyFile::Identity)?);
        let mut identities = Vec::new();
        for line in text.0.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let identity = x25519::Identity::from_str(line)
                .map_err(|_| StorageError::InvalidIdentityFile(path.to_owned()))?;
            identities.push(identity);
        }
        if identities.is_empty() {
            return Err(StorageError::EmptyKeyFile(path.to_owned()));
        }
        Ok(Self(identities))
    }

    #[must_use]
    pub fn public_keys(&self) -> Vec<String> {
        self.0
            .iter()
            .map(|identity| identity.to_public().to_string())
            .collect()
    }
}

struct Zeroizing(String);

impl Drop for Zeroizing {
    fn drop(&mut self) {
        let mut bytes = std::mem::take(&mut self.0).into_bytes();
        bytes.iter_mut().for_each(|byte| *byte = 0);
        std::hint::black_box(&bytes);
    }
}

#[derive(Clone, Copy)]
enum KeyFile {
    Identity,
    Recipients,
}

fn read_key_file(path: &Path, kind: KeyFile) -> Result<String, StorageError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|source| match source.kind() {
        io::ErrorKind::NotFound => StorageError::KeyFileNotFound(path.to_owned()),
        _ => io_error(path)(source),
    })?;
    let unsafe_file = |reason| StorageError::UnsafeKeyFile {
        path: path.to_owned(),
        reason,
    };
    if metadata.file_type().is_symlink() {
        return Err(unsafe_file("link simbólico"));
    }
    if !metadata.is_file() {
        return Err(unsafe_file("não é um arquivo regular"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = metadata.permissions().mode();
        match kind {
            KeyFile::Identity if mode & 0o077 != 0 => {
                return Err(unsafe_file(
                    "acessível por grupo ou outros usuários; use chmod 600",
                ));
            }
            KeyFile::Recipients if mode & 0o022 != 0 => {
                return Err(unsafe_file("gravável por grupo ou outros usuários"));
            }
            _ => {}
        }
    }
    #[cfg(not(unix))]
    let _ = kind;
    if metadata.len() > MAX_KEY_FILE_BYTES {
        return Err(unsafe_file("maior que 64 KiB"));
    }
    let bytes = std::fs::read(path).map_err(io_error(path))?;
    String::from_utf8(bytes).map_err(|error| {
        let mut bytes = error.into_bytes();
        bytes.iter_mut().for_each(|byte| *byte = 0);
        std::hint::black_box(&bytes);
        StorageError::InvalidIdentityFile(path.to_owned())
    })
}

pub fn generate_identity(path: &Path) -> Result<String, StorageError> {
    let identity = x25519::Identity::generate();
    let public = identity.to_public().to_string();
    let created = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|source| match source.kind() {
        io::ErrorKind::AlreadyExists => StorageError::AlreadyExists(path.to_owned()),
        _ => io_error(path)(source),
    })?;
    let secret = identity.to_string();
    let written = (|| {
        writeln!(file, "# created: {created}")?;
        writeln!(file, "# public key: {public}")?;
        writeln!(file, "{}", secret.expose_secret())?;
        file.sync_all()
    })();
    if let Err(source) = written {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(io_error(path)(source));
    }
    crate::sync_dir_blocking(path)?;
    Ok(public)
}

#[derive(Debug, Clone, Default)]
pub struct Encoding {
    compression: Option<Compression>,
    recipients: Recipients,
}

impl Encoding {
    #[must_use]
    pub fn new(compression: Option<Compression>, recipients: Recipients) -> Self {
        Self {
            compression,
            recipients,
        }
    }

    #[must_use]
    pub fn is_plain(&self) -> bool {
        self.compression.is_none() && self.recipients.is_empty()
    }

    #[must_use]
    pub fn compression(&self) -> Option<Compression> {
        self.compression
    }

    #[must_use]
    pub fn encryption(&self) -> Option<Encryption> {
        (!self.recipients.is_empty()).then_some(Encryption::Age)
    }

    #[must_use]
    pub fn file_name(&self, plain: &str) -> String {
        let mut name = plain.to_owned();
        if let Some(compression) = self.compression {
            name.push('.');
            name.push_str(compression.extension());
        }
        if let Some(encryption) = self.encryption() {
            name.push('.');
            name.push_str(encryption.extension());
        }
        name
    }
}

struct HashingWriter<W> {
    inner: W,
    hasher: Sha256,
    size: u64,
}

impl<W: Write> HashingWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
            size: 0,
        }
    }

    fn finish(self) -> (W, u64, String) {
        (self.inner, self.size, to_hex(&self.hasher.finalize()))
    }
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(buf)?;
        self.hasher.update(&buf[..written]);
        self.size += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

enum CopyError {
    Read(io::Error),
    Write(io::Error),
    Canceled,
    TooLarge,
}

fn copy_stream<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    cancel: &AtomicBool,
    limit: Option<u64>,
) -> Result<u64, CopyError> {
    let mut buffer = vec![0_u8; BUFFER_SIZE];
    let mut total = 0_u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(CopyError::Canceled);
        }
        let read = match reader.read(&mut buffer) {
            Ok(0) => return Ok(total),
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(CopyError::Read(error)),
        };
        total += read as u64;
        if limit.is_some_and(|limit| total > limit) {
            return Err(CopyError::TooLarge);
        }
        writer
            .write_all(&buffer[..read])
            .map_err(CopyError::Write)?;
    }
}

pub(crate) fn encode_file(
    plain: &Path,
    file: std::fs::File,
    output: &Path,
    encoding: &Encoding,
    cancel: &AtomicBool,
) -> Result<(u64, String), StorageError> {
    let mut reader = BufReader::new(std::fs::File::open(plain).map_err(io_error(plain))?);
    let sink = HashingWriter::new(file);
    let write_error = |error: io::Error| io_error(output)(error);
    let copy_error = |error: CopyError| match error {
        CopyError::Read(source) => io_error(plain)(source),
        CopyError::Write(source) => io_error(output)(source),
        CopyError::Canceled => StorageError::Canceled,
        CopyError::TooLarge => io_error(output)(io::Error::other("limite de tamanho excedido")),
    };
    let sink = if encoding.recipients.is_empty() {
        compress_into(&mut reader, sink, encoding.compression, cancel).map_err(copy_error)?
    } else {
        let encryptor = age::Encryptor::with_recipients(
            encoding
                .recipients
                .0
                .iter()
                .map(|recipient| recipient as &dyn age::Recipient),
        )
        .map_err(|_| StorageError::Encryption)?;
        let encrypted = encryptor.wrap_output(sink).map_err(write_error)?;
        let encrypted = compress_into(&mut reader, encrypted, encoding.compression, cancel)
            .map_err(copy_error)?;
        encrypted.finish().map_err(write_error)?
    };
    let (mut file, size, sha256) = sink.finish();
    file.flush().map_err(write_error)?;
    file.sync_all().map_err(write_error)?;
    Ok((size, sha256))
}

fn compress_into<R: Read, W: Write>(
    reader: &mut R,
    mut writer: W,
    compression: Option<Compression>,
    cancel: &AtomicBool,
) -> Result<W, CopyError> {
    match compression {
        Some(Compression::Zstd) => {
            let mut encoder =
                zstd::stream::Encoder::new(writer, ZSTD_LEVEL).map_err(CopyError::Write)?;
            copy_stream(reader, &mut encoder, cancel, None)?;
            encoder.finish().map_err(CopyError::Write)
        }
        None => {
            copy_stream(reader, &mut writer, cancel, None)?;
            Ok(writer)
        }
    }
}

pub(crate) fn decode_file(
    source: &Path,
    output: &Path,
    encoding: &ArtifactEncoding,
    identities: Option<&Identities>,
    cancel: &AtomicBool,
) -> Result<(u64, String), StorageError> {
    let reader = BufReader::new(std::fs::File::open(source).map_err(io_error(source))?);
    let file = open_new_private(output)?;
    let mut sink = HashingWriter::new(file);
    let limit = Some(encoding.content.size_bytes);
    let copy_error = |error: CopyError| match error {
        CopyError::Read(_) => StorageError::Unreadable,
        CopyError::Write(error) => io_error(output)(error),
        CopyError::Canceled => StorageError::Canceled,
        CopyError::TooLarge => {
            StorageError::ContentMismatch("o conteúdo aberto é maior que o registrado no manifesto")
        }
    };
    match encoding.encryption {
        Some(Encryption::Age) => {
            let identities = identities.ok_or(StorageError::MissingIdentity)?;
            let decryptor = age::Decryptor::new_buffered(reader).map_err(|error| match error {
                age::DecryptError::Io(error) => io_error(source)(error),
                _ => StorageError::Unreadable,
            })?;
            let mut decrypted = decryptor
                .decrypt(
                    identities
                        .0
                        .iter()
                        .map(|identity| identity as &dyn age::Identity),
                )
                .map_err(|error| match error {
                    age::DecryptError::NoMatchingKeys => StorageError::WrongIdentity,
                    _ => StorageError::Unreadable,
                })?;
            decompress_from(
                &mut decrypted,
                &mut sink,
                encoding.compression,
                cancel,
                limit,
            )
            .map_err(copy_error)?;
        }
        None => {
            let mut reader = reader;
            decompress_from(&mut reader, &mut sink, encoding.compression, cancel, limit)
                .map_err(copy_error)?;
        }
    }
    let (mut file, size, sha256) = sink.finish();
    file.flush().map_err(io_error(output))?;
    file.sync_all().map_err(io_error(output))?;
    Ok((size, sha256))
}

fn decompress_from<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    compression: Option<Compression>,
    cancel: &AtomicBool,
    limit: Option<u64>,
) -> Result<u64, CopyError> {
    match compression {
        Some(Compression::Zstd) => {
            let mut decoder = zstd::stream::Decoder::new(reader).map_err(CopyError::Read)?;
            copy_stream(&mut decoder, writer, cancel, limit)
        }
        None => copy_stream(reader, writer, cancel, limit),
    }
}

pub(crate) fn open_new_private(path: &Path) -> Result<std::fs::File, StorageError> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).map_err(|source| match source.kind() {
        io::ErrorKind::AlreadyExists => StorageError::AlreadyExists(path.to_owned()),
        _ => io_error(path)(source),
    })
}

pub struct WorkDir {
    path: PathBuf,
    lock: Option<std::fs::File>,
}

impl WorkDir {
    pub fn create(base: &Path) -> Result<Self, StorageError> {
        create_private_dir(base)?;
        remove_orphan_work_dirs(base);
        for attempt in 0..16_u32 {
            let name = format!(
                "{WORK_DIR_PREFIX}{}-{}-{attempt}",
                std::process::id(),
                time::OffsetDateTime::now_utc().unix_timestamp_nanos()
            );
            let path = base.join(name);
            match create_dir_private(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(source) => return Err(io_error(&path)(source)),
            }
            let lock_path = path.join(WORK_LOCK_FILE);
            if let Err(error) = open_new_private(&lock_path) {
                let _ = std::fs::remove_dir_all(&path);
                return Err(error);
            }
            let lock = match lock_partial(&lock_path) {
                Ok(lock) => lock,
                Err(error) => {
                    let _ = std::fs::remove_dir_all(&path);
                    return Err(error);
                }
            };
            return Ok(Self {
                path,
                lock: Some(lock),
            });
        }
        Err(StorageError::AlreadyExists(base.to_owned()))
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
        drop(self.lock.take());
    }
}

fn create_private_dir(path: &Path) -> Result<(), StorageError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(StorageError::UnsafeSource {
            path: path.to_owned(),
            reason: "não é um diretório",
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            std::fs::create_dir_all(path).map_err(io_error(path))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
                    .map_err(io_error(path))?;
            }
            Ok(())
        }
        Err(source) => Err(io_error(path)(source)),
    }
}

fn create_dir_private(path: &Path) -> io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

fn older_than_orphan_age(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age >= crate::ORPHAN_MIN_AGE)
}

fn remove_orphan_work_dirs(base: &Path) {
    let Ok(entries) = std::fs::read_dir(base) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with(WORK_DIR_PREFIX)
        {
            continue;
        }
        let path = entry.path();
        if !std::fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_dir()) {
            continue;
        }
        let orphan = match std::fs::File::open(path.join(WORK_LOCK_FILE)) {
            Ok(file) => file.try_lock().is_ok(),
            Err(error) => error.kind() == io::ErrorKind::NotFound && older_than_orphan_age(&path),
        };
        if orphan {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}
