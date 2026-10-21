#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;
#[path = "../../../crates/database-adapters/tests/support/mod.rs"]
mod support;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use common::{Workspace, sha256_hex, text};
use support::{E2E_SQL, PostgresTestDb, TestResult};
use tokio::sync::Mutex;
use tokio_postgres::{Client, NoTls};

static SERIAL: Mutex<()> = Mutex::const_new(());

const TEST_TIMEOUT: Duration = Duration::from_secs(300);
const SYSTEM_DATABASES: [&str; 4] = ["postgres", "template0", "template1", "backup_test"];

struct Session {
    name: &'static str,
    secrets: Vec<String>,
    steps: RefCell<Vec<(String, String)>>,
}

impl Session {
    fn new(name: &'static str, secrets: &[&str]) -> Self {
        Self {
            name,
            secrets: secrets.iter().map(|secret| (*secret).to_owned()).collect(),
            steps: RefCell::new(Vec::new()),
        }
    }

    fn run(&self, step: &str, command: &mut Command) -> Output {
        let output = command
            .output()
            .unwrap_or_else(|error| panic!("{step}: a CLI não executou: {error}"));
        self.record(step, &output);
        output
    }

    fn record(&self, step: &str, output: &Output) {
        let combined = text(output);
        for secret in &self.secrets {
            assert!(
                !combined.contains(secret.as_str()),
                "{step}: a saída da CLI contém um segredo"
            );
        }
        self.steps.borrow_mut().push((
            step.to_owned(),
            format!(
                "exit: {:?}\n--- stdout ---\n{}\n--- stderr ---\n{}",
                output.status.code(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ),
        ));
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            return;
        }
        let base = std::env::var_os("DBU_E2E_LOG_DIR").map_or_else(
            || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/e2e-logs"),
            PathBuf::from,
        );
        let dir = base.join(self.name);
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        for (index, (step, body)) in self.steps.borrow().iter().enumerate() {
            let file = dir.join(format!("{index:02}-{}.log", sanitize(step)));
            let _ = std::fs::write(file, body);
        }
    }
}

fn sanitize(step: &str) -> String {
    step.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn native_required() -> bool {
    std::env::var_os("DBU_REQUIRE_NATIVE").is_some()
}

fn native_available() -> bool {
    Command::new("pg_dump")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

fn runners() -> Vec<&'static str> {
    let mut runners = vec!["docker"];
    if native_required() || native_available() {
        runners.push("native");
    } else {
        eprintln!("pg_dump nativo ausente: cenários com --runner native ignorados");
    }
    runners
}

fn connection(command: &mut Command, db: &PostgresTestDb, port: u16) {
    command
        .args(["--host", db.host()])
        .args(["--port", &port.to_string()])
        .args(["--user", db.user()]);
}

fn backup(ws: &Workspace, db: &PostgresTestDb, database: &str, runner: &str) -> Command {
    backup_on(ws, db, db.port(), database, runner)
}

fn backup_on(
    ws: &Workspace,
    db: &PostgresTestDb,
    port: u16,
    database: &str,
    runner: &str,
) -> Command {
    let mut command = ws.cli(db.password());
    command.args([
        "--log-level",
        "debug",
        "--json",
        "backup",
        "run",
        "--runner",
        runner,
        "--database",
        database,
    ]);
    connection(&mut command, db, port);
    command.arg("--output-dir").arg(&ws.out);
    command
}

fn restore(
    ws: &Workspace,
    db: &PostgresTestDb,
    file: &Path,
    target: &str,
    runner: &str,
) -> Command {
    restore_on(ws, db, db.port(), file, target, runner)
}

fn restore_on(
    ws: &Workspace,
    db: &PostgresTestDb,
    port: u16,
    file: &Path,
    target: &str,
    runner: &str,
) -> Command {
    let mut command = ws.cli(db.password());
    command.args([
        "--log-level",
        "debug",
        "--json",
        "restore",
        "run",
        "--runner",
        runner,
        "--target-database",
        target,
    ]);
    connection(&mut command, db, port);
    command.arg("--file").arg(file);
    command
}

fn json(output: &Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("stdout não é JSON ({error}): {}", text(output)))
}

