use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use crate::support::{MysqlTestDb, TestResult};
use database_backup_adapters::mysql::{Flavor, MysqlTools, MysqlVersion, RunnerChoice};
use tempfile::TempDir;

const SNAPSHOT: [&str; 4] = [
    "SELECT id, name, email FROM customers ORDER BY id",
    "SELECT id, customer_id, total, HEX(payload) FROM orders ORDER BY id",
    "SELECT order_id, note FROM audit_log ORDER BY id",
    "SELECT id, name, total FROM customer_totals ORDER BY id",
];

struct Env {
    _dir: TempDir,
    data: PathBuf,
    out: PathBuf,
    tmp: PathBuf,
}

impl Env {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let data = dir.path().join("data");
        let out = dir.path().join("out");
        let tmp = dir.path().join("tmp");
        std::fs::create_dir(&tmp).unwrap();
        Self {
            _dir: dir,
            data,
            out,
            tmp,
        }
    }

    fn run(&self, db: &MysqlTestDb, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_dbu"))
            .args(args)
            .env("DBU_DATA_DIR", &self.data)
            .env("TMPDIR", &self.tmp)
            .env_remove("RUST_LOG")
            .env_remove("DBU_PG_PASSWORD")
            .env_remove("DBU_PG_PASSWORD_FILE")
            .env_remove("DBU_MYSQL_PASSWORD_FILE")
            .env_remove("DBU_MARIADB_PASSWORD_FILE")
            .env("DBU_MYSQL_PASSWORD", db.password())
            .env("DBU_MARIADB_PASSWORD", db.password())
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn connection_args<'a>(db: &'a MysqlTestDb, port: &'a str) -> [&'a str; 6] {
        ["--host", db.host(), "--port", port, "--user", db.user()]
    }
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn json(output: &Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!("saída não é JSON ({error}): {}", text(output));
    })
}

fn assert_code(output: &Output, code: i32) {
    assert_eq!(output.status.code(), Some(code), "{}", text(output));
}

async fn snapshot(db: &MysqlTestDb, database: &str) -> TestResult<Vec<Vec<Vec<String>>>> {
    let mut tables = Vec::new();
    for query in SNAPSHOT {
        tables.push(db.rows(database, query).await?);
    }
    Ok(tables)
}

async fn database_exists(db: &MysqlTestDb, name: &str) -> TestResult<bool> {
    let rows = db
        .rows(
            "information_schema",
            &format!("SELECT COUNT(*) FROM SCHEMATA WHERE SCHEMA_NAME = '{name}'"),
        )
        .await?;
    Ok(rows == vec![vec!["1".to_owned()]])
}

async fn native_available(db: &MysqlTestDb) -> TestResult<bool> {
    let version = db.rows(db.database(), "SELECT VERSION()").await?;
    let server = MysqlVersion::parse_as(Flavor::Mariadb, &version[0][0])
        .ok_or("versão do servidor MariaDB inválida")?;
    let native = MysqlTools::detect_flavor(Flavor::Mariadb, RunnerChoice::Native, server).await;
    if let Err(error) = &native {
        assert!(
            std::env::var_os("DBU_REQUIRE_NATIVE_MARIADB").is_none(),
            "cliente MariaDB nativo exigido: {error}"
        );
        eprintln!("runner nativo ignorado: {error}");
    }
    Ok(native.is_ok())
}

#[tokio::test]
async fn backup_verify_and_restore_round_trip_preserves_data() -> TestResult {
    let db = MysqlTestDb::start_mariadb_seeded().await?;
    let expected = snapshot(&db, db.database()).await?;
    let port = db.port().to_string();
    let mut runners = vec!["docker"];
    if native_available(&db).await? {
        runners.push("native");
    }

    for runner in runners {
        let env = Env::new();
        let connection = Env::connection_args(&db, &port);
        let mut args = vec!["connection", "test", "-e", "mariadb", "-d", db.database()];
        args.extend(connection);
        let test = env.run(&db, &args);
        assert_code(&test, 0);
        assert!(
            text(&test).contains(&format!("Conexão OK: MariaDB {}", db.tag())),
            "{}",
            text(&test)
        );

        let out = env.out.to_str().unwrap();
        let mut args = vec![
            "--json",
            "backup",
            "run",
            "-e",
            "mariadb",
            "--runner",
            runner,
            "-d",
            db.database(),
            "-o",
            out,
        ];
        args.extend(connection);
        let backup = env.run(&db, &args);
        assert_code(&backup, 0);
        let file = PathBuf::from(json(&backup)["file"].as_str().unwrap());
        let meta_path = PathBuf::from(format!("{}.meta.json", file.display()));
        let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(&meta_path)?)?;
        assert_eq!(manifest["engine"], "mariadb", "{runner}");
        assert_eq!(manifest["format"], "mariadb-dump-sql");
        assert_eq!(manifest["runner"], runner);
        let dump_major = manifest["mysqldump_version"]
            .as_str()
            .and_then(|version| version.split('.').next())
            .and_then(|major| major.parse::<u32>().ok());
        assert!(dump_major.is_some_and(|major| major >= 10), "{manifest}");

        let verify = env.run(&db, &["--json", "backup", "verify", file.to_str().unwrap()]);
        assert_code(&verify, 0);
        let verified = json(&verify);
        assert_eq!(verified["engine"], "mariadb");
        assert_eq!(verified["checksum_verified"], true);
        assert_eq!(verified["tables"], 3);

        let target = format!("restaurado_{runner}");
        let mut args = vec![
            "--json",
            "restore",
            file.to_str().unwrap(),
            "--to",
            &target,
            "--runner",
            runner,
        ];
        args.extend(connection);
        let restore = env.run(&db, &args);
        assert_code(&restore, 0);
        assert_eq!(snapshot(&db, &target).await?, expected, "{runner}");

        let history = rusqlite::Connection::open(env.data.join("history.sqlite3"))?;
        let engines: (String, String) = history.query_row(
            "SELECT (SELECT engine FROM connections), (SELECT engine FROM restore_executions)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!(engines, ("mariadb".to_owned(), "mariadb".to_owned()));

        for output in [&test, &backup, &verify, &restore] {
            assert!(!text(output).contains(db.password()), "{runner}");
        }
        assert!(!std::fs::read_to_string(&meta_path)?.contains(db.password()));
        assert_eq!(
            std::fs::read_dir(&env.tmp)?.count(),
            0,
            "credenciais temporárias ficaram"
        );
    }
    Ok(())
}

