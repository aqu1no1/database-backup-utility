#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../../../crates/database-adapters/tests/support/mod.rs"]
mod support;

use std::path::Path;
use std::process::{Command, Output, Stdio};

use support::{PostgresTestDb, TestResult};
use tempfile::TempDir;

fn cli(dir: &TempDir) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_dbu"));
    command
        .env("DBU_DATA_DIR", dir.path().join("data"))
        .env_remove("RUST_LOG")
        .stdin(Stdio::null());
    for engine in ["PG", "MYSQL", "MONGO"] {
        command.env_remove(format!("DBU_{engine}_PASSWORD"));
        command.env_remove(format!("DBU_{engine}_PASSWORD_FILE"));
    }
    command
}

fn check(output: &Output, code: i32) -> serde_json::Value {
    assert_eq!(output.status.code(), Some(code), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).lines().count(),
        1,
        "{output:?}"
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn add_offline(dir: &TempDir, engine: &str) -> Output {
    cli(dir)
        .args([
            "connection",
            "add",
            "dev",
            "-e",
            engine,
            "-d",
            "app",
            "-u",
            "backup",
            "--no-test",
            "--json",
        ])
        .output()
        .unwrap()
}

#[test]
fn profiles_are_persistent_unique_and_removal_requires_confirmation() {
    for (engine, port) in [("postgres", 5432), ("mysql", 3306), ("mongodb", 27017)] {
        let dir = TempDir::new().unwrap();
        check(&add_offline(&dir, engine), 0);
        check(&add_offline(&dir, engine), 2);
        let listed = cli(&dir)
            .args(["connection", "list", "--json"])
            .output()
            .unwrap();
        let list = check(&listed, 0);
        assert_eq!(list["profiles"][0]["engine"], engine);
        assert_eq!(list["profiles"][0]["port"], port);
        assert_eq!(list["profiles"][0]["host"], "localhost");
        let refused = cli(&dir)
            .args(["connection", "remove", "dev", "--json"])
            .output()
            .unwrap();
        check(&refused, 12);
        let removed = cli(&dir)
            .args(["connection", "remove", "dev", "-y", "--json"])
            .output()
            .unwrap();
        check(&removed, 0);
        let absent = cli(&dir)
            .args(["connection", "remove", "dev", "-y", "--json"])
            .output()
            .unwrap();
        check(&absent, 2);
        let empty = cli(&dir)
            .args(["connection", "list", "--json"])
            .output()
            .unwrap();
        assert_eq!(check(&empty, 0)["profiles"], serde_json::json!([]));
    }
}

#[test]
fn invalid_profiles_and_unknown_names_fail_without_network_access() {
    let dir = TempDir::new().unwrap();
    check(&add_offline(&dir, "sqlite"), 2);
    for args in [
        vec![
            "connection",
            "add",
            "../bad",
            "-d",
            "app",
            "-u",
            "backup",
            "--no-test",
        ],
        vec![
            "connection",
            "add",
            "remote",
            "-H",
            "db.example.org",
            "--tls",
            "disable",
            "-d",
            "app",
            "-u",
            "backup",
            "--no-test",
        ],
        vec!["test", "--profile", "missing"],
        vec!["backup", "missing"],
        vec![
            "restore",
            "missing.dump",
            "--profile",
            "missing",
            "--to",
            "copy",
        ],
    ] {
        let output = cli(&dir).args(args).arg("--json").output().unwrap();
        check(&output, 2);
    }
    let empty = cli(&dir)
        .args(["connection", "list", "--json"])
        .output()
        .unwrap();
    assert_eq!(check(&empty, 0)["profiles"], serde_json::json!([]));
}

#[test]
fn profiles_refuse_sqlite_restore_and_add_reports_missing_fields_plainly() {
    let dir = TempDir::new().unwrap();
    check(&add_offline(&dir, "postgres"), 0);
    for args in [
        vec![
            "restore",
            "x.db",
            "--profile",
            "dev",
            "-e",
            "sqlite",
            "--to",
            "y.db",
        ],
        vec!["restore", "x.db", "--profile", "dev", "--target", "y.db"],
    ] {
        let output = cli(&dir).args(args).arg("--json").output().unwrap();
        check(&output, 2);
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("não suportam SQLite"),
            "{output:?}"
        );
    }
    let missing = cli(&dir)
        .args([
            "connection",
            "add",
            "other",
            "-u",
            "backup",
            "--no-test",
            "--json",
        ])
        .output()
        .unwrap();
    check(&missing, 2);
    let stderr = String::from_utf8_lossy(&missing.stderr);
    assert!(stderr.contains("--database é obrigatório"), "{stderr}");
    assert!(!stderr.contains("sem perfil"), "{stderr}");
}

#[cfg(unix)]
fn password_file(path: &Path, password: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, password).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