fn dump_of(output: &Output) -> PathBuf {
    let body = json(output);
    PathBuf::from(
        body["file"]
            .as_str()
            .expect("campo file no resultado do backup"),
    )
}

async fn connect(db: &PostgresTestDb, database: &str) -> TestResult<Client> {
    let (client, connection) = tokio_postgres::Config::new()
        .host(db.host())
        .port(db.port())
        .user(db.user())
        .password(db.password())
        .dbname(database)
        .connect(NoTls)
        .await?;
    tokio::spawn(connection);
    Ok(client)
}

async fn user_databases(db: &PostgresTestDb) -> TestResult<Vec<String>> {
    let rows = connect(db, "postgres")
        .await?
        .query("SELECT datname FROM pg_database ORDER BY datname", &[])
        .await?;
    Ok(rows
        .iter()
        .map(|row| row.get::<_, String>(0))
        .filter(|name| !SYSTEM_DATABASES.contains(&name.as_str()))
        .collect())
}

const EXCLUDED_SCHEMAS: &str = "('pg_catalog', 'information_schema', 'pg_toast')";

async fn lines(client: &Client, sql: &str) -> TestResult<Vec<String>> {
    let rows = client.query(sql, &[]).await?;
    Ok(rows
        .iter()
        .map(|row| {
            (0..row.len())
                .map(|index| row.get::<_, Option<String>>(index).unwrap_or_default())
                .collect::<Vec<_>>()
                .join(" | ")
        })
        .collect())
}

async fn snapshot(db: &PostgresTestDb, database: &str) -> TestResult<Vec<String>> {
    let client = connect(db, database).await?;
    let mut snapshot = Vec::new();
    let catalog = [
        (
            "column",
            format!(
                "SELECT table_schema, table_name, column_name, data_type, is_nullable, column_default
                 FROM information_schema.columns WHERE table_schema NOT IN {EXCLUDED_SCHEMAS}
                 ORDER BY table_schema, table_name, ordinal_position"
            ),
        ),
        (
            "constraint",
            format!(
                "SELECT conrelid::regclass::text, conname, pg_get_constraintdef(oid)
                 FROM pg_constraint WHERE connamespace::regnamespace::text NOT IN {EXCLUDED_SCHEMAS}
                 ORDER BY 1, 2"
            ),
        ),
        (
            "index",
            format!(
                "SELECT schemaname, indexname, indexdef FROM pg_indexes
                 WHERE schemaname NOT IN {EXCLUDED_SCHEMAS} ORDER BY 1, 2"
            ),
        ),
        (
            "view",
            format!(
                "SELECT schemaname, viewname, definition FROM pg_views
                 WHERE schemaname NOT IN {EXCLUDED_SCHEMAS} ORDER BY 1, 2"
            ),
        ),
        (
            "function",
            format!(
                "SELECT n.nspname, p.proname, pg_get_functiondef(p.oid)
                 FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace
                 WHERE n.nspname NOT IN {EXCLUDED_SCHEMAS} ORDER BY 1, 2"
            ),
        ),
        (
            "sequence",
            format!(
                "SELECT schemaname, sequencename, last_value::text FROM pg_sequences
                 WHERE schemaname NOT IN {EXCLUDED_SCHEMAS} ORDER BY 1, 2"
            ),
        ),
    ];
    for (kind, sql) in catalog {
        for line in lines(&client, &sql).await? {
            snapshot.push(format!("{kind}: {line}"));
        }
    }
    let tables = client
        .query(
            &format!(
                "SELECT format('%I.%I', table_schema, table_name) FROM information_schema.tables
                 WHERE table_type = 'BASE TABLE' AND table_schema NOT IN {EXCLUDED_SCHEMAS}
                 ORDER BY 1"
            ),
            &[],
        )
        .await?;
    for table in tables {
        let name: String = table.get(0);
        let row = client
            .query_one(
                &format!(
                    "SELECT count(*)::text, md5(coalesce(string_agg(t::text, '|' ORDER BY t::text), ''))
                     FROM {name} t"
                ),
                &[],
            )
            .await?;
        let count: String = row.get(0);
        let hash: String = row.get(1);
        snapshot.push(format!("data: {name} | {count} linhas | md5 {hash}"));
    }
    Ok(snapshot)
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .expect("metadata")
        .permissions()
        .mode()
        & 0o777
}

