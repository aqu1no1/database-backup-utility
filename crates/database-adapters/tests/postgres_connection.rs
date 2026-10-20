mod support;

use std::net::TcpListener;
use std::time::Duration;

use database_backup_adapters::postgres::{ConnectionError, Timeouts, test_connection};
use database_backup_core::{Password, PostgresConnectionConfig, TlsMode};
use support::{PostgresTestDb, TestResult};

fn config_for(db: &PostgresTestDb, database: &str, user: &str) -> PostgresConnectionConfig {
    PostgresConnectionConfig::new(db.host(), db.port(), database, user, TlsMode::Auto)
        .expect("configuração de teste válida")
}

fn assert_no_secret(error: &ConnectionError, secret: &str) {
    assert!(!error.to_string().contains(secret));
    assert!(!format!("{error:?}").contains(secret));
}

#[tokio::test]
async fn valid_credentials_connect_and_report_version() -> TestResult {
    let db = PostgresTestDb::start().await?;
    let config = config_for(&db, db.database(), db.user());

    let info = test_connection(
        &config,
        &Password::new(db.password().to_owned()),
        Timeouts::default(),
    )
    .await?;

    assert!(
        info.server_version.starts_with("17."),
        "{}",
        info.server_version
    );
    Ok(())
}

#[tokio::test]
async fn wrong_password_is_authentication_failure() -> TestResult {
    let db = PostgresTestDb::start().await?;
    let config = config_for(&db, db.database(), db.user());
    let wrong = "senha-incorreta-de-teste";

    let error = test_connection(
        &config,
        &Password::new(wrong.to_owned()),
        Timeouts::default(),
    )
    .await
    .expect_err("a senha incorreta deve falhar");

    assert!(
        matches!(error, ConnectionError::AuthenticationFailed { .. }),
        "{error:?}"
    );
    assert_no_secret(&error, wrong);
    assert_no_secret(&error, db.password());
    Ok(())
}

#[tokio::test]
async fn missing_database_is_reported() -> TestResult {
    let db = PostgresTestDb::start().await?;
    let config = config_for(&db, "banco_inexistente", db.user());

    let error = test_connection(
        &config,
        &Password::new(db.password().to_owned()),
        Timeouts::default(),
    )
    .await
    .expect_err("o banco inexistente deve falhar");

    assert!(
        matches!(error, ConnectionError::DatabaseNotFound { .. }),
        "{error:?}"
    );
    assert_no_secret(&error, db.password());
    Ok(())
}

#[tokio::test]
async fn closed_port_is_unreachable() -> TestResult {
    let port = {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.local_addr()?.port()
    };
    let config =
        PostgresConnectionConfig::new("127.0.0.1", port, "postgres", "postgres", TlsMode::Auto)?;

    let error = test_connection(&config, &Password::new("x".to_owned()), Timeouts::default())
        .await
        .expect_err("a porta fechada deve falhar");

    assert!(
        matches!(error, ConnectionError::Unreachable { .. }),
        "{error:?}"
    );
    Ok(())
}

#[tokio::test]
async fn silent_server_times_out() -> TestResult {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let config =
        PostgresConnectionConfig::new("127.0.0.1", port, "postgres", "postgres", TlsMode::Auto)?;
    let timeouts = Timeouts {
        connect: Duration::from_millis(500),
        query: Duration::from_millis(500),
    };

    let error = test_connection(&config, &Password::new("x".to_owned()), timeouts)
        .await
        .expect_err("um servidor que não responde deve esgotar o tempo");

    assert!(
        matches!(error, ConnectionError::Timeout { .. }),
        "{error:?}"
    );
    drop(listener);
    Ok(())
}

#[tokio::test]
async fn verify_full_never_falls_back_to_plain_text() -> TestResult {
    let db = PostgresTestDb::start().await?;
    let config = PostgresConnectionConfig::new(
        db.host(),
        db.port(),
        db.database(),
        db.user(),
        TlsMode::VerifyFull,
    )?;

    let error = test_connection(
        &config,
        &Password::new(db.password().to_owned()),
        Timeouts::default(),
    )
    .await
    .expect_err("um servidor sem TLS deve ser recusado");

    assert!(matches!(error, ConnectionError::Tls { .. }), "{error:?}");
    assert_no_secret(&error, db.password());
    Ok(())
}
