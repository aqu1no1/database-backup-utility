#![allow(dead_code)]

use std::collections::BTreeMap;
use std::collections::hash_map::RandomState;
use std::error::Error;
use std::fmt::Debug;
use std::hash::{BuildHasher, Hasher};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use database_backup_core::{DatabaseEngine, Operation, Support};
use mongodb::Client as MongoClient;
use mongodb::bson::{Document, doc};
use mongodb::options::{ClientOptions, Credential, ServerAddress};
use mysql_async::prelude::Queryable;
use testcontainers::{
    ContainerAsync, GenericImage, ImageExt,
    core::{ExecCommand, IntoContainerPort, WaitFor},
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

pub const MYSQL_SEED_SQL: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/mysql/seed.sql"
));

pub const MONGO_SEED_JS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/mongodb/seed.js"
));

const MYSQL_IMAGE: &str = "mysql";
const MYSQL_TAG: &str = "8.4";
const MYSQL_USER: &str = "root";
const MYSQL_DATABASE: &str = "backup_test";
const MYSQL_READY: &str = "port: 3306  MySQL Community Server";

const MONGO_IMAGE: &str = "mongo";
const MONGO_DEFAULT_TAG: &str = "7.0";
const MONGO_TAG_VARIABLE: &str = "DBU_MONGO_TAG";
const MONGO_USER: &str = "root";
const MONGO_DATABASE: &str = "backup_test";
const MONGO_AUTH_DATABASE: &str = "admin";
const MONGO_INIT_DONE: &str = "MongoDB init process complete; ready for start up.";
const MONGO_READY: &str = "Waiting for connections";
const MONGO_REPLICA_SET: &str = "rs0";

pub type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

pub struct RoundTrip<'a, T> {
    pub engine: DatabaseEngine,
    pub label: &'a str,
    pub source_tables: usize,
    pub artifact_tables: usize,
    pub restored_tables: usize,
    pub source_before: &'a T,
    pub restored: &'a T,
    pub source_after: &'a T,
}

pub fn assert_round_trip<T: PartialEq + Debug>(case: &RoundTrip<'_, T>) {
    let (engine, label) = (case.engine, case.label);
    for operation in [Operation::FullBackup, Operation::Verify, Operation::Restore] {
        assert_eq!(
            engine.capability(operation).support,
            Support::Supported,
            "{engine} ({label}) passou pelo contrato, mas a matriz não declara {operation:?}"
        );
    }
    assert!(
        case.source_tables > 0,
        "{engine} ({label}): a fixture não tem tabelas"
    );
    assert_eq!(
        case.artifact_tables, case.source_tables,
        "{engine} ({label}): o artefato lido sem servidor não tem as tabelas da origem"
    );
    assert_eq!(
        case.restored_tables, case.source_tables,
        "{engine} ({label}): o destino restaurado não tem as tabelas da origem"
    );
    assert_eq!(
        case.restored, case.source_before,
        "{engine} ({label}): o restore não reproduz os dados da origem"
    );
    assert_eq!(
        case.source_after, case.source_before,
        "{engine} ({label}): o backup ou o restore alterou a origem"
    );
}

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

pub struct MysqlTestDb {
    host: String,
    port: u16,
    password: String,
    _container: ContainerAsync<GenericImage>,
}

impl MysqlTestDb {
    pub async fn start() -> TestResult<Self> {
        let password = random_password();
        let container = GenericImage::new(MYSQL_IMAGE, MYSQL_TAG)
            .with_exposed_port(3306.tcp())
            .with_wait_for(WaitFor::message_on_stderr(MYSQL_READY))
            .with_env_var("MYSQL_ROOT_PASSWORD", &password)
            .with_env_var("MYSQL_DATABASE", MYSQL_DATABASE)
            .start()
            .await
            .map_err(|error| {
                format!("falha ao iniciar o container {MYSQL_IMAGE}:{MYSQL_TAG}: {error}")
            })?;
        let host = container.get_host().await?.to_string();
        let port = container.get_host_port_ipv4(3306.tcp()).await?;
        let db = Self {
            host,
            port,
            password,
            _container: container,
        };
        db.conn(MYSQL_DATABASE).await?.disconnect().await?;
        Ok(db)
    }

    pub async fn start_seeded() -> TestResult<Self> {
        let db = Self::start().await?;
        let mut conn = db.conn(MYSQL_DATABASE).await?;
        for statement in MYSQL_SEED_SQL.split(";\n").filter(|s| !s.trim().is_empty()) {
            conn.query_drop(statement).await.map_err(|error| {
                format!("falha ao aplicar a fixture mysql/seed.sql ({statement}): {error}")
            })?;
        }
        conn.disconnect().await?;
        Ok(db)
    }