fn files_containing(dir: &Path, needle: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(files_containing(&path, needle));
        } else if std::fs::read(&path)
            .is_ok_and(|bytes| bytes.windows(needle.len()).any(|w| w == needle.as_bytes()))
        {
            found.push(path);
        }
    }
    found
}

fn assert_no_residue(ws: &Workspace) {
    let containers = Command::new("docker")
        .args(["ps", "-aq", "--filter", "name=^dbu-pg-"])
        .output()
        .expect("docker ps");
    assert!(
        containers.stdout.is_empty(),
        "containers do runner ficaram para trás: {}",
        String::from_utf8_lossy(&containers.stdout)
    );
    let leftovers: Vec<_> = std::fs::read_dir(&ws.tmp)
        .expect("tmp do workspace")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        leftovers.is_empty(),
        "temporários ficaram para trás: {leftovers:?}"
    );
    let partials: Vec<_> = std::fs::read_dir(&ws.out)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| name.ends_with(".partial"))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        partials.is_empty(),
        "arquivos parciais ficaram para trás: {partials:?}"
    );
}

async fn guarded<F>(test: F) -> TestResult
where
    F: std::future::Future<Output = TestResult>,
{
    let _serial = SERIAL.lock().await;
    tokio::time::timeout(TEST_TIMEOUT, test)
        .await
        .map_err(|_| format!("cenário E2E excedeu {}s", TEST_TIMEOUT.as_secs()))?
}

#[tokio::test]
async fn full_cycle_restores_identical_data_on_another_server() -> TestResult {
    guarded(async {
        let source = PostgresTestDb::start_with("e2e.sql", E2E_SQL).await?;
        let target = PostgresTestDb::start().await?;
        let expected = snapshot(&source, source.database()).await?;
        assert!(
            expected
                .iter()
                .any(|line| line.starts_with("data: vendas.pedidos | 800 linhas"))
        );

        for runner in runners() {
            let session = Session::new("full-cycle", &[source.password(), target.password()]);
            let ws = Workspace::new()?;

            let backed_up = session.run(
                &format!("{runner}-backup"),
                &mut backup(&ws, &source, source.database(), runner),
            );
            assert_eq!(backed_up.status.code(), Some(0), "{}", text(&backed_up));
            let dump = dump_of(&backed_up);
            #[cfg(unix)]
            {
                assert_eq!(mode(&ws.out), 0o700);
                assert_eq!(mode(&dump), 0o600);
                assert_eq!(mode(&database_backup_storage_meta(&dump)), 0o600);
            }

            let verified = session.run(
                &format!("{runner}-verify"),
                ws.cli(source.password())
                    .args(["--json", "backup", "verify", "--runner", runner, "--file"])
                    .arg(&dump),
            );
            assert_eq!(verified.status.code(), Some(0), "{}", text(&verified));
            assert_eq!(json(&verified)["checksum_verified"], true);
            assert_eq!(json(&verified)["sha256"], sha256_hex(&dump).as_str());

            let target_name = format!("restaurado_{runner}");
            let restored = session.run(
                &format!("{runner}-restore"),
                &mut restore(&ws, &target, &dump, &target_name, runner),
            );
            assert_eq!(restored.status.code(), Some(0), "{}", text(&restored));
            assert_eq!(json(&restored)["checksum_verified"], true);
            assert_eq!(
                snapshot(&target, &target_name).await?,
                expected,
                "runner {runner}"
            );

            let backups = session.run(
                &format!("{runner}-backup-list"),
                ws.cli(source.password()).args(["--json", "backup", "list"]),
            );
            let restores = session.run(
                &format!("{runner}-restore-list"),
                ws.cli(source.password())
                    .args(["--json", "restore", "list"]),
            );
            let backups = json(&backups)["backups"].clone();
            let restores = json(&restores)["restores"].clone();
            assert_eq!(backups.as_array().map(Vec::len), Some(1));
            assert_eq!(backups[0]["status"], "succeeded");
            assert_eq!(backups[0]["file"], dump.display().to_string());
            assert_eq!(restores.as_array().map(Vec::len), Some(1));
            assert_eq!(restores[0]["status"], "succeeded");
            assert_eq!(restores[0]["target_database"], target_name.as_str());

            for secret in [source.password(), target.password()] {
                let leaks = [
                    files_containing(&ws.data, secret),
                    files_containing(&ws.out, secret),
                ]
                .concat();
                assert!(leaks.is_empty(), "segredo gravado em {leaks:?}");
            }
            assert_no_residue(&ws);
        }
        Ok(())
    })
    .await
}

