#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::path::PathBuf;
use std::process::Command;

use database_backup_adapters::postgres::{
    PgTools, PgVersion, RunnerChoice, RunnerKind, Timeouts, ToolError, ToolPaths, test_connection,
};
use database_backup_core::{Password, PostgresConnectionConfig, TlsMode};
use support::{PostgresTestDb, TestResult};
use tokio::io::AsyncReadExt;

fn config_for(db: &PostgresTestDb, database: &str) -> PostgresConnectionConfig {
    PostgresConnectionConfig::new(db.host(), db.port(), database, db.user(), TlsMode::Auto)
        .expect("configuração de teste válida")
}

async fn server_version(db: &PostgresTestDb) -> TestResult<PgVersion> {
    let info = test_connection(
        &config_for(db, db.database()),
        &Password::new(db.password().to_owned()),
        Timeouts::default(),
    )
    .await?;
    Ok(PgVersion::parse(&info.server_version).ok_or("versão do servidor inválida")?)
}

async fn dump_to_file(
    tools: &PgTools,
    db: &PostgresTestDb,
    path: &std::path::Path,
) -> TestResult<(Vec<u8>, Option<String>)> {
    let mut process = tools.dump(
        &config_for(db, db.database()),
        &Password::new(db.password().to_owned()),
    )?;
    let container = process.container_name().map(str::to_owned);
    let mut stdout = process.take_stdout().ok_or("stdout ausente")?;
    let mut file = tokio::fs::File::create(path).await?;
    tokio::io::copy(&mut stdout, &mut file).await?;
    process.wait().await?;
    Ok((tokio::fs::read(path).await?, container))
}

fn container_exists(name: &str) -> bool {
    let output = Command::new("docker")
        .args(["ps", "-aq", "--filter", &format!("name=^{name}$")])
        .output()
        .expect("docker ps");
    !output.stdout.is_empty()
}

fn native_required() -> bool {
    std::env::var_os("DBU_REQUIRE_NATIVE").is_some()
}

#[tokio::test]
async fn docker_runner_dumps_and_lists_archive() -> TestResult {
    let db = PostgresTestDb::start_seeded().await?;
    let tools = PgTools::detect(RunnerChoice::Docker, server_version(&db).await?).await?;
    let dir = tempfile::tempdir()?;
    let archive = dir.path().join("seed.dump");

    let (bytes, container) = dump_to_file(&tools, &db, &archive).await?;
    let entries = tools.restore_list(&archive).await?.entries;
    let container = container.ok_or("o modo docker deve nomear o container")?;

    assert_eq!(tools.kind(), RunnerKind::Docker);
    assert!(bytes.starts_with(b"PGDMP"), "formato custom esperado");
    assert!(entries >= 4, "TOC com tabelas e dados: {entries}");
    assert!(!container_exists(&container), "{container} ainda existe");
    Ok(())
}

