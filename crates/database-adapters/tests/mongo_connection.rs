#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::net::TcpListener;
use std::time::Duration;

use database_backup_adapters::mongo::{
    ConnectionError, MongoVersion, Timeouts, Topology, probe_server, test_connection,
};
use database_backup_core::{MongoConnectionConfig, Password, TlsMode};
use support::{MongoTestDb, TestResult};

fn config_for(
    db: &MongoTestDb,
    database: &str,
    auth_database: &str,
    tls: TlsMode,
) -> MongoConnectionConfig {
    MongoConnectionConfig::new(
        db.host(),
        db.port(),
        database,
        db.user(),
        auth_database,
        tls,
    )
    .expect("configuração de teste válida")
}

fn assert_no_secret(error: &ConnectionError, secret: &str) {
    assert!(!error.to_string().contains(secret));
    assert!(!format!("{error:?}").contains(secret));
}

fn quick() -> Timeouts {
    Timeouts {
        connect: Duration::from_secs(2),
        query: Duration::from_secs(2),
    }
}

#[tokio::test]
async fn connection_outcomes_are_classified_without_leaking_secrets() -> TestResult {
    let db = MongoTestDb::start_seeded().await?;
    let password = Password::new(db.password().to_owned());
    let auth = db.auth_database();

    let server = test_connection(
        &config_for(&db, db.database(), auth, TlsMode::Auto),
        &password,
        Timeouts::default(),
    )
    .await?;
    assert!(
        server.server_version.starts_with(&format!("{}.", db.tag())),
        "{}",
        server.server_version
    );
    assert_eq!(Some(server.version), MongoVersion::parse(db.tag()));
    assert_eq!(server.topology, Topology::Standalone);

    let wrong = "senha-incorreta-de-teste";
    let error = test_connection(
        &config_for(&db, db.database(), auth, TlsMode::Auto),
        &Password::new(wrong.to_owned()),
        quick(),
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
        &config_for(&db, db.database(), "outro_banco", TlsMode::Auto),
        &password,
        quick(),
    )
    .await
    .expect_err("o usuário não existe no banco de autenticação informado");
    assert!(
        matches!(error, ConnectionError::AuthenticationFailed { .. }),
        "{error:?}"
    );
    assert_no_secret(&error, db.password());

    let error = test_connection(
        &config_for(&db, "banco_inexistente", auth, TlsMode::Auto),
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

    let probed = probe_server(
        &config_for(&db, "banco_inexistente", auth, TlsMode::Auto),
        &password,
        Timeouts::default(),
    )
    .await?;
    assert_eq!(probed.topology, Topology::Standalone);

    let error = test_connection(
        &config_for(&db, db.database(), auth, TlsMode::VerifyFull),
        &password,
        quick(),
    )
    .await
    .expect_err("um servidor sem TLS deve ser recusado com verify-full");
    assert!(matches!(error, ConnectionError::Tls { .. }), "{error:?}");
    assert_no_secret(&error, db.password());
    Ok(())
}

#[tokio::test]
async fn replica_set_primary_is_detected() -> TestResult {
    let db = MongoTestDb::start_replica_set_seeded().await?;
    let password = Password::new(db.password().to_owned());

    let server = test_connection(
        &config_for(&db, db.database(), db.auth_database(), TlsMode::Auto),
        &password,
        Timeouts::default(),
    )
    .await?;

    assert_eq!(
        server.topology,
        Topology::ReplicaSet {
            name: "rs0".to_owned(),
            primary: true
        }
    );
    assert_eq!(server.topology.as_str(), "replica_set");
    Ok(())
}

#[tokio::test]
async fn closed_port_is_unreachable() -> TestResult {
    let port = {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.local_addr()?.port()
    };
    let config =
        MongoConnectionConfig::new("127.0.0.1", port, "app", "root", "admin", TlsMode::Auto)?;

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
        MongoConnectionConfig::new("127.0.0.1", port, "app", "root", "admin", TlsMode::Auto)?;
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
