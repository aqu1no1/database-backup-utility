#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../../../crates/database-adapters/tests/support/mod.rs"]
mod support;

use std::process::{Command, Output};

use support::{PostgresTestDb, TestResult};

fn run_cli(args: &[&str], password: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_database-backup"));
    command
        .args(["connection", "test"])
        .args(args)
        .env_remove("DBU_PG_PASSWORD")
        .env_remove("DBU_PG_PASSWORD_FILE");
    if let Some(password) = password {
        command.env("DBU_PG_PASSWORD", password);
    }
    command.output().expect("o binário da CLI deve executar")
}

fn assert_no_secret(output: &Output, secret: &str) {
    assert!(!String::from_utf8_lossy(&output.stdout).contains(secret));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(secret));
}

#[test]
fn invalid_parameters_exit_with_code_2() {
    let output = run_cli(
        &["--host", "", "--database", "db", "--user", "u"],
        Some("x"),
    );

    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("host"));
}

#[test]
fn missing_password_without_terminal_exits_with_code_2() {
    let output = run_cli(&["--database", "db", "--user", "u"], None);

    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("DBU_PG_PASSWORD"));
}

#[tokio::test]
async fn successful_and_failed_connections_use_exit_codes() -> TestResult {
    let db = PostgresTestDb::start().await?;
    let port = db.port().to_string();
    let base = [
        "--host",
        db.host(),
        "--port",
        &port,
        "--user",
        db.user(),
        "--timeout",
        "5",
    ];

    let ok = run_cli(
        &[&base[..], &["--database", db.database()]].concat(),
        Some(db.password()),
    );
    assert_eq!(ok.status.code(), Some(0), "{ok:?}");
    assert!(String::from_utf8_lossy(&ok.stdout).contains("Conexão OK: PostgreSQL 17."));
    assert_no_secret(&ok, db.password());

    let wrong = "senha-incorreta-de-teste";
    let auth = run_cli(
        &[&base[..], &["--database", db.database()]].concat(),
        Some(wrong),
    );
    assert_eq!(auth.status.code(), Some(3), "{auth:?}");
    assert_no_secret(&auth, wrong);

    let missing = run_cli(
        &[&base[..], &["--database", "banco_inexistente"]].concat(),
        Some(db.password()),
    );
    assert_eq!(missing.status.code(), Some(4), "{missing:?}");
    assert_no_secret(&missing, db.password());
    Ok(())
}

#[cfg(unix)]
fn password_file(dir: &std::path::Path, name: &str, value: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, format!("{value}\n")).expect("arquivo de senha");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("modo");
    path
}

#[cfg(unix)]
#[tokio::test]
async fn password_file_authenticates_without_leaking() -> TestResult {
    let db = PostgresTestDb::start().await?;
    let dir = tempfile::tempdir()?;
    let port = db.port().to_string();
    let right = password_file(dir.path(), "certa.pass", db.password());
    let wrong_value = "senha-errada-do-arquivo";
    let wrong = password_file(dir.path(), "errada.pass", wrong_value);
    let args = |file: &std::path::Path| {
        vec![
            "--host".to_owned(),
            db.host().to_owned(),
            "--port".to_owned(),
            port.clone(),
            "--user".to_owned(),
            db.user().to_owned(),
            "--database".to_owned(),
            db.database().to_owned(),
            "--password-file".to_owned(),
            file.display().to_string(),
        ]
    };
    let run = |file: &std::path::Path| {
        let args = args(file);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        run_cli(&args, None)
    };

    let ok = run(&right);
    assert_eq!(ok.status.code(), Some(0), "{ok:?}");
    assert_no_secret(&ok, db.password());

    let denied = run(&wrong);
    assert_eq!(denied.status.code(), Some(3), "{denied:?}");
    assert_no_secret(&denied, wrong_value);
    Ok(())
}
