#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../../../crates/database-adapters/tests/support/mod.rs"]
mod support;

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use database_backup_adapters::mysql::{MysqlTools, MysqlVersion, RunnerChoice};
use support::{MysqlTestDb, TestResult};
use tempfile::TempDir;

const SNAPSHOT: [&str; 4] = [
    "SELECT id, name, email FROM customers ORDER BY id",
    "SELECT id, customer_id, total, HEX(payload) FROM orders ORDER BY id",
    "SELECT order_id, note FROM audit_log ORDER BY id",
    "SELECT id, name, total FROM customer_totals ORDER BY id",
];

struct Env {
    dir: TempDir,
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
            dir,
            data,
            out,
            tmp,
        }
    }

    fn run(&self, db: &MysqlTestDb, args: &[&str]) -> Output {
        self.run_with_password(db.password(), args)
    }

    fn run_with_password(&self, password: &str, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_dbu"))
            .args(args)
            .env("DBU_DATA_DIR", &self.data)
            .env("TMPDIR", &self.tmp)
            .env_remove("RUST_LOG")
            .env_remove("DBU_PG_PASSWORD")
            .env_remove("DBU_PG_PASSWORD_FILE")
            .env_remove("DBU_MYSQL_PASSWORD_FILE")
            .env("DBU_MYSQL_PASSWORD", password)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn backup(&self, db: &MysqlTestDb, runner: &str) -> Output {
        let port = db.port().to_string();
        self.run(
            db,
            &[
                "--json",
                "backup",
                "run",
                "--engine",
                "mysql",
                "--runner",
                runner,
                "--host",
                db.host(),
                "--port",
                &port,
                "--database",
                db.database(),
                "--user",
                db.user(),
                "--output-dir",
                self.out.to_str().unwrap(),
            ],
        )
    }

    fn restore(&self, db: &MysqlTestDb, file: &Path, target: &str, extra: &[&str]) -> Output {
        let port = db.port().to_string();
        let mut args = vec![
            "--json",
            "restore",
            "run",
            "--engine",
            "mysql",
            "--runner",
            "docker",
            "--host",
            db.host(),
            "--port",
            &port,
            "--user",
            db.user(),
            "--target-database",
            target,
            "--file",
            file.to_str().unwrap(),
        ];
        args.extend_from_slice(extra);
        self.run(db, &args)
    }

    fn verify(&self, db: &MysqlTestDb, file: &Path, extra: &[&str]) -> Output {
        let mut args = vec![
            "--json",
            "backup",
            "verify",
            "--file",
            file.to_str().unwrap(),
        ];
        args.extend_from_slice(extra);
        self.run(db, &args)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }
}

fn write_private(path: &Path, content: &[u8]) {
    std::fs::write(path, content).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
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
    let server = MysqlVersion::parse(&version[0][0]).ok_or("versão do servidor inválida")?;
    let native = MysqlTools::detect(RunnerChoice::Native, server).await;
    if let Err(error) = &native {
        assert!(
            std::env::var_os("DBU_REQUIRE_NATIVE_MYSQL").is_none(),
            "cliente MySQL nativo exigido: {error}"
        );
        eprintln!("runner nativo ignorado: {error}");
    }
    Ok(native.is_ok())
}