#[cfg(unix)]
#[test]
fn profile_password_path_is_not_a_secret_and_conflicting_sources_are_refused() {
    let dir = TempDir::new().unwrap();
    let secret = "unique-profile-secret-754038";
    let pass = dir.path().join("profile.pass");
    password_file(&pass, secret);
    let output = cli(&dir)
        .args([
            "connection",
            "add",
            "dev",
            "-d",
            "app",
            "-u",
            "backup",
            "--no-test",
            "--json",
            "--password-file",
        ])
        .arg(&pass)
        .output()
        .unwrap();
    check(&output, 0);
    let listed = cli(&dir)
        .args(["connection", "list", "--json"])
        .output()
        .unwrap();
    assert_eq!(
        check(&listed, 0)["profiles"][0]["password_file"],
        pass.to_str().unwrap()
    );
    let conflict = cli(&dir)
        .args(["test", "dev", "--json"])
        .env("DBU_PG_PASSWORD", "ambient-secret")
        .output()
        .unwrap();
    check(&conflict, 2);
    assert!(String::from_utf8_lossy(&conflict.stderr).contains("DBU_PG_PASSWORD"));
    for entry in std::fs::read_dir(dir.path().join("data")).unwrap() {
        let bytes = std::fs::read(entry.unwrap().path()).unwrap();
        assert!(!bytes.windows(secret.len()).any(|w| w == secret.as_bytes()));
    }
    for output in [&output, &listed, &conflict] {
        assert!(!String::from_utf8_lossy(&output.stdout).contains(secret));
        assert!(!String::from_utf8_lossy(&output.stderr).contains(secret));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("ambient-secret"));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("ambient-secret"));
    }
    std::fs::remove_file(&pass).unwrap();
    let invalid = cli(&dir)
        .args(["test", "--profile", "dev", "--json"])
        .output()
        .unwrap();
    check(&invalid, 2);
    let override_path = dir.path().join("override.pass");
    password_file(&override_path, "different-secret");
    let override_conflict = cli(&dir)
        .args(["test", "dev", "--json", "--password-file"])
        .arg(&override_path)
        .env("DBU_PG_PASSWORD", "env-conflict")
        .output()
        .unwrap();
    check(&override_conflict, 2);
    assert!(String::from_utf8_lossy(&override_conflict.stderr).contains("DBU_PG_PASSWORD"));
    let removed = cli(&dir)
        .args(["connection", "remove", "dev", "-y", "--json"])
        .output()
        .unwrap();
    check(&removed, 0);
    assert_eq!(
        std::fs::read_to_string(override_path).unwrap(),
        "different-secret"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn tested_profile_drives_backup_test_and_restore_without_using_its_database_as_target()
-> TestResult {
    let db = PostgresTestDb::start_seeded().await?;
    let dir = TempDir::new()?;
    let pass = dir.path().join("db.pass");
    password_file(&pass, db.password());
    let connection = [
        "-H",
        db.host(),
        "-p",
        &db.port().to_string(),
        "-d",
        db.database(),
        "-u",
        db.user(),
    ];
    let failed = cli(&dir)
        .args(["connection", "add", "bad", "--json"])
        .args(connection)
        .env("DBU_PG_PASSWORD", "wrong-password")
        .output()?;
    check(&failed, 3);
    let added = cli(&dir)
        .args(["connection", "add", "dev", "--json"])
        .args(connection)
        .arg("--password-file")
        .arg(&pass)
        .output()?;
    check(&added, 0);
    let tested = cli(&dir).args(["test", "dev", "--json"]).output()?;
    check(&tested, 0);
    let tested_long = cli(&dir)
        .args(["connection", "test", "--profile", "dev", "--json"])
        .output()?;
    check(&tested_long, 0);
    let out = dir.path().join("backups");
    let backup = cli(&dir)
        .args(["backup", "dev", "--runner", "docker", "--json", "-o"])
        .arg(&out)
        .output()?;
    let backup_json = check(&backup, 0);
    let file = backup_json["file"].as_str().unwrap();
    let refused = cli(&dir)
        .args(["restore", file, "--profile", "dev", "--json"])
        .output()?;
    check(&refused, 2);
    let restored = cli(&dir)
        .args([
            "restore",
            file,
            "--profile",
            "dev",
            "--to",
            "profile_copy",
            "--runner",
            "docker",
            "--json",
        ])
        .output()?;
    check(&restored, 0);
    let history = rusqlite::Connection::open(dir.path().join("data/history.sqlite3"))?;
    let count: i64 = history.query_row("SELECT count(*) FROM profiles", [], |r| r.get(0))?;
    assert_eq!(count, 1);
    let (db_client, connection_task) = tokio_postgres::Config::new()
        .host(db.host())
        .port(db.port())
        .user(db.user())
        .password(db.password())
        .dbname("profile_copy")
        .connect(tokio_postgres::NoTls)
        .await?;
    let connection_task = tokio::spawn(connection_task);
    let query = "SELECT json_agg(c ORDER BY c.id)::text FROM customers c";
    let expected: String = db.client().query_one(query, &[]).await?.get(0);
    let actual: String = db_client.query_one(query, &[]).await?.get(0);
    assert_eq!(actual, expected);
    connection_task.abort();
    for entry in std::fs::read_dir(dir.path().join("data"))? {
        let bytes = std::fs::read(entry?.path())?;
        assert!(
            !bytes
                .windows(db.password().len())
                .any(|w| w == db.password().as_bytes())
        );
    }
    Ok(())
}
