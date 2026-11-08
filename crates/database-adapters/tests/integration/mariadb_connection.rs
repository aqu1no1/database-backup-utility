use crate::support::{MysqlTestDb, TestResult};
use database_backup_adapters::mysql::{ConnectionError, Timeouts, test_connection};
use database_backup_core::{MysqlConnectionConfig, Password, TlsMode};

fn config_for(db: &MysqlTestDb, database: &str) -> MysqlConnectionConfig {
    MysqlConnectionConfig::new(db.host(), db.port(), database, db.user(), TlsMode::Auto)
        .expect("configuração de teste válida")
}

fn assert_no_secret(error: &ConnectionError, secret: &str) {
    assert!(!error.to_string().contains(secret));
    assert!(!format!("{error:?}").contains(secret));
}

#[tokio::test]
async fn connection_outcomes_are_classified_without_leaking_secrets() -> TestResult {
    let db = MysqlTestDb::start_mariadb().await?;
    let password = Password::new(db.password().to_owned());

    let info = test_connection(
        &config_for(&db, db.database()),
        &password,
        Timeouts::default(),
    )
    .await?;
    assert!(
        info.server_version.starts_with(&format!("{}.", db.tag()))
            && info.server_version.contains("-MariaDB"),
        "{}",
        info.server_version
    );

    let wrong = "senha-incorreta-de-teste";
    let error = test_connection(
        &config_for(&db, db.database()),
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
        &config_for(&db, "banco_inexistente"),
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
    Ok(())
}
