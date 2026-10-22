#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use database_backup_adapters::sqlite::{
    DEFAULT_BUSY_LIMIT, SqliteError, check_database_file, check_not_in_use, copy_database,
    inspect_backup,
};
use rusqlite::Connection;
use tempfile::TempDir;

const BATCH: i64 = 10;
const COPY_DEADLINE: Duration = Duration::from_secs(30);

fn wal_source(dir: &Path, filler_rows: i64) -> PathBuf {
    let path = dir.join("app.db");
    let conn = Connection::open(&path).unwrap();
    conn.pragma_update(None, "journal_mode", "WAL").unwrap();
    conn.execute_batch(
        "CREATE TABLE filler (id INTEGER PRIMARY KEY, data BLOB NOT NULL);
         CREATE TABLE events (batch INTEGER NOT NULL, seq INTEGER NOT NULL, PRIMARY KEY (batch, seq));",
    )
    .unwrap();
    conn.execute(
        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < ?1)
         INSERT INTO filler (data) SELECT randomblob(4000) FROM n",
        [filler_rows],
    )
    .unwrap();
    path
}

fn empty_partial(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::File::create(&path).unwrap();
    path
}

fn copy(source: &Path, destination: &Path) -> Result<(), SqliteError> {
    copy_database(
        source,
        destination,
        &AtomicBool::new(false),
        DEFAULT_BUSY_LIMIT,
    )
}

fn side_files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| {
            !name.starts_with("app.db")
                && (name.ends_with("-journal") || name.ends_with("-wal") || name.ends_with("-shm"))
        })
        .collect();
    names.sort();
    names
}

#[test]
fn copy_of_wal_database_under_concurrent_writes_is_a_consistent_snapshot() {
    let dir = TempDir::new().unwrap();
    let source = wal_source(dir.path(), 4000);
    let partial = empty_partial(dir.path(), "backup.sqlite3.partial");
    let stop = Arc::new(AtomicBool::new(false));
    let committed = Arc::new(AtomicU64::new(0));

    let writer = {
        let (source, stop, committed) = (source.clone(), stop.clone(), committed.clone());
        thread::spawn(move || {
            let mut conn = Connection::open(&source).unwrap();
            conn.busy_timeout(Duration::from_secs(5)).unwrap();
            let mut batch = 0_i64;
            while !stop.load(Ordering::Relaxed) {
                let tx = conn.transaction().unwrap();
                for seq in 0..BATCH {
                    tx.execute(
                        "INSERT INTO events (batch, seq) VALUES (?1, ?2)",
                        [batch, seq],
                    )
                    .unwrap();
                }
                tx.commit().unwrap();
                committed.fetch_add(1, Ordering::Relaxed);
                batch += 1;
            }
        })
    };
    while committed.load(Ordering::Relaxed) == 0 {
        thread::sleep(Duration::from_millis(5));
    }

    let before = committed.load(Ordering::Relaxed);
    let (done, finished) = mpsc::channel();
    {
        let (source, partial) = (source.clone(), partial.clone());
        thread::spawn(move || done.send(copy(&source, &partial)));
    }
    finished
        .recv_timeout(COPY_DEADLINE)
        .expect("o backup não terminou com um escritor contínuo (livelock)")
        .unwrap();
    let during = committed.load(Ordering::Relaxed) - before;
    stop.store(true, Ordering::Relaxed);
    writer.join().unwrap();

    assert!(during > 0, "o escritor não gravou durante o backup");
    let info = inspect_backup(&partial).unwrap();
    assert_eq!(info.tables, 2);
    let copied = Connection::open(&partial).unwrap();
    let incomplete: i64 = copied
        .query_row(
            "SELECT count(*) FROM (SELECT batch FROM events GROUP BY batch HAVING count(*) <> ?1)",
            [BATCH],
            |row| row.get(0),
        )
        .unwrap();
    let filler: i64 = copied
        .query_row("SELECT count(*) FROM filler", [], |row| row.get(0))
        .unwrap();
    assert_eq!(incomplete, 0, "a cópia tem lote gravado pela metade");
    assert_eq!(filler, 4000);
    assert_eq!(side_files(dir.path()), Vec::<String>::new());
}

#[test]
fn backup_restored_into_new_file_has_the_same_rows() {
    let dir = TempDir::new().unwrap();
    let source = wal_source(dir.path(), 3);
    Connection::open(&source)
        .unwrap()
        .execute_batch("INSERT INTO events VALUES (1, 1), (1, 2), (2, 1);")
        .unwrap();
    let artifact = empty_partial(dir.path(), "backup.sqlite3");
    copy(&source, &artifact).unwrap();
    let restored = empty_partial(dir.path(), "restored.db.partial");

    copy(&artifact, &restored).unwrap();

    assert_eq!(inspect_backup(&restored).unwrap().tables, 2);
    let rows: Vec<(i64, i64)> = Connection::open(&restored)
        .unwrap()
        .prepare("SELECT batch, seq FROM events ORDER BY batch, seq")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(rows, vec![(1, 1), (1, 2), (2, 1)]);
    assert_eq!(side_files(dir.path()), Vec::<String>::new());
}

