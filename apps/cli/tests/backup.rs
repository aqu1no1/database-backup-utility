#[path = "../../../crates/database-adapters/tests/support/mod.rs"]
mod support;

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use sha2::{Digest, Sha256};
use support::{PostgresTestDb, TestResult};
use tempfile::TempDir;

struct Workspace {
    _dir: TempDir,
    data: PathBuf,
    out: PathBuf,
}

impl Workspace {
    fn new() -> TestResult<Self> {
        let dir = TempDir::new()?;
        let data = dir.path().join("data");
        let out = dir.path().join("out");
        Ok(Self {
            _dir: dir,
            data,
            out,
        })
    }

    fn command(&self, db: &PostgresTestDb, user: &str, password: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_database-backup"));
        command
            .args(["backup", "--runner", "docker", "--host", db.host()])
            .args(["--port", &db.port().to_string()])
            .args(["--database", db.database(), "--user", user])
            .arg("--output-dir")
            .arg(&self.out)
            .env("DBU_DATA_DIR", &self.data)
            .env("DBU_PG_PASSWORD", password)
            .stdin(Stdio::null());
        command
    }

    fn files(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.out)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    fn history(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.data.join("history.sqlite3")).expect("histórico")
    }
}

fn sha256_hex(path: &Path) -> String {
    let digest = Sha256::digest(std::fs::read(path).expect("dump"));
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[tokio::test]
async fn backup_produces_verified_artifact_and_history() -> TestResult {
    let db = PostgresTestDb::start_seeded().await?;
    let ws = Workspace::new()?;

    let output = ws.command(&db, db.user(), db.password()).output()?;

    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    assert!(!text(&output).contains(db.password()));
    let files = ws.files();
    assert_eq!(files.len(), 2, "{files:?}");
    let dump = ws.out.join(&files[0]);
    let meta = ws.out.join(&files[1]);
    assert!(files[0].ends_with(".dump") && files[1].ends_with(".dump.meta.json"));

    let bytes = std::fs::read(&dump)?;
    assert!(bytes.starts_with(b"PGDMP"));
    let checksum = sha256_hex(&dump);
    let metadata: serde_json::Value = serde_json::from_slice(&std::fs::read(&meta)?)?;
    assert_eq!(metadata["sha256"], checksum.as_str());
    assert_eq!(metadata["size_bytes"], bytes.len());
    assert_eq!(metadata["runner"], "docker");

    let history = ws.history();
    let (status, duration, error): (String, Option<i64>, Option<String>) = history.query_row(
        "SELECT status, duration_ms, error_message FROM backup_executions",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let (location, size, recorded): (String, i64, String) = history.query_row(
        "SELECT location, size_bytes, checksum_sha256 FROM backup_artifacts",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    assert_eq!(status, "succeeded");
    assert!(duration.is_some_and(|ms| ms >= 0));
    assert_eq!(error, None);
    assert_eq!(PathBuf::from(location), dump);
    assert_eq!(size, bytes.len() as i64);
    assert_eq!(recorded, checksum);
    Ok(())
}

#[tokio::test]
async fn failed_pg_dump_leaves_no_artifact() -> TestResult {
    let db = PostgresTestDb::start_seeded().await?;
    let restricted_password = "senha-do-usuario-restrito";
    db.client()
        .batch_execute(&format!(
            "CREATE ROLE restrito LOGIN PASSWORD '{restricted_password}'"
        ))
        .await?;
    let ws = Workspace::new()?;

    let output = ws.command(&db, "restrito", restricted_password).output()?;

    assert_eq!(output.status.code(), Some(8), "{}", text(&output));
    assert!(
        text(&output).contains("permission denied"),
        "{}",
        text(&output)
    );
    assert!(!text(&output).contains(restricted_password));
    assert!(ws.files().is_empty(), "{:?}", ws.files());

    let (status, error): (String, String) = ws.history().query_row(
        "SELECT status, error_message FROM backup_executions",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let artifacts: i64 =
        ws.history()
            .query_row("SELECT COUNT(*) FROM backup_artifacts", [], |row| {
                row.get(0)
            })?;
    assert_eq!(status, "failed");
    assert!(error.contains("pg_dump"), "{error}");
    assert!(!error.contains(restricted_password));
    assert_eq!(artifacts, 0);
    Ok(())
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn large_backup_uses_bounded_memory() -> TestResult {
    let db = PostgresTestDb::start().await?;
    db.client()
        .batch_execute(
            "CREATE TABLE payload AS
             SELECT g AS id, md5(random()::text) || md5(random()::text) AS a,
                    md5(random()::text) || md5(random()::text) AS b
             FROM generate_series(1, 600000) g",
        )
        .await?;
    let ws = Workspace::new()?;

    let mut child = ws
        .command(&db, db.user(), db.password())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let status_path = format!("/proc/{}/status", child.id());
    let mut peak_kb = 0_u64;
    let status = loop {
        if let Ok(status) = std::fs::read_to_string(&status_path)
            && let Some(kb) = status
                .lines()
                .find_map(|line| line.strip_prefix("VmHWM:"))
                .and_then(|value| value.trim().trim_end_matches("kB").trim().parse().ok())
        {
            peak_kb = peak_kb.max(kb);
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };

    assert!(status.success());
    let dump = ws.out.join(&ws.files()[0]);
    let dump_kb = std::fs::metadata(&dump)?.len() / 1024;
    eprintln!("dump: {dump_kb} KiB, pico de memória da CLI: {peak_kb} KiB");
    assert!(
        dump_kb > 40 * 1024,
        "dump pequeno demais para o teste: {dump_kb} KiB"
    );
    assert!(
        peak_kb < dump_kb / 2,
        "memória ({peak_kb} KiB) deveria ser bem menor que o dump ({dump_kb} KiB)"
    );
    Ok(())
}