#[tokio::test]
async fn native_runner_dumps_when_client_is_installed() -> TestResult {
    let db = PostgresTestDb::start_seeded().await?;
    let tools = match PgTools::detect(RunnerChoice::Native, server_version(&db).await?).await {
        Ok(tools) => tools,
        Err(error @ (ToolError::NotFound { .. } | ToolError::Incompatible { .. }))
            if !native_required() =>
        {
            eprintln!("modo native pulado: {error}");
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    let dir = tempfile::tempdir()?;
    let archive = dir.path().join("seed.dump");

    let (bytes, container) = dump_to_file(&tools, &db, &archive).await?;

    assert_eq!(tools.kind(), RunnerKind::Native);
    assert_eq!(container, None);
    assert!(bytes.starts_with(b"PGDMP"));
    assert!(tools.restore_list(&archive).await?.entries >= 4);
    Ok(())
}

#[tokio::test]
async fn missing_native_client_is_reported() {
    let paths = ToolPaths {
        pg_dump: PathBuf::from("/nonexistent/pg_dump"),
        pg_restore: PathBuf::from("/nonexistent/pg_restore"),
        docker: PathBuf::from("/nonexistent/docker"),
    };
    let server = PgVersion {
        major: 17,
        minor: 0,
    };

    let native = PgTools::detect_with(RunnerChoice::Native, server, paths.clone()).await;
    let auto = PgTools::detect_with(RunnerChoice::Auto, server, paths.clone()).await;
    let docker = PgTools::detect_with(RunnerChoice::Docker, server, paths).await;

    assert!(matches!(
        native,
        Err(ToolError::NotFound {
            tool: "pg_dump",
            ref required,
        }) if required == "PostgreSQL 17"
    ));
    assert!(matches!(
        auto,
        Err(ToolError::NoRunner {
            client: "PostgreSQL"
        })
    ));
    assert!(matches!(docker, Err(ToolError::DockerUnavailable)));
}

#[tokio::test]
async fn failed_dump_reports_stderr_without_secrets() -> TestResult {
    let db = PostgresTestDb::start().await?;
    let tools = PgTools::detect(RunnerChoice::Docker, server_version(&db).await?).await?;

    let mut process = tools.dump(
        &config_for(&db, "banco_inexistente"),
        &Password::new(db.password().to_owned()),
    )?;
    let mut stdout = process.take_stdout().ok_or("stdout ausente")?;
    let mut sink = Vec::new();
    stdout.read_to_end(&mut sink).await?;
    let error = process.wait().await.expect_err("o dump deve falhar");

    let message = error.to_string();
    assert!(
        matches!(
            error,
            ToolError::Failed {
                tool: "pg_dump",
                ..
            }
        ),
        "{error:?}"
    );
    assert!(message.contains("banco_inexistente"), "{message}");
    assert!(!message.contains(db.password()));
    assert!(!format!("{error:?}").contains(db.password()));
    Ok(())
}

#[tokio::test]
async fn truncated_archive_fails_listing() -> TestResult {
    let db = PostgresTestDb::start_seeded().await?;
    let tools = PgTools::detect(RunnerChoice::Docker, server_version(&db).await?).await?;
    let dir = tempfile::tempdir()?;
    let archive = dir.path().join("seed.dump");
    let (bytes, _) = dump_to_file(&tools, &db, &archive).await?;
    let truncated = dir.path().join("truncated.dump");
    tokio::fs::write(&truncated, &bytes[..bytes.len() / 3]).await?;

    let result = tools.restore_list(&truncated).await;

    assert!(
        matches!(
            result,
            Err(ToolError::Failed {
                tool: "pg_restore",
                ..
            })
        ),
        "{result:?}"
    );
    Ok(())
}

#[tokio::test]
async fn cancel_stops_dump_and_container() -> TestResult {
    let db = PostgresTestDb::start_seeded().await?;
    db.client()
        .batch_execute(
            "CREATE TABLE big AS SELECT g AS id, md5(g::text) AS v FROM generate_series(1, 300000) g",
        )
        .await?;
    let tools = PgTools::detect(RunnerChoice::Docker, server_version(&db).await?).await?;

    let mut process = tools.dump(
        &config_for(&db, db.database()),
        &Password::new(db.password().to_owned()),
    )?;
    let container = process
        .container_name()
        .ok_or("o modo docker deve nomear o container")?
        .to_owned();
    let mut stdout = process.take_stdout().ok_or("stdout ausente")?;
    let mut first = [0_u8; 5];
    stdout.read_exact(&mut first).await?;
    process.cancel().await;

    assert_eq!(&first, b"PGDMP");
    assert!(!container_exists(&container), "{container} ainda existe");
    Ok(())
}

#[tokio::test]
async fn cancel_right_after_spawn_leaves_no_container() -> TestResult {
    let db = PostgresTestDb::start_seeded().await?;
    let tools = PgTools::detect(RunnerChoice::Docker, server_version(&db).await?).await?;
    let config = config_for(&db, db.database());
    let password = Password::new(db.password().to_owned());

    for _ in 0..3 {
        let process = tools.dump(&config, &password)?;
        let container = process
            .container_name()
            .ok_or("o modo docker deve nomear o container")?
            .to_owned();
        process.cancel().await;
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        assert!(!container_exists(&container), "{container} ainda existe");
    }
    Ok(())
}