    pub async fn conn(&self, database: &str) -> TestResult<mysql_async::Conn> {
        let opts = mysql_async::OptsBuilder::default()
            .ip_or_hostname(self.host.as_str())
            .tcp_port(self.port)
            .user(Some(MYSQL_USER))
            .pass(Some(self.password.as_str()))
            .db_name(Some(database))
            .prefer_socket(false);
        let mut last = None;
        for _ in 0..30 {
            match mysql_async::Conn::new(opts.clone()).await {
                Ok(conn) => return Ok(conn),
                Err(error) => last = Some(error),
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        Err(format!(
            "falha ao conectar no MySQL de teste em {}:{}: {last:?}",
            self.host, self.port
        )
        .into())
    }

    pub async fn execute(&self, database: &str, sql: &str) -> TestResult {
        let mut conn = self.conn(database).await?;
        conn.query_drop(sql).await?;
        conn.disconnect().await?;
        Ok(())
    }

    pub async fn rows(&self, database: &str, sql: &str) -> TestResult<Vec<Vec<String>>> {
        let mut conn = self.conn(database).await?;
        let rows: Vec<mysql_async::Row> = conn.query(sql).await?;
        conn.disconnect().await?;
        Ok(rows
            .into_iter()
            .map(|row| {
                row.unwrap()
                    .into_iter()
                    .map(|value| match value {
                        mysql_async::Value::NULL => "NULL".to_owned(),
                        mysql_async::Value::Bytes(bytes) => {
                            String::from_utf8_lossy(&bytes).into_owned()
                        }
                        other => other.as_sql(true),
                    })
                    .collect()
            })
            .collect())
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn user(&self) -> &str {
        MYSQL_USER
    }

    pub fn database(&self) -> &str {
        MYSQL_DATABASE
    }

    pub fn password(&self) -> &str {
        &self.password
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct CollectionSnapshot {
    pub options: Document,
    pub indexes: Vec<Document>,
    pub documents: Vec<Document>,
}

#[derive(Debug, Default, PartialEq)]
pub struct ViewSnapshot {
    pub options: Document,
    pub documents: Vec<Document>,
}

#[derive(Debug, Default, PartialEq)]
pub struct MongoSnapshot {
    pub collections: BTreeMap<String, CollectionSnapshot>,
    pub views: BTreeMap<String, ViewSnapshot>,
}

pub struct MongoTestDb {
    host: String,
    port: u16,
    password: String,
    tag: String,
    _container: ContainerAsync<GenericImage>,
}

impl MongoTestDb {
    pub async fn start_seeded() -> TestResult<Self> {
        let tag = mongo_tag();
        let password = random_password();
        let container = GenericImage::new(MONGO_IMAGE, tag.as_str())
            .with_exposed_port(27017.tcp())
            .with_wait_for(WaitFor::message_on_stdout(MONGO_INIT_DONE))
            .with_env_var("MONGO_INITDB_ROOT_USERNAME", MONGO_USER)
            .with_env_var("MONGO_INITDB_ROOT_PASSWORD", &password)
            .with_env_var("MONGO_INITDB_DATABASE", MONGO_DATABASE)
            .with_copy_to(
                "/docker-entrypoint-initdb.d/seed.js",
                MONGO_SEED_JS.as_bytes().to_vec(),
            )
            .start()
            .await
            .map_err(|error| {
                format!("falha ao iniciar o container {MONGO_IMAGE}:{tag}: {error}")
            })?;
        let host = container.get_host().await?.to_string();
        let port = container.get_host_port_ipv4(27017.tcp()).await?;
        let db = Self {
            host,
            port,
            password,
            tag,
            _container: container,
        };
        db.client().await?;
        Ok(db)
    }

    pub async fn start_replica_set_seeded() -> TestResult<Self> {
        let tag = mongo_tag();
        let password = random_password();
        let container = GenericImage::new(MONGO_IMAGE, tag.as_str())
            .with_exposed_port(27017.tcp())
            .with_wait_for(WaitFor::message_on_stdout(MONGO_READY))
            .with_cmd(["--replSet", MONGO_REPLICA_SET, "--bind_ip_all"])
            .with_copy_to("/seed.js", MONGO_SEED_JS.as_bytes().to_vec())
            .start()
            .await
            .map_err(|error| {
                format!("falha ao iniciar o container {MONGO_IMAGE}:{tag}: {error}")
            })?;
        let host = container.get_host().await?.to_string();
        let port = container.get_host_port_ipv4(27017.tcp()).await?;

        let anonymous = MongoClient::with_options(mongo_options(&host, port))?;
        let admin = anonymous.database(MONGO_AUTH_DATABASE);
        admin
            .run_command(doc! {
                "replSetInitiate": {
                    "_id": MONGO_REPLICA_SET,
                    "members": [{ "_id": 0, "host": "127.0.0.1:27017" }],
                },
            })
            .await?;
        let mut writable = false;
        for _ in 0..60 {
            let hello = admin.run_command(doc! { "hello": 1 }).await?;
            if hello.get_bool("isWritablePrimary").unwrap_or(false) {
                writable = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        if !writable {
            return Err("o replica set de teste não elegeu um primário".into());
        }
        admin
            .run_command(doc! {
                "createUser": MONGO_USER,
                "pwd": &password,
                "roles": ["root"],
            })
            .await?;
        let mut seeding = container
            .exec(ExecCommand::new([
                "mongosh",
                "--quiet",
                "mongodb://127.0.0.1:27017/backup_test",
                "/seed.js",
            ]))
            .await?;
        let output = seeding.stdout_to_vec().await?;
        let errors = seeding.stderr_to_vec().await?;
        if seeding.exit_code().await? != Some(0) {
            return Err(format!(
                "falha ao aplicar a fixture mongodb/seed.js: {} {}",
                String::from_utf8_lossy(&output),
                String::from_utf8_lossy(&errors)
            )
            .into());
        }
        anonymous.shutdown().await;

        let db = Self {
            host,
            port,
            password,
            tag,
            _container: container,
        };
        db.client().await?;
        Ok(db)
    }

    pub async fn client(&self) -> TestResult<MongoClient> {
        let mut options = mongo_options(&self.host, self.port);
        options.credential = Some(
            Credential::builder()
                .username(MONGO_USER.to_owned())
                .password(self.password.clone())
                .source(MONGO_AUTH_DATABASE.to_owned())
                .build(),
        );
        let client = MongoClient::with_options(options)?;
        let mut last = None;
        for _ in 0..30 {
            match client
                .database(MONGO_AUTH_DATABASE)
                .run_command(doc! { "ping": 1 })
                .await
            {
                Ok(_) => return Ok(client),
                Err(error) => last = Some(error),
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        Err(format!(
            "falha ao conectar no MongoDB de teste em {}:{}: {last:?}",
            self.host, self.port
        )
        .into())
    }

    pub async fn snapshot(&self, database: &str) -> TestResult<MongoSnapshot> {
        let client = self.client().await?;
        let db = client.database(database);
        let listed = db.run_command(doc! { "listCollections": 1 }).await?;
        let mut snapshot = MongoSnapshot::default();
        for entry in listed.get_document("cursor")?.get_array("firstBatch")? {
            let entry = entry
                .as_document()
                .ok_or("entrada de listCollections inválida")?;
            let name = entry.get_str("name")?;
            if name.starts_with("system.") {
                continue;
            }
            let options = entry.get_document("options").cloned().unwrap_or_default();
            let collection = db.collection::<Document>(name);
            let mut cursor = collection.find(doc! {}).sort(doc! { "_id": 1 }).await?;
            let mut documents = Vec::new();
            while cursor.advance().await? {
                documents.push(cursor.deserialize_current()?);
            }
            if entry.get_str("type").is_ok_and(|kind| kind == "view") {
                snapshot
                    .views
                    .insert(name.to_owned(), ViewSnapshot { options, documents });
                continue;
            }
            let indexes = db.run_command(doc! { "listIndexes": name }).await?;
            let mut specs = Vec::new();
            for index in indexes.get_document("cursor")?.get_array("firstBatch")? {
                let mut spec = index.as_document().ok_or("índice inválido")?.clone();
                spec.remove("v");
                spec.remove("ns");
                specs.push(spec);
            }
            snapshot.collections.insert(
                name.to_owned(),
                CollectionSnapshot {
                    options,
                    indexes: specs,
                    documents,
                },
            );
        }
        client.shutdown().await;
        Ok(snapshot)
    }

    pub async fn database_exists(&self, database: &str) -> TestResult<bool> {
        let client = self.client().await?;
        let names = client.database(database).list_collection_names().await?;
        client.shutdown().await;
        Ok(!names.is_empty())
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn user(&self) -> &str {
        MONGO_USER
    }

    pub fn password(&self) -> &str {
        &self.password
    }

    pub fn database(&self) -> &str {
        MONGO_DATABASE
    }

    pub fn auth_database(&self) -> &str {
        MONGO_AUTH_DATABASE
    }

    pub fn tag(&self) -> &str {
        &self.tag
    }
}

fn mongo_tag() -> String {
    std::env::var(MONGO_TAG_VARIABLE)
        .ok()
        .filter(|tag| !tag.is_empty())
        .unwrap_or_else(|| MONGO_DEFAULT_TAG.to_owned())
}

fn mongo_options(host: &str, port: u16) -> ClientOptions {
    ClientOptions::builder()
        .hosts(vec![ServerAddress::Tcp {
            host: host.to_owned(),
            port: Some(port),
        }])
        .direct_connection(true)
        .server_selection_timeout(Duration::from_secs(10))
        .build()
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
