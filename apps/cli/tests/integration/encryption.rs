use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use rusqlite::Connection;
use tempfile::TempDir;

use crate::common::{Workspace, text};
use crate::support::{PostgresTestDb, TestResult};

const SECRET_PREFIX: &str = "AGE-SECRET-KEY-";

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
        Command::new(env!("CARGO_BIN_EXE_dbu"))
            .args(args)
            .current_dir(self.dir.path())
            .env("DBU_DATA_DIR", &self.data)
            .env_remove("RUST_LOG")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn key(&self, name: &str) -> (PathBuf, String) {
        let path = self.path(name);
        let output = self.run(&["--json", "key", "generate", "-o", path.to_str().unwrap()]);
        assert_eq!(output.status.code(), Some(0), "{}", text(&output));
        let public = json(&output)["public_keys"][0].as_str().unwrap().to_owned();
        assert!(public.starts_with("age1"), "{public}");
        assert!(!text(&output).contains(SECRET_PREFIX));
        (path, public)
    }

    fn backup(&self, source: &Path, extra: &[&str]) -> Output {
        let mut args = vec![
            "--json",
            "--log-level",
            "trace",
            "backup",
            source.to_str().unwrap(),
            "-o",
            self.out.to_str().unwrap(),
        ];
        args.extend_from_slice(extra);
        self.run(&args)
    }

    fn work_entries(&self) -> Vec<PathBuf> {
        std::fs::read_dir(self.data.join("work"))
            .map(|entries| entries.map(|entry| entry.unwrap().path()).collect())
            .unwrap_or_default()
    }
}

