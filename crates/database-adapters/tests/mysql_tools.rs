#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;

use database_backup_adapters::mysql::{
    Admin, AdminError, DumpError, MysqlTools, MysqlVersion, ObjectKind, RunnerChoice, Timeouts,
    ToolError, ToolPaths, inspect_dump, test_connection,
};
use database_backup_core::{MysqlConnectionConfig, Password, TlsMode};
use support::{MysqlTestDb, TestResult};
use tokio::io::AsyncReadExt;

const SNAPSHOT: [&str; 4] = [
    "SELECT id, name, email FROM customers ORDER BY id",
    "SELECT id, customer_id, total, HEX(payload) FROM orders ORDER BY id",
    "SELECT order_id, note FROM audit_log ORDER BY id",
    "SELECT id, name, total FROM customer_totals ORDER BY id",
];

fn config_for(db: &MysqlTestDb, database: &str) -> MysqlConnectionConfig {
    MysqlConnectionConfig::new(db.host(), db.port(), database, db.user(), TlsMode::Auto)
        .expect("configuração de teste válida")
}

fn password(db: &MysqlTestDb) -> Password {
    Password::new(db.password().to_owned())
}

async fn server_version(db: &MysqlTestDb) -> TestResult<MysqlVersion> {
    let info = test_connection(
        &config_for(db, db.database()),
        &password(db),
        Timeouts::default(),
    )
    .await?;
    Ok(MysqlVersion::parse(&info.server_version).ok_or("versão do servidor inválida")?)
}

async fn snapshot(db: &MysqlTestDb, database: &str) -> TestResult<Vec<Vec<Vec<String>>>> {
    let mut tables = Vec::new();
    for query in SNAPSHOT {
        tables.push(db.rows(database, query).await?);
    }
    Ok(tables)
}

async fn dump_to_file(tools: &MysqlTools, db: &MysqlTestDb, path: &Path) -> TestResult {
    let mut process = tools.dump(&config_for(db, db.database()), &password(db))?;
    let mut stdout = process.take_stdout().ok_or("stdout ausente")?;
    let mut file = tokio::fs::File::create(path).await?;
    tokio::io::copy(&mut stdout, &mut file).await?;
    process.wait().await?;
    Ok(())
}

fn container_exists(name: &str) -> bool {
    let output = Command::new("docker")
        .args(["ps", "-aq", "--filter", &format!("name=^{name}$")])
        .output()
        .expect("docker ps");
    !output.stdout.is_empty()
}

async fn available_runners(server: MysqlVersion) -> Vec<MysqlTools> {
    let mut runners = Vec::new();
    for choice in [RunnerChoice::Docker, RunnerChoice::Native] {
        match MysqlTools::detect(choice, server).await {
            Ok(tools) => runners.push(tools),
            Err(error) if choice == RunnerChoice::Native => {
                assert!(
                    std::env::var_os("DBU_REQUIRE_NATIVE_MYSQL").is_none(),
                    "cliente MySQL nativo exigido: {error}"
                );
                eprintln!("runner nativo ignorado: {error}");
            }
            Err(error) => panic!("runner docker indisponível: {error}"),
        }
    }
    runners
}

#[tokio::test]
async fn dump_and_restore_preserve_schema_data_and_objects_on_every_runner() -> TestResult {
    let db = MysqlTestDb::start_seeded().await?;
    let expected = snapshot(&db, db.database()).await?;
    let admin = Admin::new(
        &config_for(&db, db.database()),
        &password(&db),
        Timeouts::default(),
    );
    let dir = tempfile::tempdir()?;

    for tools in available_runners(server_version(&db).await?).await {
        let runner = tools.kind().as_str();
        let file = dir.path().join(format!("{runner}.sql"));
        dump_to_file(&tools, &db, &file).await?;

        let info = inspect_dump(&file)?;
        assert_eq!(info.database.as_deref(), Some(db.database()), "{runner}");
        assert_eq!(
            (info.tables, info.views, info.triggers),
            (3, 1, 1),
            "{runner}"
        );

        let target = format!("copia_{runner}");
        admin.create_database(&target).await?;
        tools
            .restore(&config_for(&db, &target), &password(&db), &file)?
            .wait()
            .await?;

        assert_eq!(snapshot(&db, &target).await?, expected, "{runner}");
        assert_eq!(admin.count_tables(&target).await?, 3, "{runner}");
        db.execute(
            &target,
            "INSERT INTO orders (customer_id, total) VALUES (2, 5.00)",
        )
        .await?;
        let audit = db.rows(&target, "SELECT COUNT(*) FROM audit_log").await?;
        assert_eq!(
            audit,
            vec![vec!["5".to_owned()]],
            "o trigger não foi restaurado ({runner})"
        );
        assert_eq!(
            snapshot(&db, db.database()).await?,
            expected,
            "a origem mudou"
        );
    }
    Ok(())
}