#[tokio::test]
async fn mysql_and_mariadb_are_never_confused() -> TestResult {
    let db = MysqlTestDb::start_mariadb_seeded().await?;
    let env = Env::new();
    let port = db.port().to_string();
    let connection = Env::connection_args(&db, &port);

    let mut args = vec!["connection", "test", "-e", "mysql", "-d", db.database()];
    args.extend(connection);
    let wrong_test = env.run(&db, &args);
    assert_code(&wrong_test, 2);
    assert!(
        text(&wrong_test).contains("--engine mariadb"),
        "{}",
        text(&wrong_test)
    );

    let out = env.out.to_str().unwrap();
    let mut args = vec![
        "--json",
        "backup",
        "run",
        "-e",
        "mysql",
        "-d",
        db.database(),
        "-o",
        out,
    ];
    args.extend(connection);
    let wrong_backup = env.run(&db, &args);
    assert_code(&wrong_backup, 2);
    assert!(!env.out.exists() || std::fs::read_dir(&env.out)?.count() == 0);

    let mut args = vec![
        "--json",
        "backup",
        "run",
        "-e",
        "mariadb",
        "--runner",
        "docker",
        "-d",
        db.database(),
        "-o",
        out,
    ];
    args.extend(connection);
    let backup = env.run(&db, &args);
    assert_code(&backup, 0);
    let file = PathBuf::from(json(&backup)["file"].as_str().unwrap());
    let bare = env.out.join("sem-manifesto.sql");
    std::fs::copy(&file, &bare)?;

    for dump in [&file, &bare] {
        let mut args = vec![
            "--json",
            "restore",
            dump.to_str().unwrap(),
            "-e",
            "mysql",
            "--to",
            "nunca_criado",
            "--allow-unverified",
        ];
        args.extend(connection);
        let refused = env.run(&db, &args);
        assert_code(&refused, 10);
        assert!(text(&refused).contains("MariaDB"), "{}", text(&refused));
    }
    assert!(!database_exists(&db, "nunca_criado").await?);
    Ok(())
}

const MARIADB_DUMP: &str = "/*M!999999\\- enable the sandbox mode */ \n-- MariaDB dump 10.19-11.4.2-MariaDB, for debian-linux-gnu (x86_64)\n--\n-- Host: h    Database: app\nCREATE TABLE `t` (\n  `id` int NOT NULL\n);\n-- Dump completed on 2026-11-08\n";

fn offline_dbu(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_dbu"))
        .args(args)
        .env("DBU_DATA_DIR", dir.join("data"))
        .env_remove("RUST_LOG")
        .env_remove("DBU_MYSQL_PASSWORD")
        .env_remove("DBU_MYSQL_PASSWORD_FILE")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn private_dump(dir: &Path) -> PathBuf {
    let file = dir.join("maria.sql");
    std::fs::write(&file, MARIADB_DUMP).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    file
}

#[test]
fn mariadb_dump_without_manifest_is_refused_by_a_mysql_restore() {
    let env = TempDir::new().unwrap();
    let file = private_dump(env.path());
    let output = offline_dbu(
        env.path(),
        &[
            "restore",
            file.to_str().unwrap(),
            "-e",
            "mysql",
            "--to",
            "x",
            "-u",
            "root",
            "--allow-unverified",
        ],
    );

    assert_code(&output, 10);
    assert!(
        text(&output).contains("backup de MariaDB, não de MySQL"),
        "{}",
        text(&output)
    );
}

#[test]
fn mariadb_dump_is_recognized_from_its_header_without_a_manifest() {
    let env = TempDir::new().unwrap();
    let file = private_dump(env.path());
    let output = offline_dbu(
        env.path(),
        &[
            "--json",
            "backup",
            "verify",
            file.to_str().unwrap(),
            "--allow-unverified",
        ],
    );

    assert_code(&output, 0);
    let verified = json(&output);
    assert_eq!(verified["engine"], "mariadb");
    assert_eq!(verified["tables"], 1);
    assert_eq!(verified["database"], "app");
}
