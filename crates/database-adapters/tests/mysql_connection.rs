#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::net::TcpListener;
use std::time::Duration;

use database_backup_adapters::mysql::{ConnectionError, Timeouts, test_connection};
use database_backup_core::{MysqlConnectionConfig, Password, TlsMode};
use support::{MysqlTestDb, TestResult};

fn config_for(db: &MysqlTestDb, database: &str, tls: TlsMode) -> MysqlConnectionConfig {
    MysqlConnectionConfig::new(db.host(), db.port(), database, db.user(), tls)
        .expect("configuração de teste válida")
}

fn assert_no_secret(error: &ConnectionError, secret: &str) {
    assert!(!error.to_string().contains(secret));
    assert!(!format!("{error:?}").contains(secret));
}

#[tokio::test]
async fn connection_outcomes_are_classified_without_leaking_secrets() -> TestResult {
    let db = MysqlTestDb::start().await?;
    let password = Password::new(db.password().to_owned());

    let info = test_connection(
        &config_for(&db, db.database(), TlsMode::Auto),
        &password,
        Timeouts::default(),
    )
    .await?;
    assert!(
        info.server_version.starts_with("8.4."),
        "{}",
        info.server_version
    );

    let wrong = "senha-incorreta-de-teste";
    let error = test_connection(
        &config_for(&db, db.database(), TlsMode::Auto),
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

    let error = test_connection(
        &config_for(&db, "banco_inexistente", TlsMode::Auto),
        &password,
        Timeouts::default(),
    )
    .await
    .expect_err("o banco inexistente deve falhar");
    assert!(
        matches!(error, ConnectionError::DatabaseNotFound { .. }),
        "{error:?}"
    );
    assert_no_secret(&error, db.password());

    let error = test_connection(
        &config_for(&db, db.database(), TlsMode::VerifyFull),
        &password,
        Timeouts::default(),
    )
    .await
    .expect_err("o certificado autoassinado do container deve ser recusado");
    assert!(matches!(error, ConnectionError::Tls { .. }), "{error:?}");
    assert_no_secret(&error, db.password());
    Ok(())
}

#[tokio::test]
async fn closed_port_is_unreachable() -> TestResult {
    let port = {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.local_addr()?.port()
    };
    let config = MysqlConnectionConfig::new("127.0.0.1", port, "app", "root", TlsMode::Auto)?;

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
    let config = MysqlConnectionConfig::new("127.0.0.1", port, "app", "root", TlsMode::Auto)?;
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
