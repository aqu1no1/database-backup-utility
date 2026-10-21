#![allow(dead_code)]

use std::collections::hash_map::RandomState;
use std::error::Error;
use std::hash::{BuildHasher, Hasher};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use testcontainers::{
    ContainerAsync, GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};
use tokio::task::JoinHandle;
use tokio_postgres::{Client, Config, NoTls};

const IMAGE: &str = "postgres";
const TAG: &str = "17";
const USER: &str = "backup_test";
const DATABASE: &str = "backup_test";

pub const SEED_SQL: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/postgres/seed.sql"
));

pub const E2E_SQL: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/postgres/e2e.sql"
));

pub type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

pub struct PostgresTestDb {
    client: Client,
    connection_task: JoinHandle<()>,
    host: String,
    port: u16,
    password: String,
    _container: ContainerAsync<GenericImage>,
}

impl PostgresTestDb {
    pub async fn start() -> TestResult<Self> {
        let password = random_password();
        let container = GenericImage::new(IMAGE, TAG)
            .with_exposed_port(5432.tcp())
            .with_wait_for(WaitFor::message_on_stderr(
                "database system is ready to accept connections",
            ))
            .with_env_var("POSTGRES_USER", USER)
            .with_env_var("POSTGRES_PASSWORD", &password)
            .with_env_var("POSTGRES_DB", DATABASE)
            .start()
            .await
            .map_err(|error| format!("falha ao iniciar o container {IMAGE}:{TAG}: {error}"))?;

        let host = container.get_host().await?;
        let port = container.get_host_port_ipv4(5432.tcp()).await?;

        let mut config = Config::new();
        config
            .host(host.to_string())
            .port(port)
            .user(USER)
            .password(&password)
            .dbname(DATABASE)
            .connect_timeout(Duration::from_secs(10));

        let (client, connection) = config.connect(NoTls).await.map_err(|error| {
            format!("falha ao conectar no PostgreSQL de teste em {host}:{port}: {error}")
        })?;
        let connection_task = tokio::spawn(async move {
            let _ = connection.await;
        });

        Ok(Self {
            client,
            connection_task,
            host: host.to_string(),
            port,
            password,
            _container: container,
        })
    }

    pub async fn start_seeded() -> TestResult<Self> {
        Self::start_with("seed.sql", SEED_SQL).await
    }

    pub async fn start_with(name: &str, sql: &str) -> TestResult<Self> {
        let db = Self::start().await?;
        db.client
            .batch_execute(sql)
            .await
            .map_err(|error| format!("falha ao aplicar a fixture {name}: {error}"))?;
        Ok(db)
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn user(&self) -> &str {
        USER
    }

    pub fn database(&self) -> &str {
        DATABASE
    }

    pub fn password(&self) -> &str {
        &self.password
    }
}

impl Drop for PostgresTestDb {
    fn drop(&mut self) {
        self.connection_task.abort();
    }
}

fn random_password() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u128(nanos);
    hasher.write_u32(std::process::id());
    format!("test-{:016x}", hasher.finish())
}
