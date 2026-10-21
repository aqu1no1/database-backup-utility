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
async fn restores_backup_into_new_database_with_same_data() -> TestResult {
    let db = PostgresTestDb::start_seeded().await?;
    let (ws, dump) = backed_up(&db).await?;

    let output = ws.restore(&db, &dump, "restaurado").output()?;

    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    assert!(!text(&output).contains(db.password()));
    assert_eq!(
        query_i64(&db, "restaurado", "SELECT COUNT(*) FROM customers").await?,
        3
    );
    assert_eq!(
        query_i64(
            &db,
            "restaurado",
            "SELECT SUM(total_cents)::BIGINT FROM orders"
        )
        .await?,
        18490
    );
    assert_eq!(
        query_i64(
            &db,
            "restaurado",
            "SELECT COUNT(*) FROM customers WHERE name = 'Cliente Gama' AND created_at = '2026-03-30'",
        )
        .await?,
        1
    );
    let (status, mode, linked): (String, String, bool) = ws.history().query_row(
        "SELECT status, mode, artifact_id IS NOT NULL FROM restore_executions",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    assert_eq!(
        (status.as_str(), mode.as_str(), linked),
        ("succeeded", "create", true)
    );
    Ok(())
}

#[tokio::test]
async fn rejects_missing_corrupted_and_unverified_files() -> TestResult {
    let db = PostgresTestDb::start_seeded().await?;
    let (ws, dump) = backed_up(&db).await?;
    let dir = tempfile::tempdir()?;

    let missing = ws
        .restore(&db, &dir.path().join("nao.dump"), "x")
        .output()?;
    assert_eq!(missing.status.code(), Some(10), "{}", text(&missing));

    let mut bytes = std::fs::read(&dump)?;
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0xFF;
    std::fs::write(&dump, &bytes)?;
    let corrupted = ws.restore(&db, &dump, "corrompido").output()?;
    assert_eq!(corrupted.status.code(), Some(10), "{}", text(&corrupted));
    assert!(text(&corrupted).contains("checksum divergente"));
    assert!(databases_like(&db, "corrompido%").await?.is_empty());

    bytes[middle] ^= 0xFF;
    std::fs::write(&dump, &bytes)?;
    let external = copy_without_metadata(&dump, dir.path())?;
    let unverified = ws.restore(&db, &external, "externo").output()?;
    assert_eq!(unverified.status.code(), Some(10), "{}", text(&unverified));
    assert!(text(&unverified).contains("--allow-unverified"));

    let size = bytes.len();
    let truncated = dir.path().join("truncado.dump");
    std::fs::write(&truncated, &bytes[..size * 3 / 4])?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&truncated, std::fs::Permissions::from_mode(0o600))?;
    }
    let invalid = ws
        .restore(&db, &truncated, "truncado")
        .arg("--allow-unverified")
        .output()?;
    assert_eq!(invalid.status.code(), Some(10), "{}", text(&invalid));
    assert!(databases_like(&db, "truncado%").await?.is_empty());

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
async fn overwrite_requires_confirmation_and_keeps_previous_database() -> TestResult {
    let db = PostgresTestDb::start_seeded().await?;
    let (ws, dump) = backed_up(&db).await?;
    let target = db.database();
    db.client()
        .batch_execute(
            "INSERT INTO customers (name, created_at) VALUES ('Depois do backup', '2026-10-20')",
        )
        .await?;

    let refused = ws.restore(&db, &dump, target).output()?;
    assert_eq!(refused.status.code(), Some(12), "{}", text(&refused));
    assert!(text(&refused).contains("--overwrite"));

    let unconfirmed = ws.restore(&db, &dump, target).arg("--overwrite").output()?;
    assert_eq!(
        unconfirmed.status.code(),
        Some(12),
        "{}",
        text(&unconfirmed)
    );

    let wrong = ws
        .restore(&db, &dump, target)
        .args(["--overwrite", "--confirm", "outro"])
        .output()?;
    assert_eq!(wrong.status.code(), Some(12), "{}", text(&wrong));
    assert_eq!(
        query_i64(&db, target, "SELECT COUNT(*) FROM customers").await?,
        4
    );

    let confirmed = ws
        .restore(&db, &dump, target)
        .args(["--overwrite", "--confirm", target])
        .output()?;
    assert_eq!(confirmed.status.code(), Some(0), "{}", text(&confirmed));

    let previous = databases_like(&db, &format!("{target}__previous_%")).await?;
    assert_eq!(previous.len(), 1, "{previous:?}");
    assert!(
        databases_like(&db, &format!("{target}__restore_%"))
            .await?
            .is_empty()
    );
    assert_eq!(
        query_i64(&db, target, "SELECT COUNT(*) FROM customers").await?,
        3
    );
    assert_eq!(
        query_i64(&db, &previous[0], "SELECT COUNT(*) FROM customers").await?,
        4
    );
    let (mode, kept): (String, String) = ws.history().query_row(
        "SELECT mode, previous_database FROM restore_executions WHERE status = 'succeeded'",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert_eq!(
        (mode.as_str(), kept.as_str()),
        ("overwrite", previous[0].as_str())
    );
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