#[cfg(unix)]
#[test]
fn unsafe_or_foreign_sources_are_rejected_before_any_copy() {
    let dir = TempDir::new().unwrap();
    let source = wal_source(dir.path(), 1);
    let link = dir.path().join("link.db");
    std::os::unix::fs::symlink(&source, &link).unwrap();
    let text = dir.path().join("notes.txt");
    std::fs::write(&text, "não sou um banco SQLite, só texto comum").unwrap();
    let empty = empty_partial(dir.path(), "empty.db");

    assert!(matches!(
        check_database_file(&link),
        Err(SqliteError::Unsafe { .. })
    ));
    assert!(matches!(
        check_database_file(dir.path()),
        Err(SqliteError::Unsafe { .. })
    ));
    assert!(matches!(
        check_database_file(&text),
        Err(SqliteError::NotSqlite(_))
    ));
    assert!(matches!(
        check_database_file(&empty),
        Err(SqliteError::NotSqlite(_))
    ));
    assert!(matches!(
        check_database_file(&dir.path().join("ausente.db")),
        Err(SqliteError::NotFound(_))
    ));

    let partial = empty_partial(dir.path(), "out.partial");
    assert!(matches!(
        copy(&link, &partial),
        Err(SqliteError::Unsafe { .. })
    ));
    assert_eq!(std::fs::metadata(&partial).unwrap().len(), 0);
}

#[test]
fn copy_never_creates_the_destination_file() {
    let dir = TempDir::new().unwrap();
    let source = wal_source(dir.path(), 1);
    let missing = dir.path().join("nao-criado.partial");

    let result = copy(&source, &missing);

    assert!(matches!(result, Err(SqliteError::Sqlite { .. })));
    assert!(!missing.exists());
}

#[test]
fn canceled_copy_stops_with_canceled() {
    let dir = TempDir::new().unwrap();
    let source = wal_source(dir.path(), 10);
    let partial = empty_partial(dir.path(), "out.partial");

    let result = copy_database(
        &source,
        &partial,
        &AtomicBool::new(true),
        DEFAULT_BUSY_LIMIT,
    );

    assert!(matches!(result, Err(SqliteError::Canceled)));
}

#[test]
fn inspection_rejects_wal_mode_and_truncated_files() {
    let dir = TempDir::new().unwrap();
    let source = wal_source(dir.path(), 200);
    let artifact = empty_partial(dir.path(), "backup.sqlite3");
    copy(&source, &artifact).unwrap();
    let bytes = std::fs::read(&artifact).unwrap();
    let truncated = dir.path().join("truncated.sqlite3");
    std::fs::write(&truncated, &bytes[..bytes.len() / 2]).unwrap();

    assert!(matches!(
        inspect_backup(&source),
        Err(SqliteError::WalArtifact(_))
    ));
    assert!(inspect_backup(&truncated).is_err());
    assert_eq!(inspect_backup(&artifact).unwrap().tables, 2);
}

fn rollback_db(dir: &Path) -> PathBuf {
    let path = dir.join("destino.db");
    Connection::open(&path)
        .unwrap()
        .execute_batch("CREATE TABLE t (x INTEGER); INSERT INTO t VALUES (1);")
        .unwrap();
    path
}

#[test]
fn database_with_open_transaction_in_another_connection_is_in_use() {
    let dir = TempDir::new().unwrap();
    let target = rollback_db(dir.path());
    let writer = Connection::open(&target).unwrap();
    writer
        .execute_batch("BEGIN IMMEDIATE; INSERT INTO t VALUES (2);")
        .unwrap();

    let busy = check_not_in_use(&target);
    writer.execute_batch("ROLLBACK;").unwrap();
    drop(writer);
    let idle = check_not_in_use(&target);

    assert!(matches!(busy, Err(SqliteError::InUse { .. })), "{busy:?}");
    idle.unwrap();
    assert_eq!(side_files(dir.path()), Vec::<String>::new());
}

#[cfg(target_os = "linux")]
#[test]
fn database_held_open_by_another_process_is_in_use() {
    let dir = TempDir::new().unwrap();
    let target = rollback_db(dir.path());
    let holder = FileHolder::spawn(&target);
    let pid = holder.pid();

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let detected = loop {
        match check_not_in_use(&target) {
            Err(SqliteError::InUse { pids, .. }) => break pids,
            _ if std::time::Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            other => panic!("o processo {pid} com o arquivo aberto não foi detectado: {other:?}"),
        }
    };
    drop(holder);

    assert!(detected.contains(&pid), "{detected:?} não contém {pid}");
    check_not_in_use(&target).unwrap();
}

#[cfg(target_os = "linux")]
struct FileHolder(std::process::Child);

#[cfg(target_os = "linux")]
impl FileHolder {
    fn spawn(path: &Path) -> Self {
        Self(
            std::process::Command::new("tail")
                .arg("-f")
                .arg(path)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap(),
        )
    }

    fn pid(&self) -> u32 {
        self.0.id()
    }
}

#[cfg(target_os = "linux")]
impl Drop for FileHolder {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
