#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use rusqlite::Connection;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

struct Env {
    dir: TempDir,
    data: PathBuf,
    out: PathBuf,
}

impl Env {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let data = dir.path().join("data");
        let out = dir.path().join("out");
        Self { dir, data, out }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_database-backup"))
            .args(args)
            .env("DBU_DATA_DIR", &self.data)
            .env_remove("RUST_LOG")
            .env_remove("DBU_PG_PASSWORD")
            .env_remove("DBU_PG_PASSWORD_FILE")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn backup(&self, source: &Path) -> Output {
        self.run(&[
            "--json",
            "backup",
            "run",
            "--engine",
            "sqlite",
            "--source",
            source.to_str().unwrap(),
            "--output-dir",
            self.out.to_str().unwrap(),
        ])
    }

    fn restore(&self, file: &Path, target: &str, extra: &[&str]) -> Output {
        let mut args = vec![
            "--json",
            "restore",
            "run",
            "--engine",
            "sqlite",
            "--file",
            file.to_str().unwrap(),
            "--target",
            target,
        ];
        args.extend_from_slice(extra);
        self.run(&args)
    }

    fn out_files(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.out)
            .map(|entries| {
                entries
                    .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }
}

fn source_db(path: &Path, rows: &[(i64, &str)]) {
    let conn = Connection::open(path).unwrap();
    conn.pragma_update(None, "journal_mode", "WAL").unwrap();
    conn.execute_batch("CREATE TABLE notes (id INTEGER PRIMARY KEY, body TEXT NOT NULL);")
        .unwrap();
    for (id, body) in rows {
        conn.execute(
            "INSERT INTO notes (id, body) VALUES (?1, ?2)",
            rusqlite::params![id, body],
        )
        .unwrap();
    }
}

fn notes(path: &Path) -> Vec<(i64, String)> {
    Connection::open(path)
        .unwrap()
        .prepare("SELECT id, body FROM notes ORDER BY id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn json(output: &Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!("stdout não é JSON ({error}): {}", text(output));
    })
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn sha256_hex(path: &Path) -> String {
    Sha256::digest(std::fs::read(path).unwrap())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn backup_verify_and_restore_round_trip_preserves_rows() {
    let env = Env::new();
    let source = env.path("app.db");
    source_db(
        &source,
        &[(1, "primeira"), (2, "segunda"), (3, "acentuação ✓")],
    );

    let backup = env.backup(&source);
    assert_eq!(backup.status.code(), Some(0), "{}", text(&backup));
    let report = json(&backup);
    let artifact = PathBuf::from(report["file"].as_str().unwrap());
    let manifest_path = PathBuf::from(format!("{}.meta.json", artifact.display()));
    let artifact_name = artifact.file_name().unwrap().to_string_lossy().into_owned();
    assert!(artifact_name.starts_with("app-") && artifact_name.ends_with(".sqlite3"));
    assert_eq!(
        env.out_files(),
        vec![artifact_name.clone(), format!("{artifact_name}.meta.json")]
    );
    assert_eq!(report["sha256"], sha256_hex(&artifact).as_str());
    #[cfg(unix)]
    {
        assert_eq!(mode(&artifact), 0o600);
        assert_eq!(mode(&manifest_path), 0o600);
    }
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    assert_eq!(manifest["manifest_version"], 2);
    assert_eq!(manifest["engine"], "sqlite");
    assert_eq!(manifest["format"], "sqlite-backup-api");
    assert_eq!(manifest["database"], "app.db");
    for absent in ["host", "port", "pg_dump_version", "runner"] {
        assert!(manifest.get(absent).is_none(), "{absent}: {manifest}");
    }

    let verify = env.run(&[
        "--json",
        "backup",
        "verify",
        "--file",
        artifact.to_str().unwrap(),
    ]);
    assert_eq!(verify.status.code(), Some(0), "{}", text(&verify));
    let verified = json(&verify);
    assert_eq!(verified["engine"], "sqlite");
    assert_eq!(verified["checksum_verified"], true);
    assert_eq!(verified["tables"], 1);

    let target = env.path("copia.db");
    let restore = env.restore(&artifact, target.to_str().unwrap(), &[]);
    assert_eq!(restore.status.code(), Some(0), "{}", text(&restore));
    assert_eq!(json(&restore)["mode"], "create");
    assert_eq!(notes(&target), notes(&source));
    #[cfg(unix)]
    assert_eq!(mode(&target), 0o600);
    assert!(!env.path("copia.db.partial").exists());

    let backups = json(&env.run(&["--json", "backup", "list"]));
    assert_eq!(
        backups["backups"][0]["database"],
        source.display().to_string().as_str()
    );
    assert_eq!(backups["backups"][0]["status"], "succeeded");
    let restores = json(&env.run(&["--json", "restore", "list"]));
    assert_eq!(
        restores["restores"][0]["target_database"],
        target.display().to_string().as_str()
    );
    assert_eq!(restores["restores"][0]["status"], "succeeded");
}

#[test]
fn restore_never_replaces_an_existing_file_without_confirmation() {
    let env = Env::new();
    let source = env.path("app.db");
    source_db(&source, &[(1, "do backup")]);
    let backup = env.backup(&source);
    assert_eq!(backup.status.code(), Some(0), "{}", text(&backup));
    let artifact = PathBuf::from(json(&backup)["file"].as_str().unwrap());
    let target = env.path("producao.db");
    source_db(&target, &[(7, "dado atual")]);
    let typed = target.to_str().unwrap();

    for extra in [
        &[][..],
        &["--confirm", typed][..],
        &["--overwrite"][..],
        &["--overwrite", "--confirm", "outro.db"][..],
    ] {
        let refused = env.restore(&artifact, typed, extra);
        assert_eq!(
            refused.status.code(),
            Some(12),
            "{extra:?}: {}",
            text(&refused)
        );
        assert_eq!(
            notes(&target),
            vec![(7, "dado atual".to_owned())],
            "{extra:?}"
        );
    }

    let replaced = env.restore(&artifact, typed, &["--overwrite", "--confirm", typed]);
    assert_eq!(replaced.status.code(), Some(0), "{}", text(&replaced));
    let report = json(&replaced);
    let previous = PathBuf::from(report["previous_database"].as_str().unwrap());
    assert_eq!(report["mode"], "overwrite");
    assert_eq!(notes(&target), vec![(1, "do backup".to_owned())]);
    assert_eq!(notes(&previous), vec![(7, "dado atual".to_owned())]);
    assert!(
        previous
            .to_string_lossy()
            .starts_with(&format!("{typed}.previous-"))
    );
}

#[test]
fn damaged_sources_fail_without_publishing_a_backup() {
    let env = Env::new();
    let healthy = env.path("saudavel.db");
    source_db(&healthy, &[(1, "x")]);
    Connection::open(&healthy)
        .unwrap()
        .execute_batch(
            "PRAGMA journal_mode = DELETE;
             WITH RECURSIVE n(i) AS (SELECT 2 UNION ALL SELECT i + 1 FROM n WHERE i < 500)
             INSERT INTO notes (id, body) SELECT i, hex(randomblob(500)) FROM n;",
        )
        .unwrap();
    let bytes = std::fs::read(&healthy).unwrap();
    let truncated = env.path("truncado.db");
    std::fs::write(&truncated, &bytes[..bytes.len() / 2]).unwrap();
    let inconsistent = env.path("indice_inconsistente.db");
    source_db(&inconsistent, &[(1, "a"), (2, "b")]);
    Connection::open(&inconsistent)
        .unwrap()
        .execute_batch(
            "CREATE INDEX notes_body ON notes (body);
             PRAGMA writable_schema = ON;
             UPDATE sqlite_schema SET sql = 'CREATE INDEX notes_body ON notes (id)'
             WHERE name = 'notes_body';",
        )
        .unwrap();

    for damaged in [&truncated, &inconsistent] {
        let output = env.backup(damaged);

        assert_eq!(output.status.code(), Some(8), "{}", text(&output));
        assert_eq!(env.out_files(), Vec::<String>::new());
        let backups = json(&env.run(&["--json", "backup", "list"]));
        assert_eq!(backups["backups"][0]["status"], "failed");
    }
}

#[test]
fn invalid_sources_and_mixed_engine_arguments_are_rejected() {
    let env = Env::new();
    let source = env.path("app.db");
    source_db(&source, &[(1, "x")]);
    let text_file = env.path("notas.txt");
    std::fs::write(&text_file, "não sou um banco SQLite, só texto comum").unwrap();
    let listed = env.run(&["backup", "list"]);
    assert_eq!(listed.status.code(), Some(0), "{}", text(&listed));
    let history = env.data.join("history.sqlite3");
    let source_arg = source.to_str().unwrap();

    let cases: Vec<(&str, Vec<&str>, i32, &str)> = vec![
        (
            "histórico interno",
            vec![
                "backup",
                "run",
                "--engine",
                "sqlite",
                "--source",
                history.to_str().unwrap(),
            ],
            2,
            "histórico interno",
        ),
        (
            "não é SQLite",
            vec![
                "backup",
                "run",
                "--engine",
                "sqlite",
                "--source",
                text_file.to_str().unwrap(),
            ],
            2,
            "não é um banco SQLite",
        ),
        (
            "inexistente",
            vec![
                "backup",
                "run",
                "--engine",
                "sqlite",
                "--source",
                "/nao/existe.db",
            ],
            4,
            "não encontrado",
        ),
        (
            "sqlite com usuário",
            vec![
                "backup", "run", "--engine", "sqlite", "--source", source_arg, "--user", "u",
            ],
            2,
            "cannot be used with",
        ),
        (
            "source sem engine",
            vec!["backup", "run", "--source", source_arg],
            2,
            "--source só vale com --engine sqlite",
        ),
        (
            "sqlite sem source",
            vec!["backup", "run", "--engine", "sqlite"],
            2,
            "--source",
        ),
        (
            "restore com target-database",
            vec![
                "restore",
                "run",
                "--engine",
                "sqlite",
                "--file",
                source_arg,
                "--target",
                "x.db",
                "--target-database",
                "x",
            ],
            2,
            "cannot be used with",
        ),
    ];

    for (name, args, code, expected) in cases {
        let output = env.run(&args);
        assert_eq!(
            output.status.code(),
            Some(code),
            "{name}: {}",
            text(&output)
        );
        assert!(
            text(&output).contains(expected),
            "{name}: {}",
            text(&output)
        );
    }
    assert_eq!(env.out_files(), Vec::<String>::new());
}

#[cfg(unix)]
#[test]
fn symlinked_source_is_rejected() {
    let env = Env::new();
    let source = env.path("app.db");
    source_db(&source, &[(1, "x")]);
    let link = env.path("link.db");
    std::os::unix::fs::symlink(&source, &link).unwrap();

    let output = env.backup(&link);

    assert_eq!(output.status.code(), Some(2), "{}", text(&output));
    assert!(
        text(&output).contains("link simbólico"),
        "{}",
        text(&output)
    );
}

fn age(path: &Path, by: std::time::Duration) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(std::time::SystemTime::now() - by)
        .unwrap();
}

#[test]
fn backup_removes_only_old_orphan_partials() {
    let env = Env::new();
    let source = env.path("app.db");
    source_db(&source, &[(1, "x")]);
    std::fs::create_dir(&env.out).unwrap();
    let old = env.out.join("antigo-20261001T000000Z.dump.partial");
    let recent = env.out.join("recente-20261022T000000Z.sqlite3.partial");
    std::fs::write(&old, b"lixo").unwrap();
    std::fs::write(&recent, b"lixo").unwrap();
    age(&old, std::time::Duration::from_secs(3600));

    let output = env.backup(&source);

    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    assert!(
        !old.exists(),
        "o .partial órfão antigo deveria ser removido"
    );
    assert!(
        recent.exists(),
        "um .partial recente pode ser de outra versão em andamento"
    );
    assert!(
        text(&output).contains("removido arquivo .partial"),
        "{}",
        text(&output)
    );
}

#[test]
fn orphan_partial_from_an_interrupted_restore_does_not_block_a_new_one() {
    let env = Env::new();
    let source = env.path("app.db");
    source_db(&source, &[(1, "x")]);
    let backup = env.backup(&source);
    assert_eq!(backup.status.code(), Some(0), "{}", text(&backup));
    let artifact = PathBuf::from(json(&backup)["file"].as_str().unwrap());
    let target = env.path("copia.db");
    std::fs::write(env.path("copia.db.partial"), b"restore interrompido").unwrap();

    let restore = env.restore(&artifact, target.to_str().unwrap(), &[]);

    assert_eq!(restore.status.code(), Some(0), "{}", text(&restore));
    assert_eq!(notes(&target), vec![(1, "x".to_owned())]);
    assert!(!env.path("copia.db.partial").exists());
}

#[cfg(target_os = "linux")]
#[test]
fn restore_refuses_a_target_held_open_by_another_process() {
    let env = Env::new();
    let source = env.path("app.db");
    source_db(&source, &[(1, "do backup")]);
    let backup = env.backup(&source);
    assert_eq!(backup.status.code(), Some(0), "{}", text(&backup));
    let artifact = PathBuf::from(json(&backup)["file"].as_str().unwrap());
    let target = env.path("producao.db");
    source_db(&target, &[(7, "dado atual")]);
    Connection::open(&target)
        .unwrap()
        .pragma_update(None, "journal_mode", "DELETE")
        .unwrap();
    let holder = FileHolder::spawn(&target);
    let pid = holder.pid();
    wait_until_open(pid, &target);
    let typed = target.to_str().unwrap();

    let refused = env.restore(&artifact, typed, &["--overwrite", "--confirm", typed]);
    drop(holder);

    assert_eq!(refused.status.code(), Some(11), "{}", text(&refused));
    assert!(
        text(&refused).contains(&pid.to_string()),
        "{}",
        text(&refused)
    );
    assert_eq!(notes(&target), vec![(7, "dado atual".to_owned())]);
}

#[cfg(target_os = "linux")]
fn wait_until_open(pid: u32, path: &Path) {
    let wanted = path.canonicalize().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        let open = std::fs::read_dir(format!("/proc/{pid}/fd")).is_ok_and(|fds| {
            fds.flatten()
                .any(|fd| std::fs::read_link(fd.path()).is_ok_and(|link| link == wanted))
        });
        if open {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("o processo {pid} não abriu {} a tempo", path.display());
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
