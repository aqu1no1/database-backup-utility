use std::process::{Command, Output};

use tempfile::TempDir;

fn cli(data_dir: &TempDir, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_database-backup"))
        .args(args)
        .env("DBU_DATA_DIR", data_dir.path())
        .env("DBU_PG_PASSWORD", "segredo-de-teste")
        .env_remove("RUST_LOG")
        .output()
        .expect("o binário da CLI deve executar")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn json(output: &Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!("stdout não é JSON ({error}): {}", stdout(output));
    })
}

#[test]
fn without_command_shows_help_and_exits_with_code_2() {
    let dir = TempDir::new().expect("dir");

    let output = cli(&dir, &[]);

    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("Usage:"), "{}", stderr(&output));
}

#[test]
fn invalid_arguments_exit_with_code_2() {
    let dir = TempDir::new().expect("dir");
    let cases: [&[&str]; 5] = [
        &["backup", "run", "--user", "u"],
        &[
            "connection",
            "test",
            "--database",
            "db",
            "--user",
            "u",
            "--port",
            "0",
        ],
        &[
            "connection",
            "test",
            "--database",
            "db",
            "--user",
            "u",
            "--timeout",
            "0",
        ],
        &["backup", "list", "--limit", "0"],
        &["test-connection", "--database", "db", "--user", "u"],
    ];

    for args in cases {
        let output = cli(&dir, args);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{args:?}: {}",
            stderr(&output)
        );
    }
}

#[test]
fn every_subcommand_help_has_examples() {
    let dir = TempDir::new().expect("dir");
    let commands: [&[&str]; 7] = [
        &[],
        &["connection", "test"],
        &["backup", "run"],
        &["backup", "list"],
        &["backup", "verify"],
        &["restore", "run"],
        &["restore", "list"],
    ];

    for command in commands {
        let output = cli(&dir, &[command, &["--help"]].concat());
        assert_eq!(output.status.code(), Some(0), "{command:?}");
        assert!(stdout(&output).contains("Exemplos:"), "{command:?}");
    }
}

#[test]
fn lists_on_empty_history_succeed_in_text_and_json() {
    let dir = TempDir::new().expect("dir");

    let text = cli(&dir, &["backup", "list"]);
    let backups = cli(&dir, &["backup", "list", "--json"]);
    let restores = cli(&dir, &["restore", "list", "--json"]);

    assert_eq!(text.status.code(), Some(0), "{}", stderr(&text));
    assert!(stdout(&text).contains("Nenhum backup registrado."));
    assert_eq!(backups.status.code(), Some(0));
    assert_eq!(json(&backups)["status"], "ok");
    assert_eq!(json(&backups)["backups"], serde_json::json!([]));
    assert_eq!(restores.status.code(), Some(0));
    assert_eq!(json(&restores)["restores"], serde_json::json!([]));
}

#[test]
fn verify_missing_file_fails_with_code_10_and_json_error() {
    let dir = TempDir::new().expect("dir");
    let missing = dir.path().join("ausente.dump");

    let output = cli(
        &dir,
        &[
            "backup",
            "verify",
            "--json",
            "--file",
            missing.to_str().expect("utf-8"),
        ],
    );

    assert_eq!(output.status.code(), Some(10));
    let body = json(&output);
    assert_eq!(body["status"], "error");
    assert_eq!(body["code"], 10);
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|e| e.contains("não encontrado"))
    );
    assert!(stderr(&output).contains("Erro:"));
}

#[test]
fn verify_without_known_checksum_is_rejected() {
    let dir = TempDir::new().expect("dir");
    let dump = dir.path().join("externo.dump");
    std::fs::write(&dump, b"PGDMP").expect("arquivo");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dump, std::fs::Permissions::from_mode(0o600)).expect("modo");
    }

    let output = cli(
        &dir,
        &["backup", "verify", "--file", dump.to_str().expect("utf-8")],
    );

    assert_eq!(output.status.code(), Some(10));
    assert!(stderr(&output).contains("--allow-unverified"));
}