#[tokio::test]
async fn password_never_reaches_container_arguments_environment_or_errors() -> TestResult {
    let db = MysqlTestDb::start_seeded().await?;
    db.execute(
        db.database(),
        "CREATE TABLE big (id INT PRIMARY KEY, v CHAR(64));
         SET SESSION cte_max_recursion_depth = 200000;
         INSERT INTO big WITH RECURSIVE s(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM s WHERE n < 200000)
         SELECT n, SHA2(n, 256) FROM s",
    )
    .await?;
    let tools = MysqlTools::detect(RunnerChoice::Docker, server_version(&db).await?).await?;

    let mut process = tools.dump(&config_for(&db, db.database()), &password(&db))?;
    let container = process
        .container_name()
        .ok_or("o modo docker deve nomear o container")?
        .to_owned();
    let mut stdout = process.take_stdout().ok_or("stdout ausente")?;
    let mut first = [0_u8; 14];
    stdout.read_exact(&mut first).await?;
    let inspected = Command::new("docker")
        .args([
            "inspect",
            "--format",
            "{{json .Args}} {{json .Config.Env}} {{json .Config.Cmd}}",
            &container,
        ])
        .output()?;
    process.cancel().await;

    let inspected = String::from_utf8_lossy(&inspected.stdout);
    assert_eq!(&first, b"-- MySQL dump ");
    assert!(inspected.contains("--defaults-extra-file="), "{inspected}");
    assert!(!inspected.contains(db.password()), "{inspected}");
    assert!(!container_exists(&container), "{container} ainda existe");

    let mut failing = tools.dump(&config_for(&db, "banco_inexistente"), &password(&db))?;
    let mut sink = Vec::new();
    failing
        .take_stdout()
        .ok_or("stdout ausente")?
        .read_to_end(&mut sink)
        .await?;
    let error = failing.wait().await.expect_err("o dump deve falhar");
    assert!(
        matches!(
            error,
            ToolError::Failed {
                tool: "mysqldump",
                ..
            }
        ),
        "{error:?}"
    );
    assert!(error.to_string().contains("banco_inexistente"), "{error}");
    assert!(!format!("{error:?}").contains(db.password()));
    Ok(())
}

#[tokio::test]
async fn restore_refuses_client_commands_embedded_in_the_dump() -> TestResult {
    let db = MysqlTestDb::start().await?;
    let tools = MysqlTools::detect(RunnerChoice::Docker, server_version(&db).await?).await?;
    let admin = Admin::new(
        &config_for(&db, db.database()),
        &password(&db),
        Timeouts::default(),
    );
    let dir = tempfile::tempdir()?;
    let marker = dir.path().join("executado");
    let file = dir.path().join("malicioso.sql");
    std::fs::write(
        &file,
        format!(
            "CREATE TABLE t (id INT);\n\\! touch {0}\nsystem touch {0}\nINSERT INTO t VALUES (1);\n",
            marker.display()
        ),
    )?;
    admin.create_database("alvo").await?;

    let result = tools
        .restore(&config_for(&db, "alvo"), &password(&db), &file)?
        .wait()
        .await;

    assert!(
        matches!(result, Err(ToolError::Failed { tool: "mysql", .. })),
        "{result:?}"
    );
    assert!(!marker.exists());
    assert_eq!(
        db.rows("alvo", "SELECT COUNT(*) FROM t").await?,
        vec![vec!["0".to_owned()]]
    );
    Ok(())
}

#[tokio::test]
async fn missing_native_client_is_reported() {
    let paths = ToolPaths {
        mysqldump: PathBuf::from("/nonexistent/mysqldump"),
        mysql: PathBuf::from("/nonexistent/mysql"),
        docker: PathBuf::from("/nonexistent/docker"),
    };
    let server = MysqlVersion { major: 8, minor: 4 };

    let native = MysqlTools::detect_with(RunnerChoice::Native, server, paths.clone()).await;
    let auto = MysqlTools::detect_with(RunnerChoice::Auto, server, paths.clone()).await;
    let docker = MysqlTools::detect_with(RunnerChoice::Docker, server, paths).await;

    assert!(matches!(
        native,
        Err(ToolError::NotFound { tool: "mysqldump", ref required }) if required == "MySQL 8.4"
    ));
    assert!(matches!(auto, Err(ToolError::NoRunner { client: "MySQL" })));
    assert!(matches!(docker, Err(ToolError::DockerUnavailable)));
}