fn database_backup_storage_meta(dump: &Path) -> PathBuf {
    let mut path = dump.as_os_str().to_owned();
    path.push(".meta.json");
    PathBuf::from(path)
}

fn closed_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .map(|addr| addr.port())
        .expect("porta livre")
}

#[tokio::test]
async fn connection_failures_have_distinct_exit_codes() -> TestResult {
    guarded(async {
        let db = PostgresTestDb::start_with("e2e.sql", E2E_SQL).await?;
        let wrong = "senha-errada-e2e";
        let session = Session::new("connection-failures", &[db.password(), wrong]);
        let ws = Workspace::new()?;
        let ok = session.run("backup", &mut backup(&ws, &db, db.database(), "docker"));
        assert_eq!(ok.status.code(), Some(0), "{}", text(&ok));
        let dump = dump_of(&ok);
        let port = closed_port();

        let unreachable = backup_on(&ws, &db, port, db.database(), "docker");
        let mut auth = backup(&ws, &db, db.database(), "docker");
        auth.env("DBU_PG_PASSWORD", wrong);
        let restore_unreachable = restore_on(&ws, &db, port, &dump, "nunca_criado", "docker");
        let mut restore_auth = restore(&ws, &db, &dump, "nunca_criado", "docker");
        restore_auth.env("DBU_PG_PASSWORD", wrong);

        let cases: Vec<(&str, Command, i32)> = vec![
            ("backup-unreachable", unreachable, 5),
            ("backup-auth", auth, 3),
            (
                "backup-missing-db",
                backup(&ws, &db, "banco_inexistente", "docker"),
                4,
            ),
            ("restore-unreachable", restore_unreachable, 5),
            ("restore-auth", restore_auth, 3),
        ];
        for (step, mut command, code) in cases {
            let output = session.run(step, &mut command);
            assert_eq!(
                output.status.code(),
                Some(code),
                "{step}: {}",
                text(&output)
            );
            assert_eq!(json(&output)["code"], code, "{step}");
        }
        assert!(user_databases(&db).await?.is_empty());
        assert_no_residue(&ws);
        Ok(())
    })
    .await
}

