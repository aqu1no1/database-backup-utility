#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::path::Path;

use database_backup_adapters::postgres::{
    Admin, DatabaseState, PgTools, PgVersion, RunnerChoice, Timeouts, ToolError, test_connection,
};
use database_backup_core::{DatabaseEngine, Password, PostgresConnectionConfig, TlsMode};
use support::{PostgresTestDb, RoundTrip, TestResult, assert_round_trip};

fn config_for(db: &PostgresTestDb, database: &str) -> PostgresConnectionConfig {
    PostgresConnectionConfig::new(db.host(), db.port(), database, db.user(), TlsMode::Auto)
        .expect("configuração de teste válida")
}

fn password(db: &PostgresTestDb) -> Password {
    Password::new(db.password().to_owned())
}

async fn docker_tools(db: &PostgresTestDb) -> TestResult<PgTools> {
    let info = test_connection(
        &config_for(db, db.database()),
        &password(db),
        Timeouts::default(),
    )
    .await?;
    let server = PgVersion::parse(&info.server_version).ok_or("versão inválida")?;
    Ok(PgTools::detect(RunnerChoice::Docker, server).await?)
}

async fn dump_seed(tools: &PgTools, db: &PostgresTestDb, path: &Path) -> TestResult {
    let mut process = tools.dump(&config_for(db, db.database()), &password(db))?;
    let mut stdout = process.take_stdout().ok_or("stdout ausente")?;
    let mut file = tokio::fs::File::create(path).await?;
    tokio::io::copy(&mut stdout, &mut file).await?;
    process.wait().await?;
    Ok(())
}

async fn scalar(db: &PostgresTestDb, database: &str, sql: &str) -> TestResult<i64> {
    let (client, connection) = tokio_postgres::Config::new()
        .host(db.host())
        .port(db.port())
        .user(db.user())
        .password(db.password())
        .dbname(database)
        .connect(tokio_postgres::NoTls)
        .await?;
    tokio::spawn(connection);
    Ok(client.query_one(sql, &[]).await?.get(0))
}

async fn snapshot(db: &PostgresTestDb, database: &str) -> TestResult<(i64, i64)> {
    Ok((
        scalar(db, database, "SELECT COUNT(*) FROM customers").await?,
        scalar(db, database, "SELECT SUM(total_cents)::BIGINT FROM orders").await?,
    ))
}

#[tokio::test]
async fn restores_dump_into_new_database_with_same_data() -> TestResult {
    let db = PostgresTestDb::start_seeded().await?;
    let tools = docker_tools(&db).await?;
    let admin = Admin::new(
        &config_for(&db, "postgres"),
        &password(&db),
        Timeouts::default(),
    );
    let source_before = snapshot(&db, db.database()).await?;
    assert_eq!(source_before, (3, 18490));
    let dir = tempfile::tempdir()?;
    let archive = dir.path().join("seed.dump");
    dump_seed(&tools, &db, &archive).await?;

    let info = tools.restore_list(&archive).await?;
    admin.create_database("restaurado").await?;
    tools
        .restore(&config_for(&db, "restaurado"), &password(&db), &archive)?
        .wait()
        .await?;

    assert_eq!(info.database.as_deref(), Some(db.database()));
    assert_eq!(info.dumped_from.map(|v| v.major), Some(17));
    assert_round_trip(&RoundTrip {
        engine: DatabaseEngine::Postgres,
        label: tools.kind().as_str(),
        source_tables: usize::try_from(admin.count_tables(db.database()).await?)?,
        artifact_tables: info.tables,
        restored_tables: usize::try_from(admin.count_tables("restaurado").await?)?,
        source_before: &source_before,
        restored: &snapshot(&db, "restaurado").await?,
        source_after: &snapshot(&db, db.database()).await?,
    });
    Ok(())
}

#[tokio::test]
async fn failed_restore_leaves_target_without_partial_data() -> TestResult {
    let db = PostgresTestDb::start_seeded().await?;
    let tools = docker_tools(&db).await?;
    let admin = Admin::new(
        &config_for(&db, "postgres"),
        &password(&db),
        Timeouts::default(),
    );
    let dir = tempfile::tempdir()?;
    let archive = dir.path().join("seed.dump");
    dump_seed(&tools, &db, &archive).await?;
    let bytes = tokio::fs::read(&archive).await?;
    let truncated = dir.path().join("truncated.dump");
    tokio::fs::write(&truncated, &bytes[..bytes.len() * 3 / 4]).await?;
    admin.create_database("parcial").await?;

    let result = tools
        .restore(&config_for(&db, "parcial"), &password(&db), &truncated)?
        .wait()
        .await;

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
    let message = result
        .err()
        .map(|error| error.to_string())
        .unwrap_or_default();
    assert!(!message.contains(db.password()));
    assert_eq!(admin.count_tables("parcial").await?, 0);
    Ok(())
}

#[tokio::test]
async fn admin_reports_state_and_manages_databases() -> TestResult {
    let db = PostgresTestDb::start_seeded().await?;
    let admin = Admin::new(
        &config_for(&db, "postgres"),
        &password(&db),
        Timeouts::default(),
    );

    assert_eq!(
        admin.database_state("nao_existe").await?,
        DatabaseState::Missing
    );
    assert_eq!(
        admin.database_state(db.database()).await?,
        DatabaseState::HasData { tables: 2 }
    );

    admin.create_database("vazio").await?;
    assert_eq!(admin.database_state("vazio").await?, DatabaseState::Empty);

    admin.rename_database("vazio", "renomeado").await?;
    assert_eq!(admin.database_state("vazio").await?, DatabaseState::Missing);
    assert_eq!(
        admin.database_state("renomeado").await?,
        DatabaseState::Empty
    );

    admin.drop_database("renomeado").await?;
    assert_eq!(
        admin.database_state("renomeado").await?,
        DatabaseState::Missing
    );
    Ok(())
}
