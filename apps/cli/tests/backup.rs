#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;
#[path = "../../../crates/database-adapters/tests/support/mod.rs"]
mod support;

use std::path::PathBuf;
use std::process::Stdio;

use common::{Workspace, sha256_hex, text};
use support::{PostgresTestDb, TestResult};

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
    assert_eq!(metadata["manifest_version"], 1);
    assert_eq!(metadata["engine"], "postgresql");
    assert_eq!(metadata["tool"]["name"], "database-backup");
    assert_eq!(metadata["tool"]["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        metadata["file_name"],
        dump.file_name().expect("nome").to_string_lossy().as_ref()
    );

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

#[tokio::test]
async fn verify_list_and_logs_after_backup() -> TestResult {
    let db = PostgresTestDb::start_seeded().await?;
    let ws = Workspace::new()?;

    let backup = ws
        .command(&db, db.user(), db.password())
        .args(["--log-level", "info", "--json"])
        .output()?;
    assert_eq!(backup.status.code(), Some(0), "{}", text(&backup));
    let result: serde_json::Value = serde_json::from_slice(&backup.stdout)?;
    let logs = String::from_utf8_lossy(&backup.stderr);
    assert_eq!(result["status"], "ok");
    assert!(logs.contains("operation=\"backup.run\""), "{logs}");
    assert!(logs.contains("status=\"succeeded\""), "{logs}");
    assert!(!text(&backup).contains(db.password()));
    let dump = ws.dump_path();
    assert_eq!(result["file"], dump.display().to_string());

    let verify = |file: &std::path::Path| {
        ws.cli(db.password())
            .args(["backup", "verify", "--runner", "docker", "--json", "--file"])
            .arg(file)
            .output()
    };
    let valid = verify(&dump)?;
    assert_eq!(valid.status.code(), Some(0), "{}", text(&valid));
    let report: serde_json::Value = serde_json::from_slice(&valid.stdout)?;
    assert_eq!(report["checksum_verified"], true);
    assert_eq!(report["sha256"], sha256_hex(&dump).as_str());
    assert!(report["tables"].as_u64().is_some_and(|tables| tables > 0));

    let listed = ws
        .cli(db.password())
        .args(["backup", "list", "--json"])
        .output()?;
    assert_eq!(listed.status.code(), Some(0), "{}", text(&listed));
    let listing: serde_json::Value = serde_json::from_slice(&listed.stdout)?;
    let backups = listing["backups"].as_array().expect("lista de backups");
    assert_eq!(backups.len(), 1);
    assert_eq!(backups[0]["status"], "succeeded");
    assert_eq!(backups[0]["file"], dump.display().to_string());
    assert_eq!(backups[0]["database"], db.database());

    let mut bytes = std::fs::read(&dump)?;
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0xFF;
    std::fs::write(&dump, &bytes)?;
    let corrupted = verify(&dump)?;
    assert_eq!(corrupted.status.code(), Some(10), "{}", text(&corrupted));
    assert!(text(&corrupted).contains("checksum divergente"));
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn shared_output_directory_triggers_a_warning() -> TestResult {
    use std::os::unix::fs::PermissionsExt;

    let db = PostgresTestDb::start_seeded().await?;
    let ws = Workspace::new()?;
    std::fs::create_dir(&ws.out)?;
    std::fs::set_permissions(&ws.out, std::fs::Permissions::from_mode(0o755))?;

    let output = ws.command(&db, db.user(), db.password()).output()?;

    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("acessível por grupo ou outros"), "{stderr}");
    let dump = ws.dump_path();
    assert_eq!(
        std::fs::metadata(&dump)?.permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(&ws.out)?.permissions().mode() & 0o777,
        0o755
    );
    Ok(())
}