fn source_db(path: &Path) -> Vec<(i64, String)> {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch("CREATE TABLE notes (id INTEGER PRIMARY KEY, body TEXT NOT NULL);")
        .unwrap();
    for id in 0..2_000 {
        conn.execute(
            "INSERT INTO notes (id, body) VALUES (?1, ?2)",
            rusqlite::params![id, format!("anotação confidencial {id}")],
        )
        .unwrap();
    }
    notes(path)
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

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[test]
fn encrypted_backup_restores_only_with_a_recipient_key_and_never_leaks_it() {
    let env = Env::new();
    let source = env.path("app.db");
    let expected = source_db(&source);
    let (main_key, main_public) = env.key("main.key");
    let (recovery_key, recovery_public) = env.key("recovery.key");
    let (other_key, _) = env.key("other.key");

    let backup = env.backup(
        &source,
        &[
            "--compress",
            "--recipient",
            &main_public,
            "--recipient",
            &recovery_public,
        ],
    );
    assert_eq!(backup.status.code(), Some(0), "{}", text(&backup));
    let result = json(&backup);
    assert_eq!(result["compression"], "zstd");
    assert_eq!(result["encryption"], "age");
    let file = PathBuf::from(result["file"].as_str().unwrap());
    assert!(
        file.to_string_lossy().ends_with(".sqlite3.zst.age"),
        "{}",
        file.display()
    );
    let stored = std::fs::read(&file).unwrap();
    assert!(!contains(&stored, "confidencial".as_bytes()));
    assert!(!contains(&stored, b"SQLite format 3"));
    let leftovers: Vec<_> = std::fs::read_dir(&env.out)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| !name.ends_with(".age") && !name.ends_with(".age.meta.json"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");

    let without_key = env.run(&["verify", file.to_str().unwrap()]);
    let wrong_key = env.run(&[
        "verify",
        file.to_str().unwrap(),
        "--identity",
        other_key.to_str().unwrap(),
    ]);
    let refused = env.run(&[
        "restore",
        file.to_str().unwrap(),
        "--to",
        env.path("refused.db").to_str().unwrap(),
        "--identity",
        other_key.to_str().unwrap(),
    ]);
    assert_eq!(without_key.status.code(), Some(2), "{}", text(&without_key));
    assert!(text(&without_key).contains("--identity"));
    assert_eq!(wrong_key.status.code(), Some(10), "{}", text(&wrong_key));
    assert_eq!(refused.status.code(), Some(10), "{}", text(&refused));
    assert!(!env.path("refused.db").exists());

    let verify = env.run(&[
        "--json",
        "--log-level",
        "trace",
        "verify",
        file.to_str().unwrap(),
        "--identity",
        recovery_key.to_str().unwrap(),
    ]);
    assert_eq!(verify.status.code(), Some(0), "{}", text(&verify));
    let verified = json(&verify);
    assert_eq!(verified["checksum_verified"], true);
    assert_eq!(verified["manifest_version"], 3);
    assert_eq!(verified["sha256"], result["sha256"]);
    assert_eq!(verified["tables"], 1);

    let copy = env.path("copy.db");
    let restore = env.run(&[
        "--json",
        "--log-level",
        "trace",
        "restore",
        file.to_str().unwrap(),
        "--to",
        copy.to_str().unwrap(),
        "-i",
        main_key.to_str().unwrap(),
    ]);
    assert_eq!(restore.status.code(), Some(0), "{}", text(&restore));
    assert_eq!(notes(&copy), expected);
    assert!(env.work_entries().is_empty(), "{:?}", env.work_entries());

    let restores = env.run(&["--json", "restore", "list"]);
    assert!(
        text(&restores).contains(file.to_str().unwrap()),
        "{}",
        text(&restores)
    );

    let manifest = std::fs::read(format!("{}.meta.json", file.display())).unwrap();
    let history = std::fs::read(env.data.join("history.sqlite3")).unwrap();
    for (name, bytes) in [
        ("backup", [backup.stdout, backup.stderr].concat()),
        ("verify", [verify.stdout, verify.stderr].concat()),
        ("restore", [restore.stdout, restore.stderr].concat()),
        ("manifesto", manifest),
        ("histórico", history),
    ] {
        assert!(
            !contains(&bytes, SECRET_PREFIX.as_bytes()),
            "chave privada em {name}"
        );
    }
}

#[test]
fn tampered_or_manifestless_encrypted_backup_is_refused_before_decrypting() {
    let env = Env::new();
    let source = env.path("app.db");
    source_db(&source);
    let (key, public) = env.key("main.key");
    let backup = env.backup(&source, &["--recipient", &public]);
    assert_eq!(backup.status.code(), Some(0), "{}", text(&backup));
    let file = PathBuf::from(json(&backup)["file"].as_str().unwrap());
    assert!(file.to_string_lossy().ends_with(".sqlite3.age"));

    let mut bytes = std::fs::read(&file).unwrap();
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0x01;
    std::fs::write(&file, &bytes).unwrap();
    let tampered = env.run(&[
        "verify",
        file.to_str().unwrap(),
        "--identity",
        key.to_str().unwrap(),
    ]);

    assert_eq!(tampered.status.code(), Some(10), "{}", text(&tampered));
    assert!(
        text(&tampered).contains("checksum divergente"),
        "{}",
        text(&tampered)
    );
    assert!(env.work_entries().is_empty(), "{:?}", env.work_entries());

    std::fs::remove_file(format!("{}.meta.json", file.display())).unwrap();
    let manifestless = env.run(&[
        "verify",
        file.to_str().unwrap(),
        "--identity",
        key.to_str().unwrap(),
        "--allow-unverified",
    ]);

    assert_eq!(
        manifestless.status.code(),
        Some(10),
        "{}",
        text(&manifestless)
    );
    assert!(
        text(&manifestless).contains("age -d"),
        "{}",
        text(&manifestless)
    );
}

#[test]
fn compressed_backup_without_encryption_round_trips_without_a_key() {
    let env = Env::new();
    let source = env.path("app.db");
    let expected = source_db(&source);

    let backup = env.backup(&source, &["--compress"]);
    assert_eq!(backup.status.code(), Some(0), "{}", text(&backup));
    let result = json(&backup);
    assert_eq!(result["compression"], "zstd");
    assert!(result.get("encryption").is_none());
    let file = PathBuf::from(result["file"].as_str().unwrap());
    assert!(file.to_string_lossy().ends_with(".sqlite3.zst"));
    let content_size = std::fs::metadata(&source).unwrap().len();
    assert!(
        result["size_bytes"].as_u64().unwrap() < content_size / 2,
        "{result}"
    );

    let copy = env.path("copy.db");
    let restore = env.run(&[
        "restore",
        file.to_str().unwrap(),
        "--to",
        copy.to_str().unwrap(),
    ]);
    assert_eq!(restore.status.code(), Some(0), "{}", text(&restore));
    assert_eq!(notes(&copy), expected);
}

#[test]
fn invalid_recipient_or_shared_identity_is_refused_before_any_work() {
    let env = Env::new();
    let source = env.path("app.db");
    source_db(&source);

    let bad = env.backup(&source, &["--recipient", "age1naoeumachave"]);

    assert_eq!(bad.status.code(), Some(2), "{}", text(&bad));
    assert!(!env.out.exists() || std::fs::read_dir(&env.out).unwrap().next().is_none());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let (key, public) = env.key("main.key");
        let backup = env.backup(&source, &["--recipient", &public]);
        let file = json(&backup)["file"].as_str().unwrap().to_owned();
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();

        let shared = env.run(&["verify", &file, "--identity", key.to_str().unwrap()]);

        assert_eq!(shared.status.code(), Some(2), "{}", text(&shared));
        assert!(text(&shared).contains("chmod 600"), "{}", text(&shared));
    }
}

#[tokio::test]
async fn postgres_backup_is_encrypted_without_a_second_compression_and_restores() -> TestResult {
    let db = PostgresTestDb::start_seeded().await?;
    let ws = Workspace::new()?;
    let env = Env::new();
    let (key, public) = env.key("main.key");

    let backup = ws
        .command(&db, db.user(), db.password())
        .args(["--json", "--compress", "--recipient", &public])
        .output()?;
    assert_eq!(backup.status.code(), Some(0), "{}", text(&backup));
    let result: serde_json::Value = serde_json::from_slice(&backup.stdout)?;
    assert_eq!(result["encryption"], "age");
    assert!(result.get("compression").is_none(), "{result}");
    let file = PathBuf::from(result["file"].as_str().unwrap_or_default());
    assert!(
        file.to_string_lossy().ends_with(".dump.age"),
        "{}",
        file.display()
    );
    assert!(!std::fs::read(&file)?.starts_with(b"PGDMP"));

    let restore = ws
        .restore(&db, &file, "cifrado")
        .arg("--identity")
        .arg(&key)
        .output()?;
    assert_eq!(restore.status.code(), Some(0), "{}", text(&restore));

    let (client, connection) = tokio_postgres::Config::new()
        .host(db.host())
        .port(db.port())
        .user(db.user())
        .password(db.password())
        .dbname("cifrado")
        .connect(tokio_postgres::NoTls)
        .await?;
    tokio::spawn(connection);
    let orders: i64 = client
        .query_one("SELECT COUNT(*) FROM orders", &[])
        .await?
        .get(0);
    assert_eq!(orders, 3);
    Ok(())
}