#[cfg(unix)]
fn private_copy(from: &[u8], to: &Path) -> TestResult {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(to, from)?;
    std::fs::set_permissions(to, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn corrupted_and_partial_archives_are_rejected() -> TestResult {
    guarded(async {
        let source = PostgresTestDb::start_with("e2e.sql", E2E_SQL).await?;
        let target = PostgresTestDb::start().await?;
        let session = Session::new(
            "corrupted-archives",
            &[source.password(), target.password()],
        );
        let ws = Workspace::new()?;
        let ok = session.run(
            "backup",
            &mut backup(&ws, &source, source.database(), "docker"),
        );
        assert_eq!(ok.status.code(), Some(0), "{}", text(&ok));
        let dump = dump_of(&ok);
        let original = std::fs::read(&dump)?;

        let mut flipped = original.clone();
        let middle = flipped.len() / 2;
        flipped[middle] ^= 0xFF;
        std::fs::write(&dump, &flipped)?;
        let corrupted = session.run(
            "restore-corrupted",
            &mut restore(&ws, &target, &dump, "alvo", "docker"),
        );
        assert_eq!(corrupted.status.code(), Some(10), "{}", text(&corrupted));
        assert!(
            json(&corrupted)["error"]
                .as_str()
                .is_some_and(|e| e.contains("checksum divergente"))
        );
        std::fs::write(&dump, &original)?;

        let truncated = ws.tmp.join("truncado.dump");
        private_copy(&original[..original.len() * 2 / 3], &truncated)?;
        let mut command = restore(&ws, &target, &truncated, "alvo", "docker");
        command.arg("--allow-unverified");
        let output = session.run("restore-truncated", &mut command);
        assert!(
            matches!(output.status.code(), Some(10 | 11)),
            "{}",
            text(&output)
        );
        std::fs::remove_file(&truncated)?;

        let partial = ws.out.join("pendente.dump.partial");
        private_copy(&original, &partial)?;
        let verify = session.run(
            "verify-partial",
            ws.cli(source.password())
                .args(["--json", "backup", "verify", "--runner", "docker", "--file"])
                .arg(&partial),
        );
        assert_eq!(verify.status.code(), Some(10), "{}", text(&verify));
        let restore_partial = session.run(
            "restore-partial",
            &mut restore(&ws, &target, &partial, "alvo", "docker"),
        );
        assert_eq!(
            restore_partial.status.code(),
            Some(10),
            "{}",
            text(&restore_partial)
        );
        std::fs::remove_file(&partial)?;

        let missing_target = session.run(
            "restore-without-target",
            ws.cli(target.password())
                .args(["restore", "run", "--user", target.user(), "--file"])
                .arg(&dump),
        );
        assert_eq!(
            missing_target.status.code(),
            Some(2),
            "{}",
            text(&missing_target)
        );

        assert!(user_databases(&target).await?.is_empty());
        assert_no_residue(&ws);
        Ok(())
    })
    .await
}

#[cfg(unix)]
#[tokio::test]
async fn interrupted_operations_leave_no_partial_results() -> TestResult {
    guarded(async {
        let source = PostgresTestDb::start().await?;
        source
            .client()
            .batch_execute(
                "CREATE TABLE carga AS
                 SELECT g AS id, md5(g::text) || md5((g * 7)::text) AS a,
                        md5((g * 13)::text) || md5((g * 31)::text) AS b
                 FROM generate_series(1, 600000) g",
            )
            .await?;
        let session = Session::new("interrupted", &[source.password()]);
        let ws = Workspace::new()?;

        let child = backup(&ws, &source, source.database(), "docker")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        loop {
            let has_partial = std::fs::read_dir(&ws.out).is_ok_and(|entries| {
                entries
                    .flatten()
                    .any(|entry| entry.file_name().to_string_lossy().ends_with(".partial"))
            });
            if has_partial {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "o backup não começou a gravar"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let interrupted = Command::new("kill")
            .args(["-INT", &child.id().to_string()])
            .status()?;
        assert!(interrupted.success());
        let output = tokio::task::spawn_blocking(move || child.wait_with_output()).await??;
        session.record("backup-interrupted", &output);
        assert_eq!(output.status.code(), Some(9), "{}", text(&output));
        let files: Vec<_> = std::fs::read_dir(&ws.out)?
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            files.is_empty(),
            "o backup interrompido deixou arquivos: {files:?}"
        );
        let status: String =
            ws.history()
                .query_row("SELECT status FROM backup_executions", [], |row| row.get(0))?;
        assert_eq!(status, "canceled");
        assert_no_residue(&ws);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn destructive_restore_requires_confirmation() -> TestResult {
    guarded(async {
        let source = PostgresTestDb::start_with("e2e.sql", E2E_SQL).await?;
        let target = PostgresTestDb::start().await?;
        let session = Session::new("confirmation", &[source.password(), target.password()]);
        let ws = Workspace::new()?;
        let ok = session.run(
            "backup",
            &mut backup(&ws, &source, source.database(), "docker"),
        );
        assert_eq!(ok.status.code(), Some(0), "{}", text(&ok));
        let dump = dump_of(&ok);

        target
            .client()
            .batch_execute("CREATE DATABASE existente")
            .await?;
        connect(&target, "existente")
            .await?
            .batch_execute("CREATE TABLE marcador (id INT); INSERT INTO marcador VALUES (42)")
            .await?;

        let refused = session.run(
            "without-overwrite",
            &mut restore(&ws, &target, &dump, "existente", "docker"),
        );
        assert_eq!(refused.status.code(), Some(12), "{}", text(&refused));

        let mut wrong = restore(&ws, &target, &dump, "existente", "docker");
        wrong.args(["--overwrite", "--confirm", "outro"]);
        let wrong = session.run("wrong-confirmation", &mut wrong);
        assert_eq!(wrong.status.code(), Some(12), "{}", text(&wrong));
        assert_eq!(user_databases(&target).await?, vec!["existente".to_owned()]);

        let mut confirmed = restore(&ws, &target, &dump, "existente", "docker");
        confirmed.args(["--overwrite", "--confirm", "existente"]);
        let confirmed = session.run("confirmed", &mut confirmed);
        assert_eq!(confirmed.status.code(), Some(0), "{}", text(&confirmed));
        let previous = json(&confirmed)["previous_database"]
            .as_str()
            .expect("banco anterior")
            .to_owned();
        let marker: i32 = connect(&target, &previous)
            .await?
            .query_one("SELECT id FROM marcador", &[])
            .await?
            .get(0);
        assert_eq!(marker, 42);
        assert_eq!(
            snapshot(&target, "existente").await?,
            snapshot(&source, source.database()).await?
        );
        assert_no_residue(&ws);
        Ok(())
    })
    .await
}

#[cfg(unix)]
#[tokio::test]
async fn hostile_identifiers_are_not_executed() -> TestResult {
    guarded(async {
        let source = PostgresTestDb::start().await?;
        let target = PostgresTestDb::start().await?;
        let session = Session::new(
            "hostile-identifiers",
            &[source.password(), target.password()],
        );
        let ws = Workspace::new()?;
        let sentinel = ws.tmp.join("pwned");
        let sentinel_text = sentinel.display().to_string();
        let source_name = format!("fonte; touch {sentinel_text}");
        assert!(
            source_name.len() <= 63,
            "nome longo demais para o PostgreSQL"
        );
        source
            .client()
            .batch_execute(&format!("CREATE DATABASE {}", quote_ident(&source_name)))
            .await?;
        connect(&source, &source_name)
            .await?
            .batch_execute("CREATE TABLE prova (valor TEXT); INSERT INTO prova VALUES ('ok')")
            .await?;

        let backed_up = session.run(
            "backup-hostile-source",
            &mut backup(&ws, &source, &source_name, "docker"),
        );
        assert_eq!(backed_up.status.code(), Some(0), "{}", text(&backed_up));
        let dump = dump_of(&backed_up);
        assert!(
            !dump
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .contains(' ')
        );

        let targets = [
            "$(touch pwned2)`touch pwned3`".to_owned(),
            "aspas \"duplas\" e 'simples'".to_owned(),
            "dbname=postgres".to_owned(),
            "-h outro --help".to_owned(),
        ];
        let option_like = session.run(
            "restore-option-like-separate-arg",
            &mut restore(&ws, &target, &dump, "-h outro --help", "docker"),
        );
        assert_eq!(option_like.status.code(), Some(2), "{}", text(&option_like));
        assert!(user_databases(&target).await?.is_empty());

        for (index, name) in targets.iter().enumerate() {
            let mut command = ws.cli(target.password());
            command
                .args(["--json", "restore", "run", "--runner", "docker"])
                .arg(format!("--target-database={name}"));
            connection(&mut command, &target, target.port());
            command.arg("--file").arg(&dump);
            let output = session.run(&format!("restore-hostile-{index}"), &mut command);
            assert_eq!(output.status.code(), Some(0), "{name}: {}", text(&output));
            let value: String = connect(&target, name)
                .await?
                .query_one("SELECT valor FROM prova", &[])
                .await?
                .get(0);
            assert_eq!(value, "ok", "{name}");
        }
        let maintenance_tables: i64 = connect(&target, "postgres")
            .await?
            .query_one(
                "SELECT count(*) FROM information_schema.tables WHERE table_name = 'prova'",
                &[],
            )
            .await?
            .get(0);
        assert_eq!(
            maintenance_tables, 0,
            "o restore escreveu no banco postgres"
        );
        for file in [sentinel, ws.tmp.join("pwned2"), ws.tmp.join("pwned3")] {
            assert!(
                !file.exists(),
                "comando injetado foi executado: {}",
                file.display()
            );
        }
        assert!(!Path::new("pwned2").exists() && !Path::new("pwned3").exists());
        assert_no_residue(&ws);
        Ok(())
    })
    .await
}

fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

#[tokio::test]
async fn verify_and_restore_without_history() -> TestResult {
    guarded(async {
        let source = PostgresTestDb::start_with("e2e.sql", E2E_SQL).await?;
        let target = PostgresTestDb::start().await?;
        let session = Session::new("without-history", &[source.password(), target.password()]);
        let origin = Workspace::new()?;
        let ok = session.run(
            "backup",
            &mut backup(&origin, &source, source.database(), "docker"),
        );
        assert_eq!(ok.status.code(), Some(0), "{}", text(&ok));
        let original = dump_of(&ok);

        let fresh = Workspace::new()?;
        std::fs::create_dir(&fresh.out)?;
        let dump = fresh.out.join(original.file_name().expect("nome do dump"));
        let manifest = database_backup_storage_meta(&dump);
        std::fs::copy(&original, &dump)?;
        std::fs::copy(database_backup_storage_meta(&original), &manifest)?;

        let verify = |step: &str| {
            session.run(
                step,
                fresh
                    .cli(source.password())
                    .args(["--json", "backup", "verify", "--runner", "docker", "--file"])
                    .arg(&dump),
            )
        };
        let verified = verify("verify-without-history");
        assert_eq!(verified.status.code(), Some(0), "{}", text(&verified));
        let report = json(&verified);
        assert_eq!(report["checksum_verified"], true);
        assert_eq!(report["manifest_version"], 1);
        assert_eq!(report["engine"], "postgresql");
        assert_eq!(report["tool_version"], env!("CARGO_PKG_VERSION"));

        let restored = session.run(
            "restore-without-history",
            &mut restore(&fresh, &target, &dump, "sem_historico", "docker"),
        );
        assert_eq!(restored.status.code(), Some(0), "{}", text(&restored));
        assert_eq!(json(&restored)["checksum_verified"], true);
        assert_eq!(
            snapshot(&target, "sem_historico").await?,
            snapshot(&source, source.database()).await?
        );

        let pristine = std::fs::read(&dump)?;
        let mut flipped = pristine.clone();
        let middle = flipped.len() / 2;
        flipped[middle] ^= 0xFF;
        std::fs::write(&dump, &flipped)?;
        let altered = verify("verify-altered-dump");
        assert_eq!(altered.status.code(), Some(10), "{}", text(&altered));
        assert!(text(&altered).contains("checksum divergente"));
        std::fs::write(&dump, &pristine)?;

        let pristine_manifest = std::fs::read_to_string(&manifest)?;
        let mut forged: serde_json::Value = serde_json::from_str(&pristine_manifest)?;
        forged["sha256"] = serde_json::Value::from("f".repeat(64));
        std::fs::write(&manifest, forged.to_string())?;
        let forged = verify("verify-forged-manifest");
        assert_eq!(forged.status.code(), Some(10), "{}", text(&forged));
        assert!(text(&forged).contains("checksum divergente"));
        std::fs::write(&manifest, &pristine_manifest)?;

        std::fs::remove_file(&dump)?;
        let missing = verify("verify-missing-dump");
        assert_eq!(missing.status.code(), Some(10), "{}", text(&missing));
        assert!(text(&missing).contains("não encontrado"));

        let fresh_history: i64 =
            fresh
                .history()
                .query_row("SELECT count(*) FROM backup_artifacts", [], |row| {
                    row.get(0)
                })?;
        assert_eq!(
            fresh_history, 0,
            "o histórico novo não deveria conhecer o backup"
        );
        assert_no_residue(&fresh);
        Ok(())
    })
    .await
}

#[cfg(unix)]
fn secret_file(dir: &Path, name: &str, value: &str) -> TestResult<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, format!("{value}\n"))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    Ok(path)
}

#[cfg(unix)]
#[tokio::test]
async fn scheduled_style_backup_with_password_file() -> TestResult {
    guarded(async {
        let source = PostgresTestDb::start_with("e2e.sql", E2E_SQL).await?;
        let target = PostgresTestDb::start().await?;
        let session = Session::new("password-file", &[source.password(), target.password()]);
        let ws = Workspace::new()?;
        let secrets = tempfile::tempdir()?;
        let source_file = secret_file(secrets.path(), "origem.pass", source.password())?;
        let target_file = secret_file(secrets.path(), "destino.pass", target.password())?;

        let mut command = backup(&ws, &source, source.database(), "docker");
        command
            .env_remove("DBU_PG_PASSWORD")
            .env("DBU_PG_PASSWORD_FILE", &source_file)
            .stdin(Stdio::null());
        let backed_up = session.run("backup-with-password-file", &mut command);
        assert_eq!(backed_up.status.code(), Some(0), "{}", text(&backed_up));
        let dump = dump_of(&backed_up);

        let mut command = restore(&ws, &target, &dump, "agendado", "docker");
        command
            .env_remove("DBU_PG_PASSWORD")
            .arg("--password-file")
            .arg(&target_file)
            .stdin(Stdio::null());
        let restored = session.run("restore-with-password-file", &mut command);
        assert_eq!(restored.status.code(), Some(0), "{}", text(&restored));
        assert_eq!(
            snapshot(&target, "agendado").await?,
            snapshot(&source, source.database()).await?
        );

        let secret_ref: String =
            ws.history()
                .query_row("SELECT secret_ref FROM connections", [], |row| row.get(0))?;
        assert_eq!(secret_ref, format!("file:{}", source_file.display()));
        for secret in [source.password(), target.password()] {
            let leaks = [
                files_containing(&ws.data, secret),
                files_containing(&ws.out, secret),
            ]
            .concat();
            assert!(leaks.is_empty(), "segredo gravado em {leaks:?}");
        }
        assert_no_residue(&ws);
        Ok(())
    })
    .await
}
