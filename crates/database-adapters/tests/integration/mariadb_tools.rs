use std::path::{Path, PathBuf};
use std::process::Command;

use crate::support::{MysqlTestDb, RoundTrip, TestResult, assert_round_trip};
use database_backup_adapters::mysql::{
    Admin, Flavor, MysqlTools, MysqlVersion, RunnerChoice, Timeouts, ToolError, ToolPaths,
    inspect_dump, test_connection,
};
use database_backup_core::{DatabaseEngine, MysqlConnectionConfig, Password, TlsMode};
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
    Ok(
        MysqlVersion::parse_as(Flavor::Mariadb, &info.server_version)
            .ok_or("versão do servidor MariaDB inválida")?,
    )
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
        match MysqlTools::detect_flavor(Flavor::Mariadb, choice, server).await {
            Ok(tools) => runners.push(tools),
            Err(error) if choice == RunnerChoice::Native => {
                assert!(
                    std::env::var_os("DBU_REQUIRE_NATIVE_MARIADB").is_none(),
                    "cliente MariaDB nativo exigido: {error}"
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
    let db = MysqlTestDb::start_mariadb_seeded().await?;
    let expected = snapshot(&db, db.database()).await?;
    let admin = Admin::new(
        &config_for(&db, db.database()),
        &password(&db),
        Timeouts::default(),
    );
    let source_tables = usize::try_from(admin.count_tables(db.database()).await?)?;
    let server = server_version(&db).await?;
    let dir = tempfile::tempdir()?;

    for tools in available_runners(server).await {
        let runner = tools.kind().as_str();
        assert_eq!(tools.flavor(), Flavor::Mariadb, "{runner}");
        let file = dir.path().join(format!("{runner}.sql"));
        dump_to_file(&tools, &db, &file).await?;

        let info = inspect_dump(&file)?;
        assert_eq!(info.flavor, Flavor::Mariadb, "{runner}");
        assert_eq!(info.dumped_from, Some(server), "{runner}");
        assert_eq!(info.database.as_deref(), Some(db.database()), "{runner}");
        assert_eq!((info.views, info.triggers), (1, 1), "{runner}");

        let target = format!("copia_{runner}");
        admin.create_database(&target).await?;
        admin.probe_definers(&target, &info.definers).await?;
        tools
            .restore(&config_for(&db, &target), &password(&db), &file)?
            .wait()
            .await?;

        assert_round_trip(&RoundTrip {
            engine: DatabaseEngine::Mariadb,
            label: runner,
            source_tables,
            artifact_tables: info.tables,
            restored_tables: usize::try_from(admin.count_tables(&target).await?)?,
            source_before: &expected,
            restored: &snapshot(&db, &target).await?,
            source_after: &snapshot(&db, db.database()).await?,
        });
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
    }
    Ok(())
}

#[tokio::test]
async fn password_never_reaches_container_arguments_environment_or_errors() -> TestResult {
    let db = MysqlTestDb::start_mariadb_seeded().await?;
    db.execute(
        db.database(),
        "CREATE TABLE big (id INT PRIMARY KEY, v CHAR(64));
         INSERT INTO big SELECT seq, SHA2(seq, 256) FROM seq_1_to_200000",
    )
    .await?;
    let tools = MysqlTools::detect_flavor(
        Flavor::Mariadb,
        RunnerChoice::Docker,
        server_version(&db).await?,
    )
    .await?;

    let mut process = tools.dump(&config_for(&db, db.database()), &password(&db))?;
    let container = process
        .container_name()
        .ok_or("o modo docker deve nomear o container")?
        .to_owned();
    let mut stdout = process.take_stdout().ok_or("stdout ausente")?;
    let mut first = [0_u8; 160];
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
    let first = String::from_utf8_lossy(&first);
    assert!(first.contains("-- MariaDB dump "), "{first}");
    assert!(inspected.contains("--defaults-extra-file="), "{inspected}");
    assert!(!inspected.contains("--set-gtid-purged"), "{inspected}");
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
                tool: "mariadb-dump",
                ..
            }
        ),
        "{error:?}"
    );
    assert!(!format!("{error:?}").contains(db.password()));
    Ok(())
}

#[tokio::test]
async fn restore_refuses_client_commands_embedded_in_the_dump() -> TestResult {
    let db = MysqlTestDb::start_mariadb().await?;
    let tools = MysqlTools::detect_flavor(
        Flavor::Mariadb,
        RunnerChoice::Docker,
        server_version(&db).await?,
    )
    .await?;
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
        matches!(
            result,
            Err(ToolError::Failed {
                tool: "mariadb",
                ..
            })
        ),
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
async fn mysql_and_mariadb_tools_are_never_mixed() -> TestResult {
    let db = MysqlTestDb::start_mariadb().await?;
    let server = server_version(&db).await?;
    let info = test_connection(
        &config_for(&db, db.database()),
        &password(&db),
        Timeouts::default(),
    )
    .await?;

    assert_eq!(Flavor::of(&info.server_version), Flavor::Mariadb);
    assert_eq!(MysqlVersion::parse(&info.server_version), None);
    let mysql_image_for_mariadb =
        MysqlTools::detect_flavor(Flavor::Mysql, RunnerChoice::Docker, server).await;
    assert!(
        mysql_image_for_mariadb.is_err(),
        "não existe mysql:{server}; a imagem MySQL nunca serve a um servidor MariaDB"
    );
    Ok(())
}

#[tokio::test]
async fn missing_native_client_is_reported() {
    let paths = ToolPaths {
        mysqldump: PathBuf::from("/nonexistent/mariadb-dump"),
        mysql: PathBuf::from("/nonexistent/mariadb"),
        docker: PathBuf::from("/nonexistent/docker"),
    };
    let server = MysqlVersion {
        major: 11,
        minor: 4,
    };

    let native = MysqlTools::detect_with_flavor(
        Flavor::Mariadb,
        RunnerChoice::Native,
        server,
        paths.clone(),
    )
    .await;
    let auto =
        MysqlTools::detect_with_flavor(Flavor::Mariadb, RunnerChoice::Auto, server, paths).await;

    assert!(matches!(
        native,
        Err(ToolError::NotFound { tool: "mariadb-dump", ref required }) if required == "MariaDB 11.4"
    ));
    assert!(matches!(
        auto,
        Err(ToolError::NoRunner { client: "MariaDB" })
    ));
}