#[tokio::test]
async fn backup_verify_and_restore_round_trip_preserves_data() -> TestResult {
    let db = MysqlTestDb::start_seeded().await?;
    let expected = snapshot(&db, db.database()).await?;
    let mut runners = vec!["docker"];
    if native_available(&db).await? {
        runners.push("native");
    }

    for runner in runners {
        let env = Env::new();
        let port = db.port().to_string();
        let test = env.run(
            &db,
            &[
                "connection",
                "test",
                "--engine",
                "mysql",
                "--host",
                db.host(),
                "--port",
                &port,
                "--database",
                db.database(),
                "--user",
                db.user(),
            ],
        );
        assert_code(&test, 0);
        assert!(
            text(&test).contains("Conexão OK: MySQL 8.4"),
            "{}",
            text(&test)
        );

        let backup = env.backup(&db, runner);
        assert_code(&backup, 0);
        let file = PathBuf::from(json(&backup)["file"].as_str().unwrap());
        assert_eq!(file.extension().and_then(|e| e.to_str()), Some("sql"));
        let meta_path = PathBuf::from(format!("{}.meta.json", file.display()));
        let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(&meta_path)?)?;
        assert_eq!(manifest["engine"], "mysql", "{runner}");
        assert_eq!(manifest["format"], "mysqldump-sql");
        assert_eq!(manifest["runner"], runner);
        assert!(
            manifest["mysqldump_version"]
                .as_str()
                .is_some_and(|v| v.starts_with("8."))
        );
        assert!(manifest.get("pg_dump_version").is_none());
        assert_eq!(manifest["non_transactional_tables"], serde_json::json!([]));
        assert_eq!(
            json(&backup)["non_transactional_tables"],
            serde_json::json!([])
        );

        let verify = env.verify(&db, &file, &[]);
        assert_code(&verify, 0);
        let verified = json(&verify);
        assert_eq!(verified["engine"], "mysql");
        assert_eq!(verified["checksum_verified"], true);
        assert_eq!(verified["tables"], 3);
        assert_eq!(verified["database"], db.database());

        let target = format!("restaurado_{runner}");
        let restore = env.restore(&db, &file, &target, &[]);
        assert_code(&restore, 0);
        assert_eq!(json(&restore)["mode"], "create");
        assert_eq!(snapshot(&db, &target).await?, expected, "{runner}");

        let history = rusqlite::Connection::open(env.data.join("history.sqlite3"))?;
        let engines: (String, String) = history.query_row(
            "SELECT (SELECT engine FROM connections), (SELECT engine FROM restore_executions)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!(engines, ("mysql".to_owned(), "mysql".to_owned()));

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
async fn existing_database_and_overwrite_are_refused_without_changes() -> TestResult {
    let db = MysqlTestDb::start_seeded().await?;
    let env = Env::new();
    let backup = env.backup(&db, "docker");
    assert_code(&backup, 0);
    let file = PathBuf::from(json(&backup)["file"].as_str().unwrap());
    db.execute(
        db.database(),
        "INSERT INTO customers (name, email) VALUES ('Nova', 'nova@example.com')",
    )
    .await?;
    let before = snapshot(&db, db.database()).await?;
    db.execute("information_schema", "CREATE DATABASE vazio")
        .await?;

    let existing = env.restore(&db, &file, db.database(), &[]);
    let empty = env.restore(&db, &file, "vazio", &[]);
    let overwrite = env.restore(
        &db,
        &file,
        db.database(),
        &["--overwrite", "--confirm", db.database()],
    );

    assert_code(&existing, 2);
    assert!(text(&existing).contains("já existe"), "{}", text(&existing));
    assert_code(&empty, 2);
    assert_code(&overwrite, 2);
    assert!(
        text(&overwrite).contains("restore sobrescrevendo destino existente não está disponível"),
        "{}",
        text(&overwrite)
    );
    assert_eq!(snapshot(&db, db.database()).await?, before);
    assert_eq!(
        db.rows(
            "vazio",
            "SELECT COUNT(*) FROM information_schema.TABLES WHERE TABLE_SCHEMA = 'vazio'"
        )
        .await?,
        vec![vec!["0".to_owned()]]
    );

    let port = db.port().to_string();
    let wrong = env.run_with_password(
        "senha-errada",
        &[
            "connection",
            "test",
            "--engine",
            "mysql",
            "--host",
            db.host(),
            "--port",
            &port,
            "--database",
            db.database(),
            "--user",
            db.user(),
        ],
    );
    let missing = env.run(
        &db,
        &[
            "connection",
            "test",
            "--engine",
            "mysql",
            "--host",
            db.host(),
            "--port",
            &port,
            "--database",
            "nao_existe",
            "--user",
            db.user(),
        ],
    );
    assert_code(&wrong, 3);
    assert_code(&missing, 4);
    assert!(!text(&wrong).contains("senha-errada"));
    Ok(())
}

#[tokio::test]
async fn damaged_dumps_are_rejected_and_failed_restore_leaves_no_database() -> TestResult {
    let db = MysqlTestDb::start_seeded().await?;
    let env = Env::new();
    let backup = env.backup(&db, "docker");
    assert_code(&backup, 0);
    let file = PathBuf::from(json(&backup)["file"].as_str().unwrap());
    let original = std::fs::read(&file)?;

    let mut tampered = original.clone();
    let middle = tampered.len() / 2;
    tampered[middle] ^= 0x01;
    std::fs::write(&file, &tampered)?;
    assert_code(&env.verify(&db, &file, &[]), 10);
    assert_code(&env.restore(&db, &file, "adulterado", &[]), 10);
    assert!(!database_exists(&db, "adulterado").await?);
    std::fs::write(&file, &original)?;

    let truncated = env.path("cortado.sql");
    write_private(&truncated, &original[..original.len() * 2 / 3]);
    let verify = env.verify(&db, &truncated, &["--allow-unverified"]);
    assert_code(&verify, 10);
    assert!(text(&verify).contains("incompleto"), "{}", text(&verify));
    assert_code(
        &env.restore(&db, &truncated, "cortado", &["--allow-unverified"]),
        10,
    );
    assert!(!database_exists(&db, "cortado").await?);

    let broken = env.path("quebrado.sql");
    let source = String::from_utf8(original)?;
    let insert = source
        .find("INSERT INTO `orders`")
        .expect("insert de orders");
    write_private(
        &broken,
        format!(
            "{}INSERT INTO tabela_inexistente VALUES (1);\n{}",
            &source[..insert],
            &source[insert..]
        )
        .as_bytes(),
    );
    let failed = env.restore(&db, &broken, "falhou", &["--allow-unverified"]);
    assert_code(&failed, 11);
    assert!(text(&failed).contains("foi apagado"), "{}", text(&failed));
    assert!(!database_exists(&db, "falhou").await?);
    Ok(())
}

fn sql_files(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().ends_with(".sql"))
                .count()
        })
        .unwrap_or(0)
}

