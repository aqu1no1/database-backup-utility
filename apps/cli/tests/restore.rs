#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;
#[path = "../../../crates/database-adapters/tests/support/mod.rs"]
mod support;

use std::path::{Path, PathBuf};

use common::{Workspace, text};
use support::{PostgresTestDb, TestResult};

async fn connect(db: &PostgresTestDb, database: &str) -> TestResult<tokio_postgres::Client> {
    let (client, connection) = tokio_postgres::Config::new()
        .host(db.host())
        .port(db.port())
        .user(db.user())
        .password(db.password())
        .dbname(database)
        .connect(tokio_postgres::NoTls)
        .await?;
    tokio::spawn(connection);
    Ok(client)
}

async fn query_i64(db: &PostgresTestDb, database: &str, sql: &str) -> TestResult<i64> {
    Ok(connect(db, database)
        .await?
        .query_one(sql, &[])
        .await?
        .get(0))
}

async fn databases_like(db: &PostgresTestDb, pattern: &str) -> TestResult<Vec<String>> {
    let rows = connect(db, "postgres")
        .await?
        .query(
            "SELECT datname FROM pg_database WHERE datname LIKE $1 ORDER BY datname",
            &[&pattern],
        )
        .await?;
    Ok(rows.iter().map(|row| row.get(0)).collect())
}

async fn backed_up(db: &PostgresTestDb) -> TestResult<(Workspace, PathBuf)> {
    let ws = Workspace::new()?;
    let output = ws.command(db, db.user(), db.password()).output()?;
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    let dump = ws.dump_path();
    Ok((ws, dump))
}

fn copy_without_metadata(dump: &Path, dir: &Path) -> TestResult<PathBuf> {
    let copy = dir.join("externo.dump");
    std::fs::copy(dump, &copy)?;
    Ok(copy)
}

#[tokio::test]
async fn unverified_archive_requires_explicit_flag_and_is_recorded() -> TestResult {
    let db = PostgresTestDb::start_seeded().await?;
    let (ws, dump) = backed_up(&db).await?;
    let dir = tempfile::tempdir()?;
    let external = copy_without_metadata(&dump, dir.path())?;

    let unverified = ws.restore(&db, &external, "externo").output()?;
    assert_eq!(unverified.status.code(), Some(10), "{}", text(&unverified));
    assert!(text(&unverified).contains("--allow-unverified"));
    assert!(databases_like(&db, "externo%").await?.is_empty());

    let allowed = ws
        .restore(&db, &external, "externo")
        .arg("--allow-unverified")
        .output()?;
    assert_eq!(allowed.status.code(), Some(0), "{}", text(&allowed));
    assert!(String::from_utf8_lossy(&allowed.stderr).contains("checksum não verificado"));
    assert_eq!(
        query_i64(&db, "externo", "SELECT COUNT(*) FROM orders").await?,
        3
    );
    let verified: bool = ws.history().query_row(
        "SELECT checksum_verified FROM restore_executions WHERE target_database = 'externo' AND status = 'succeeded'",
        [],
        |row| row.get(0),
    )?;
    assert!(!verified);
    Ok(())
}

#[cfg(unix)]
#[test]
fn rejects_unsafe_source_files() -> TestResult {
    use std::os::unix::fs::PermissionsExt;

    let ws = Workspace::new()?;
    let dir = tempfile::tempdir()?;
    let private = dir.path().join("privado.dump");
    std::fs::write(&private, b"dump")?;
    std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o600))?;
    let link = dir.path().join("link.dump");
    std::os::unix::fs::symlink(&private, &link)?;
    let shared = dir.path().join("compartilhado.dump");
    std::fs::write(&shared, b"dump")?;
    std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o666))?;

    for (file, reason) in [
        (&link, "link simbólico"),
        (&shared, "gravável por grupo ou outros"),
    ] {
        let output = ws
            .cli("segredo")
            .args([
                "restore",
                "run",
                "--user",
                "postgres",
                "--target-database",
                "x",
            ])
            .arg("--file")
            .arg(file)
            .output()?;
        assert_eq!(output.status.code(), Some(10), "{}", text(&output));
        assert!(text(&output).contains(reason), "{}", text(&output));
    }
    Ok(())
}

#[tokio::test]
async fn failed_restore_drops_new_database_and_records_failure() -> TestResult {
    let db = PostgresTestDb::start_seeded().await?;
    let (ws, dump) = backed_up(&db).await?;
    let template = connect(&db, "template1").await?;
    template
        .batch_execute("CREATE TABLE customers (id INTEGER)")
        .await?;
    drop(template);
    for _ in 0..50 {
        let sessions = query_i64(
            &db,
            "postgres",
            "SELECT COUNT(*) FROM pg_stat_activity WHERE datname = 'template1'",
        )
        .await?;
        if sessions == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    let output = ws.restore(&db, &dump, "conflito").output()?;

    assert_eq!(output.status.code(), Some(11), "{}", text(&output));
    assert!(!text(&output).contains(db.password()));
    assert!(databases_like(&db, "conflito%").await?.is_empty());
    let (status, error): (String, String) = ws.history().query_row(
        "SELECT status, error_message FROM restore_executions",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert_eq!(status, "failed");
    assert!(error.contains("pg_restore"), "{error}");
    assert!(error.contains("already exists"), "{error}");
    assert!(!error.contains(db.password()));
    Ok(())
}
