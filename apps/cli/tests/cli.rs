#![allow(clippy::unwrap_used, clippy::expect_used)]

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

#[cfg(unix)]
fn private_file(path: &std::path::Path, body: &[u8]) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, body).expect("arquivo");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).expect("modo");
}

#[cfg(unix)]
#[test]
fn verify_rejects_unsupported_or_foreign_manifest() {
    let dir = TempDir::new().expect("dir");
    let dump = dir.path().join("app.dump");
    let manifest = dir.path().join("app.dump.meta.json");
    private_file(&dump, b"PGDMP");

    private_file(&manifest, br#"{"manifest_version":2,"sha256":"x"}"#);
    let future = cli(
        &dir,
        &["backup", "verify", "--file", dump.to_str().expect("utf-8")],
    );
    assert_eq!(future.status.code(), Some(10), "{}", stderr(&future));
    assert!(
        stderr(&future).contains("versão 2 do manifesto"),
        "{}",
        stderr(&future)
    );

    let body = serde_json::json!({
        "manifest_version": 1,
        "tool": { "name": "database-backup", "version": "0.1.0" },
        "engine": "postgresql",
        "format": "pg_dump-custom",
        "file_name": "outro.dump",
        "sha256": "0".repeat(64),
        "size_bytes": 5,
        "database": "app",
        "host": "localhost",
        "port": 5432,
        "server_version": "17.0",
        "pg_dump_version": "17.0",
        "runner": "docker",
        "created_at": "2026-10-21T12:00:00Z",
        "execution_id": "x"
    });
    private_file(&manifest, body.to_string().as_bytes());
    let foreign = cli(
        &dir,
        &["backup", "verify", "--file", dump.to_str().expect("utf-8")],
    );
    assert_eq!(foreign.status.code(), Some(10), "{}", stderr(&foreign));
    assert!(
        stderr(&foreign).contains("pertence a outro arquivo"),
        "{}",
        stderr(&foreign)
    );
}

const FILE_SECRET: &str = "conteudo-secreto-do-arquivo";

fn without_password(data_dir: &TempDir, args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_database-backup"));
    command
        .args(args)
        .env("DBU_DATA_DIR", data_dir.path())
        .env_remove("DBU_PG_PASSWORD")
        .env_remove("DBU_PG_PASSWORD_FILE")
        .env_remove("RUST_LOG")
        .stdin(std::process::Stdio::null());
    for (name, value) in envs {
        command.env(name, value);
    }
    command.output().expect("o binário da CLI deve executar")
}

#[cfg(unix)]
#[test]
fn password_sources_are_validated_without_leaking_contents() {
    let dir = TempDir::new().expect("dir");
    let private = dir.path().join("pg.pass");
    let open = dir.path().join("aberto.pass");
    let empty = dir.path().join("vazio.pass");
    private_file(&private, FILE_SECRET.as_bytes());
    private_file(&empty, b"\n");
    std::fs::write(&open, FILE_SECRET).expect("arquivo");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o644)).expect("modo");
    }
    let base = [
        "connection",
        "test",
        "--host",
        "127.0.0.1",
        "--port",
        "1",
        "--database",
        "db",
        "--user",
        "u",
        "--timeout",
        "1",
    ];
    let with_file = |path: &std::path::Path| {
        let mut args: Vec<String> = base.iter().map(|arg| (*arg).to_owned()).collect();
        args.push("--password-file".to_owned());
        args.push(path.display().to_string());
        args
    };
    let run = |args: &[String], envs: &[(&str, &str)]| {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        without_password(&dir, &args, envs)
    };
    let base_args: Vec<String> = base.iter().map(|arg| (*arg).to_owned()).collect();
    let private_text = private.display().to_string();

    let cases: Vec<(&str, Output, &str)> = vec![
        ("sem fonte", run(&base_args, &[]), "DBU_PG_PASSWORD_FILE"),
        (
            "flag + env",
            run(&with_file(&private), &[("DBU_PG_PASSWORD", "outra")]),
            "mais de uma fonte de senha definida: --password-file, DBU_PG_PASSWORD",
        ),
        (
            "flag + env de arquivo",
            run(
                &with_file(&private),
                &[("DBU_PG_PASSWORD_FILE", &private_text)],
            ),
            "--password-file, DBU_PG_PASSWORD_FILE",
        ),
        ("permissão aberta", run(&with_file(&open), &[]), "chmod 600"),
        (
            "inexistente",
            run(&with_file(&dir.path().join("ausente.pass")), &[]),
            "não encontrado",
        ),
        ("vazio", run(&with_file(&empty), &[]), "vazio"),
    ];

    for (name, output, expected) in cases {
        assert_eq!(output.status.code(), Some(2), "{name}: {}", stderr(&output));
        assert!(
            stderr(&output).contains(expected),
            "{name}: {}",
            stderr(&output)
        );
        assert!(
            !stdout(&output).contains(FILE_SECRET) && !stderr(&output).contains(FILE_SECRET),
            "{name}: a saída contém o conteúdo do arquivo de senha"
        );
    }

    let from_env_file = run(&base_args, &[("DBU_PG_PASSWORD_FILE", &private_text)]);
    assert_eq!(
        from_env_file.status.code(),
        Some(5),
        "{}",
        stderr(&from_env_file)
    );
    assert!(!stderr(&from_env_file).contains(FILE_SECRET));
}