#[tokio::test]
async fn risky_mysql_objects_are_caught_before_they_cost_a_backup_or_a_restore() -> TestResult {
    let db = MysqlTestDb::start_seeded().await?;
    let env = Env::new();
    let port = db.port().to_string();

    db.execute(
        db.database(),
        "CREATE TABLE legacy (id INT PRIMARY KEY) ENGINE=MyISAM",
    )
    .await?;
    let backup = env.backup(&db, "docker");
    assert_code(&backup, 0);
    let result = json(&backup);
    assert_eq!(
        result["non_transactional_tables"],
        serde_json::json!(["legacy"])
    );
    let file = PathBuf::from(result["file"].as_str().unwrap());
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(format!("{}.meta.json", file.display()))?)?;
    assert_eq!(
        manifest["non_transactional_tables"],
        serde_json::json!(["legacy"])
    );

    db.execute(
        "mysql",
        "CREATE USER 'restaurador'@'%' IDENTIFIED BY 'senha-do-restaurador';
         GRANT CREATE, DROP ON *.* TO 'restaurador'@'%';
         GRANT ALL ON `alvo`.* TO 'restaurador'@'%'",
    )
    .await?;
    let refused = env.run_with_password(
        "senha-do-restaurador",
        &[
            "restore",
            "run",
            "--engine",
            "mysql",
            "--runner",
            "docker",
            "--host",
            db.host(),
            "--port",
            &port,
            "--user",
            "restaurador",
            "--target-database",
            "alvo",
            "--file",
            file.to_str().unwrap(),
        ],
    );
    assert_code(&refused, 2);
    assert!(text(&refused).contains("DEFINER"), "{}", text(&refused));
    assert!(
        text(&refused).contains("nenhum dado foi restaurado"),
        "{}",
        text(&refused)
    );
    assert!(!database_exists(&db, "alvo").await?);
    assert!(!text(&refused).contains("senha-do-restaurador"));

    db.execute(
        db.database(),
        "CREATE TRIGGER legacy_touch BEFORE INSERT ON legacy FOR EACH ROW SET NEW.id = NEW.id;
         CREATE TABLE depois (i INT)",
    )
    .await?;
    let before = sql_files(&env.out);
    let broken = env.backup(&db, "docker");
    assert_code(&broken, 8);
    assert!(text(&broken).contains("legacy_touch"), "{}", text(&broken));
    assert_eq!(
        sql_files(&env.out),
        before,
        "um backup que não restaura foi publicado"
    );
    Ok(())
}
