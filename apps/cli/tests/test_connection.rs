#[path = "../../../crates/database-adapters/tests/support/mod.rs"]
mod support;

use std::process::{Command, Output};

use support::{PostgresTestDb, TestResult};

fn run_cli(args: &[&str], password: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_database-backup"));
    command
        .args(["connection", "test"])
        .args(args)
        .env_remove("DBU_PG_PASSWORD");
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
