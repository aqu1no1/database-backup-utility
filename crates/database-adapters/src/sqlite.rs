use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use rusqlite::backup::{Backup, StepResult};
use rusqlite::{Connection, ErrorCode, OpenFlags};
use thiserror::Error;

const MAGIC: &[u8] = b"SQLite format 3\0";
const HEADER_LEN: usize = 20;
const WRITE_VERSION: usize = 18;
const READ_VERSION: usize = 19;
const ROLLBACK_JOURNAL: u8 = 1;
const PAGES_PER_STEP: i32 = 256;
const ALL_PAGES: i32 = -1;
const BUSY_PAUSE: Duration = Duration::from_millis(50);
pub const DEFAULT_BUSY_LIMIT: Duration = Duration::from_secs(30);

#[derive(Debug, Error)]
pub enum SqliteError {
    #[error("arquivo SQLite não encontrado: {0}")]
    NotFound(PathBuf),
    #[error("arquivo recusado ({reason}): {path}")]
    Unsafe { path: PathBuf, reason: &'static str },
    #[error("{0} não é um banco SQLite")]
    NotSqlite(PathBuf),
    #[error("{0} está em modo WAL e não é um backup gerado por este utilitário")]
    WalArtifact(PathBuf),
    #[error("o banco {0} continuou bloqueado por outro processo além do tempo de espera")]
    Busy(PathBuf),
    #[error("operação cancelada")]
    Canceled,
    #[error("{}", in_use(path, pids))]
    InUse { path: PathBuf, pids: Vec<u32> },
    #[error("o banco {path} não passou na verificação de integridade: {detail}")]
    Integrity { path: PathBuf, detail: String },
    #[error("falha de E/S em {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("falha do SQLite em {path}")]
    Sqlite {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DatabaseInfo {
    pub objects: usize,
    pub tables: usize,
}

#[must_use]
pub fn library_version() -> &'static str {
    rusqlite::version()
}

pub fn check_database_file(path: &Path) -> Result<(), SqliteError> {
    read_header(path).map(|_| ())
}

pub fn copy_database(
    source: &Path,
    destination: &Path,
    cancel: &AtomicBool,
    busy_limit: Duration,
) -> Result<(), SqliteError> {
    check_database_file(source)?;
    let from = open(source, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut to = open(destination, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    to.pragma_update(None, "journal_mode", "OFF")
        .map_err(sqlite_error(destination))?;
    to.pragma_update(None, "synchronous", "OFF")
        .map_err(sqlite_error(destination))?;
    run_backup(&from, &mut to, source, cancel, busy_limit)?;
    to.pragma_update(None, "journal_mode", "DELETE")
        .map_err(sqlite_error(destination))?;
    to.close()
        .map_err(|(_, source)| sqlite_error(destination)(source))
}

pub fn inspect_backup(path: &Path) -> Result<DatabaseInfo, SqliteError> {
    let header = read_header(path)?;
    if header[WRITE_VERSION] != ROLLBACK_JOURNAL || header[READ_VERSION] != ROLLBACK_JOURNAL {
        return Err(SqliteError::WalArtifact(path.to_owned()));
    }
    let conn = open(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let problems = integrity_problems(&conn).map_err(sqlite_error(path))?;
    if problems != ["ok"] {
        return Err(SqliteError::Integrity {
            path: path.to_owned(),
            detail: problems.join("; "),
        });
    }
    let (objects, tables): (i64, i64) = conn
        .query_row(
            "SELECT count(*), coalesce(sum(type = 'table'), 0)
             FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(sqlite_error(path))?;
    Ok(DatabaseInfo {
        objects: usize::try_from(objects).unwrap_or(0),
        tables: usize::try_from(tables).unwrap_or(0),
    })
}

pub fn check_not_in_use(path: &Path) -> Result<(), SqliteError> {
    let pids = processes_with_file_open(path);
    if !pids.is_empty() {
        return Err(SqliteError::InUse {
            path: path.to_owned(),
            pids,
        });
    }
    let conn = open(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    conn.busy_timeout(Duration::ZERO)
        .map_err(sqlite_error(path))?;
    match conn.execute_batch("BEGIN EXCLUSIVE; ROLLBACK;") {
        Ok(()) => {}
        Err(rusqlite::Error::SqliteFailure(failure, _))
            if matches!(
                failure.code,
                ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked
            ) =>
        {
            return Err(SqliteError::InUse {
                path: path.to_owned(),
                pids: Vec::new(),
            });
        }
        Err(source) => return Err(sqlite_error(path)(source)),
    }
    conn.close()
        .map_err(|(_, source)| sqlite_error(path)(source))
}

fn in_use(path: &Path, pids: &[u32]) -> String {
    if pids.is_empty() {
        return format!(
            "o banco {} está com uma transação aberta por outro processo; feche-o e tente de novo",
            path.display()
        );
    }
    let list: Vec<String> = pids.iter().map(u32::to_string).collect();
    format!(
        "o banco {} está aberto pelo(s) processo(s) {}; feche-o e tente de novo",
        path.display(),
        list.join(", ")
    )
}

#[cfg(target_os = "linux")]
fn processes_with_file_open(path: &Path) -> Vec<u32> {
    use std::os::unix::fs::MetadataExt;

    let Ok(target) = std::fs::metadata(path) else {
        return Vec::new();
    };
    let Ok(processes) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let own = std::process::id();
    let mut pids: Vec<u32> = processes
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        .filter(|pid| *pid != own)
        .filter(|pid| {
            std::fs::read_dir(format!("/proc/{pid}/fd")).is_ok_and(|fds| {
                fds.flatten().any(|fd| {
                    std::fs::metadata(fd.path()).is_ok_and(|opened| {
                        opened.dev() == target.dev() && opened.ino() == target.ino()
                    })
                })
            })
        })
        .collect();
    pids.sort_unstable();
    pids
}

#[cfg(not(target_os = "linux"))]
fn processes_with_file_open(_path: &Path) -> Vec<u32> {
    Vec::new()
}

fn read_header(path: &Path) -> Result<[u8; HEADER_LEN], SqliteError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(SqliteError::NotFound(path.to_owned()));
        }
        Err(source) => return Err(io_error(path)(source)),
    };
    let unsafe_file = |reason| SqliteError::Unsafe {
        path: path.to_owned(),
        reason,
    };
    if metadata.file_type().is_symlink() {
        return Err(unsafe_file("link simbólico"));
    }
    if !metadata.is_file() {
        return Err(unsafe_file("não é um arquivo regular"));
    }
    let mut header = [0_u8; HEADER_LEN];
    let mut file = File::open(path).map_err(io_error(path))?;
    match file.read_exact(&mut header) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
            return Err(SqliteError::NotSqlite(path.to_owned()));
        }
        Err(source) => return Err(io_error(path)(source)),
    }
    if !header.starts_with(MAGIC) {
        return Err(SqliteError::NotSqlite(path.to_owned()));
    }
    Ok(header)
}

fn open(path: &Path, mode: OpenFlags) -> Result<Connection, SqliteError> {
    Connection::open_with_flags(path, mode | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(sqlite_error(path))
}

fn run_backup(
    from: &Connection,
    to: &mut Connection,
    source: &Path,
    cancel: &AtomicBool,
    busy_limit: Duration,
) -> Result<(), SqliteError> {
    let backup = Backup::new(from, to).map_err(sqlite_error(source))?;
    let mut pages = PAGES_PER_STEP;
    let mut previous_remaining: Option<i32> = None;
    let mut blocked_since: Option<Instant> = None;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(SqliteError::Canceled);
        }
        match backup.step(pages).map_err(sqlite_error(source))? {
            StepResult::Done => return Ok(()),
            StepResult::More => {
                blocked_since = None;
                let remaining = backup.progress().remaining;
                if previous_remaining.is_some_and(|before| remaining > before) {
                    pages = ALL_PAGES;
                }
                previous_remaining = Some(remaining);
            }
            _ => {
                let since = *blocked_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= busy_limit {
                    return Err(SqliteError::Busy(source.to_owned()));
                }
                thread::sleep(BUSY_PAUSE);
            }
        }
    }
}

fn integrity_problems(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut statement = conn.prepare("PRAGMA integrity_check(5)")?;
    let rows = statement.query_map([], |row| row.get(0))?;
    rows.collect()
}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> SqliteError + '_ {
    move |source| SqliteError::Io {
        path: path.to_owned(),
        source,
    }
}

fn sqlite_error(path: &Path) -> impl FnOnce(rusqlite::Error) -> SqliteError + '_ {
    move |source| SqliteError::Sqlite {
        path: path.to_owned(),
        source,
    }
}