#[test]
fn truncated_dump_is_detected_before_restore() -> TestResult {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("cortado.sql");
    std::fs::write(
        &file,
        "-- MySQL dump 10.13  Distrib 8.4.11, for Linux (x86_64)\nCREATE TABLE `t` (\n",
    )?;

    assert!(matches!(inspect_dump(&file), Err(DumpError::Incomplete)));
    assert!(matches!(
        inspect_dump(&dir.path().join("ausente.sql")),
        Err(DumpError::Io { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn trigger_saved_with_trailing_semicolon_is_flagged_as_unrestorable() -> TestResult {
    let db = MysqlTestDb::start().await?;
    db.execute(
        db.database(),
        "CREATE TABLE t (i INT);
         CREATE TRIGGER t_before BEFORE INSERT ON t FOR EACH ROW SET NEW.i = NEW.i + 1;
         CREATE TABLE u (i INT)",
    )
    .await?;
    let tools = MysqlTools::detect(RunnerChoice::Docker, server_version(&db).await?).await?;
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("trigger.sql");
    dump_to_file(&tools, &db, &file).await?;

    let inspected = inspect_dump(&file);

    match inspected {
        Err(DumpError::UnrestorableObject { kind, name }) => {
            assert_eq!((kind, name.as_str()), (ObjectKind::Trigger, "t_before"));
        }
        other => panic!("o trigger com ';' final deveria ser recusado: {other:?}"),
    }
    let admin = Admin::new(
        &config_for(&db, db.database()),
        &password(&db),
        Timeouts::default(),
    );
    admin.create_database("prova").await?;
    let restored = tools
        .restore(&config_for(&db, "prova"), &password(&db), &file)?
        .wait()
        .await;
    assert!(
        restored.is_err(),
        "o MySQL deveria falhar ao restaurar esse dump, confirmando a detecção"
    );
    Ok(())
}

#[tokio::test]
async fn definer_probe_refuses_a_user_that_cannot_recreate_the_objects() -> TestResult {
    let db = MysqlTestDb::start_seeded().await?;
    db.execute(
        "mysql",
        "CREATE USER 'restaurador'@'%' IDENTIFIED BY 'senha-do-restaurador';
         GRANT CREATE, DROP ON *.* TO 'restaurador'@'%';
         GRANT ALL ON `alvo`.* TO 'restaurador'@'%'",
    )
    .await?;
    let tools = MysqlTools::detect(RunnerChoice::Docker, server_version(&db).await?).await?;
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("seed.sql");
    dump_to_file(&tools, &db, &file).await?;
    let info = inspect_dump(&file)?;
    assert!(
        info.definers
            .iter()
            .any(|(kind, definer)| *kind == ObjectKind::Trigger && definer.user == "root"),
        "{:?}",
        info.definers
    );

    let limited = MysqlConnectionConfig::new(
        db.host(),
        db.port(),
        "information_schema",
        "restaurador",
        TlsMode::Auto,
    )?;
    let limited_admin = Admin::new(
        &limited,
        &Password::new("senha-do-restaurador".to_owned()),
        Timeouts::default(),
    );
    limited_admin.create_database("alvo").await?;
    let refused = limited_admin.probe_definers("alvo", &info.definers).await;

    assert!(
        matches!(refused, Err(AdminError::DefinerRefused { .. })),
        "{refused:?}"
    );
    assert!(
        !refused
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default()
            .contains("senha-do-restaurador")
    );

    let root_admin = Admin::new(
        &config_for(&db, db.database()),
        &password(&db),
        Timeouts::default(),
    );
    root_admin.create_database("alvo_root").await?;
    root_admin
        .probe_definers("alvo_root", &info.definers)
        .await?;
    let leftovers = db
        .rows(
            "information_schema",
            "SELECT (SELECT COUNT(*) FROM TABLES WHERE TABLE_SCHEMA = 'alvo_root')
                  + (SELECT COUNT(*) FROM ROUTINES WHERE ROUTINE_SCHEMA = 'alvo_root')
                  + (SELECT COUNT(*) FROM EVENTS WHERE EVENT_SCHEMA = 'alvo_root')
                  + (SELECT COUNT(*) FROM TRIGGERS WHERE TRIGGER_SCHEMA = 'alvo_root')",
        )
        .await?;
    assert_eq!(
        leftovers,
        vec![vec!["0".to_owned()]],
        "a sonda deixou objetos para trás"
    );
    Ok(())
}
