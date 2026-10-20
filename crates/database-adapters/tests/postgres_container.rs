use std::time::Duration;

use testcontainers::{
    GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};
use tokio_postgres::{Config, NoTls};

#[tokio::test]
async fn postgres_container_accepts_sql_query() -> Result<(), Box<dyn std::error::Error>> {
    let container = GenericImage::new("postgres", "17")
        .with_exposed_port(5432.tcp())
        .with_wait_for(WaitFor::message_on_stderr(
            "database system is ready to accept connections",
        ))
        .with_env_var("POSTGRES_USER", "backup_test")
        .with_env_var("POSTGRES_PASSWORD", "test-only-password")
        .with_env_var("POSTGRES_DB", "backup_test")
        .start()
        .await?;

    let host = container.get_host().await?;
    let port = container.get_host_port_ipv4(5432.tcp()).await?;
    let mut config = Config::new();
    config
        .host(host.to_string())
        .port(port)
        .user("backup_test")
        .password("test-only-password")
        .dbname("backup_test")
        .connect_timeout(Duration::from_secs(10));

    let (client, connection) = config.connect(NoTls).await?;
    let connection_task = tokio::spawn(async move {
        let _ = connection.await;
    });
    let row = client.query_one("SELECT 1::INTEGER", &[]).await?;
    let value: i32 = row.get(0);
    assert_eq!(value, 1);

    drop(client);
    connection_task.await?;
    Ok(())
}
