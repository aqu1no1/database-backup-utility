#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../../../crates/database-adapters/tests/support/mod.rs"]
mod support;

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use database_backup_adapters::mongo::{MongoTools, MongoVersion, RunnerChoice};
use mongodb::bson::doc;
use support::{MongoTestDb, TestResult};
use tempfile::TempDir;

const INT32_ID_2: [u8; 9] = [0x10, b'_', b'i', b'd', 0x00, 0x02, 0x00, 0x00, 0x00];

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

    fn command(&self, password: &str, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_database-backup"));
        command
            .args(args)
            .env("DBU_DATA_DIR", &self.data)
            .env("TMPDIR", &self.tmp)
            .env_remove("RUST_LOG")
            .env_remove("DBU_PG_PASSWORD")
            .env_remove("DBU_PG_PASSWORD_FILE")
            .env_remove("DBU_MYSQL_PASSWORD")
            .env_remove("DBU_MYSQL_PASSWORD_FILE")
            .env_remove("DBU_MONGO_PASSWORD_FILE")
            .env("DBU_MONGO_PASSWORD", password)
            .stdin(Stdio::null());
        command
    }

    fn run(&self, db: &MongoTestDb, args: &[&str]) -> Output {
        self.run_with_password(db.password(), args)
    }

    fn run_with_password(&self, password: &str, args: &[&str]) -> Output {
        self.command(password, args).output().unwrap()
    }

    fn test_connection(&self, db: &MongoTestDb, database: &str, extra: &[&str]) -> Output {
        let port = db.port().to_string();
        let mut args = vec![
            "--json",
            "connection",
            "test",
            "--engine",
            "mongodb",
            "--host",
            db.host(),
            "--port",
            &port,
            "--database",
            database,
            "--user",
            db.user(),
        ];
        args.extend_from_slice(extra);
        self.run(db, &args)
    }

    fn backup(&self, db: &MongoTestDb, runner: &str) -> Output {
        self.backup_with(db, runner, &[])
    }

    fn backup_with(&self, db: &MongoTestDb, runner: &str, extra: &[&str]) -> Output {
        let port = db.port().to_string();
        let mut args = vec![
            "--json",
            "backup",
            "run",
            "--engine",
            "mongodb",
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
        ];
        args.extend_from_slice(extra);
        self.run(db, &args)
    }

    fn restore(&self, db: &MongoTestDb, file: &Path, target: &str, extra: &[&str]) -> Output {
        let port = db.port().to_string();
        let mut args = vec![
            "--json",
            "restore",
            "run",
            "--engine",
            "mongodb",
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

    fn verify(&self, db: &MongoTestDb, file: &Path, extra: &[&str]) -> Output {
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

fn archive_files(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().ends_with(".archive"))
                .count()
        })
        .unwrap_or(0)
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn replace_bytes(content: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    assert_eq!(from.len(), to.len());
    let mut changed = content.to_vec();
    let mut replaced = 0;
    let mut index = 0;
    while index + from.len() <= changed.len() {
        if &changed[index..index + from.len()] == from {
            changed[index..index + from.len()].copy_from_slice(to);
            index += from.len();
            replaced += 1;
        } else {
            index += 1;
        }
    }
    assert!(replaced > 0, "{from:?} não encontrado");
    changed
}

async fn native_available(db: &MongoTestDb) -> TestResult<bool> {
    let server = MongoVersion::parse(db.tag()).ok_or("tag do MongoDB inválida")?;
    let native = MongoTools::detect(RunnerChoice::Native, server).await;
    if let Err(error) = &native {
        assert!(
            std::env::var_os("DBU_REQUIRE_NATIVE_MONGO").is_none(),
            "ferramentas MongoDB nativas exigidas: {error}"
        );
        eprintln!("runner nativo ignorado: {error}");
    }
    Ok(native.is_ok())
}

async fn round_trip(db: &MongoTestDb, topology: &str, runners: Vec<&str>) -> TestResult {
    let expected = db.snapshot(db.database()).await?;
    for runner in runners {
        let label = format!("{topology}/{runner}");
        let env = Env::new();

        let test = env.test_connection(db, db.database(), &[]);
        assert_code(&test, 0);
        let tested = json(&test);
        assert_eq!(tested["topology"], topology, "{label}");
        assert!(
            tested["server_version"]
                .as_str()
                .is_some_and(|version| version.starts_with(&format!("{}.", db.tag()))),
            "{label}: {tested}"
        );
        let human = env
            .command(
                db.password(),
                &[
                    "connection",
                    "test",
                    "--engine",
                    "mongodb",
                    "--host",
                    db.host(),
                    "--port",
                    &db.port().to_string(),
                    "--database",
                    db.database(),
                    "--user",
                    db.user(),
                ],
            )
            .output()?;
        assert_code(&human, 0);
        assert!(
            text(&human).contains(&format!("Conexão OK: MongoDB {}.", db.tag())),
            "{}",
            text(&human)
        );

        let backup = env.backup_with(db, runner, &["--log-level", "debug"]);
        assert_code(&backup, 0);
        let result = json(&backup);
        assert_eq!(result["topology"], topology, "{label}");
        let file = PathBuf::from(result["file"].as_str().unwrap());
        assert_eq!(file.extension().and_then(|e| e.to_str()), Some("archive"));
        let meta_path = PathBuf::from(format!("{}.meta.json", file.display()));
        let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(&meta_path)?)?;
        assert_eq!(manifest["engine"], "mongodb", "{label}");
        assert_eq!(manifest["format"], "mongodump-archive");
        assert_eq!(manifest["runner"], runner);
        assert_eq!(manifest["topology"], topology, "{label}");
        assert_eq!(manifest["database"], db.database());
        assert!(
            manifest["mongodump_version"]
                .as_str()
                .is_some_and(|version| version.starts_with("100.")),
            "{manifest}"
        );
        assert!(manifest.get("pg_dump_version").is_none());
        assert!(manifest.get("mysqldump_version").is_none());
        assert!(manifest.get("non_transactional_tables").is_none());
        assert!(
            text(&backup).contains("snapshot de um único instante"),
            "{}",
            text(&backup)
        );

        let verify = env.verify(db, &file, &[]);
        assert_code(&verify, 0);
        let verified = json(&verify);
        assert_eq!(verified["engine"], "mongodb");
        assert_eq!(verified["checksum_verified"], true);
        assert_eq!(verified["tables"], 4);
        assert_eq!(verified["objects"], 5);
        assert_eq!(verified["database"], db.database());
        assert!(
            verified["dumped_from"]
                .as_str()
                .is_some_and(|version| version.starts_with(&format!("{}.", db.tag())))
        );

        let target = format!("restaurado_{runner}");
        let restore = env.restore(db, &file, &target, &[]);
        assert_code(&restore, 0);
        assert_eq!(json(&restore)["mode"], "create");
        assert_eq!(db.snapshot(&target).await?, expected, "{label}");
        assert_eq!(db.snapshot(db.database()).await?, expected, "{label}");

        let history = rusqlite::Connection::open(env.data.join("history.sqlite3"))?;
        let engines: (String, String) = history.query_row(
            "SELECT (SELECT engine FROM connections), (SELECT engine FROM restore_executions)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!(engines, ("mongodb".to_owned(), "mongodb".to_owned()));
        drop(history);

        let password = db.password().as_bytes();
        for output in [&test, &human, &backup, &verify, &restore] {
            assert!(!contains_bytes(&output.stdout, password), "{label}");
            assert!(!contains_bytes(&output.stderr, password), "{label}");
        }
        assert!(!contains_bytes(&std::fs::read(&meta_path)?, password));
        assert!(!contains_bytes(
            &std::fs::read(env.data.join("history.sqlite3"))?,
            password
        ));
        assert_eq!(
            std::fs::read_dir(&env.tmp)?.count(),
            0,
            "credenciais temporárias ficaram ({label})"
        );
    }
    Ok(())
}

#[tokio::test]
async fn backup_verify_and_restore_round_trip_preserves_data_on_each_topology() -> TestResult {
    let (standalone, replica_set) = tokio::try_join!(
        MongoTestDb::start_seeded(),
        MongoTestDb::start_replica_set_seeded()
    )?;
    let mut runners = vec!["docker"];
    if native_available(&standalone).await? {
        runners.push("native");
    }

    round_trip(&standalone, "standalone", runners).await?;
    round_trip(&replica_set, "replica_set", vec!["docker"]).await
}

#[tokio::test]
async fn existing_database_and_overwrite_are_refused_without_changes() -> TestResult {
    let db = MongoTestDb::start_seeded().await?;
    let env = Env::new();
    let backup = env.backup(&db, "docker");
    assert_code(&backup, 0);
    let file = PathBuf::from(json(&backup)["file"].as_str().unwrap());
    let client = db.client().await?;
    client
        .database(db.database())
        .collection::<mongodb::bson::Document>("customers")
        .insert_one(doc! { "_id": 99, "name": "Nova" })
        .await?;
    client
        .database("vazio")
        .create_collection("sem_dados")
        .await?;
    let before = db.snapshot(db.database()).await?;

    let existing = env.restore(&db, &file, db.database(), &[]);
    let empty = env.restore(&db, &file, "vazio", &[]);
    let overwrite = env.restore(
        &db,
        &file,
        db.database(),
        &["--overwrite", "--confirm", db.database()],
    );
    let reserved = env.restore(&db, &file, "admin", &[]);
    let invalid = env.restore(&db, &file, "a.b", &[]);

    assert_code(&existing, 2);
    assert!(text(&existing).contains("já existe"), "{}", text(&existing));
    assert_code(&empty, 2);
    assert!(text(&empty).contains("já existe"), "{}", text(&empty));
    assert_code(&overwrite, 2);
    assert!(
        text(&overwrite).contains("restore sobrescrevendo destino existente não está disponível"),
        "{}",
        text(&overwrite)
    );
    assert_code(&reserved, 2);
    assert!(
        text(&reserved).contains("interno do MongoDB"),
        "{}",
        text(&reserved)
    );
    assert_code(&invalid, 2);
    assert_eq!(db.snapshot(db.database()).await?, before);
    assert_eq!(db.snapshot("vazio").await?.collections.len(), 1);

    let wrong = env.run_with_password(
        "senha-errada",
        &[
            "connection",
            "test",
            "--engine",
            "mongodb",
            "--host",
            db.host(),
            "--port",
            &db.port().to_string(),
            "--database",
            db.database(),
            "--user",
            db.user(),
        ],
    );
    let wrong_auth_database =
        env.test_connection(&db, db.database(), &["--auth-database", "outro_banco"]);
    let missing = env.test_connection(&db, "nao_existe", &[]);
    let tls = env.test_connection(
        &db,
        db.database(),
        &["--tls", "verify-full", "--timeout", "2"],
    );
    assert_code(&wrong, 3);
    assert_code(&wrong_auth_database, 3);
    assert_code(&missing, 4);
    assert_code(&tls, 6);
    assert!(!text(&wrong).contains("senha-errada"));
    assert!(!text(&tls).contains(db.password()));
    Ok(())
}

#[tokio::test]
async fn damaged_archives_are_rejected_and_failed_restore_leaves_no_database() -> TestResult {
    let db = MongoTestDb::start_seeded().await?;
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
    assert!(!db.database_exists("adulterado").await?);
    std::fs::write(&file, &original)?;

    let truncated = env.path("cortado.archive");
    write_private(&truncated, &original[..original.len() * 2 / 3]);
    let verify = env.verify(&db, &truncated, &["--allow-unverified"]);
    assert_code(&verify, 10);
    assert!(text(&verify).contains("incompleto"), "{}", text(&verify));
    assert_code(
        &env.restore(&db, &truncated, "cortado", &["--allow-unverified"]),
        10,
    );
    assert!(!db.database_exists("cortado").await?);

    let foreign = env.path("estranho.archive");
    write_private(&foreign, b"PGDMP\x01\x02\x03 isto nao e um archive");
    let verify = env.verify(&db, &foreign, &["--allow-unverified"]);
    assert_code(&verify, 10);
    assert_code(
        &env.restore(&db, &foreign, "estranho", &["--allow-unverified"]),
        10,
    );

    let duplicate = env.path("duplicado.archive");
    let mut same_id = INT32_ID_2;
    same_id[5] = 0x01;
    write_private(&duplicate, &replace_bytes(&original, &INT32_ID_2, &same_id));
    assert_code(&env.verify(&db, &duplicate, &["--allow-unverified"]), 0);
    let failed = env.restore(&db, &duplicate, "duplicado", &["--allow-unverified"]);
    assert_code(&failed, 11);
    assert!(text(&failed).contains("foi apagado"), "{}", text(&failed));
    assert!(!db.database_exists("duplicado").await?);

    let altered = env.path("alterado.archive");
    write_private(
        &altered,
        &replace_bytes(&original, b"Ana Souza", b"Ana Souzb"),
    );
    assert_code(&env.verify(&db, &altered, &["--allow-unverified"]), 0);
    let failed = env.restore(&db, &altered, "alterado", &["--allow-unverified"]);
    assert_code(&failed, 11);
    assert!(text(&failed).contains("foi apagado"), "{}", text(&failed));
    assert!(!db.database_exists("alterado").await?);

    let hostile = env.path("hostil.archive");
    write_private(
        &hostile,
        &replace_bytes(&original, db.database().as_bytes(), b"ba*kup_test"),
    );
    let refused = env.restore(&db, &hostile, "hostil", &["--allow-unverified"]);
    assert_code(&refused, 10);
    assert!(text(&refused).contains("inválido"), "{}", text(&refused));
    assert!(!db.database_exists("hostil").await?);
    assert!(!db.database_exists("ba*kup_test").await?);

    assert_eq!(db.snapshot(db.database()).await?.collections.len(), 4);

    db.client()
        .await?
        .database(db.database())
        .run_command(doc! { "create": "leituras", "timeseries": { "timeField": "ts" } })
        .await?;
    let published = archive_files(&env.out);
    let unsupported = env.backup(&db, "docker");
    assert_code(&unsupported, 8);
    assert!(
        text(&unsupported).contains("time series"),
        "{}",
        text(&unsupported)
    );
    assert_eq!(archive_files(&env.out), published);
    Ok(())
}

#[tokio::test]
async fn missing_tools_fail_with_code_7_and_a_hint() -> TestResult {
    let db = MongoTestDb::start_seeded().await?;
    let env = Env::new();
    let port = db.port().to_string();

    let output = env
        .command(
            db.password(),
            &[
                "backup",
                "run",
                "--engine",
                "mongodb",
                "--runner",
                "native",
                "--host",
                db.host(),
                "--port",
                &port,
                "--database",
                db.database(),
                "--user",
                db.user(),
                "--output-dir",
                env.out.to_str().unwrap(),
            ],
        )
        .env("PATH", "")
        .output()?;

    assert_code(&output, 7);
    let message = text(&output);
    assert!(message.contains("mongodump"), "{message}");
    assert!(message.contains("--runner docker"), "{message}");
    assert!(!message.contains(db.password()));
    assert!(
        !env.out.exists() || std::fs::read_dir(&env.out)?.count() == 0,
        "um backup parcial ficou"
    );
    Ok(())
}
