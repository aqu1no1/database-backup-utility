#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use database_backup_adapters::mongo::{
    Admin, MongoTools, MongoVersion, RunnerChoice, Timeouts, ToolError, ToolPaths, inspect_archive,
    test_connection,
};
use database_backup_core::{MongoConnectionConfig, Password, TlsMode};
use mongodb::bson::{Document, doc};
use support::{MongoTestDb, TestResult};
use tokio::io::AsyncReadExt;

const EXOTIC_PASSWORD: &str = "Ünï\"\\ #: 'x' %$ç end\\";

fn config_for(db: &MongoTestDb, database: &str) -> MongoConnectionConfig {
    MongoConnectionConfig::new(
        db.host(),
        db.port(),
        database,
        db.user(),
        db.auth_database(),
        TlsMode::Auto,
    )
    .expect("configuração de teste válida")
}

fn password(db: &MongoTestDb) -> Password {
    Password::new(db.password().to_owned())
}

async fn server_version(db: &MongoTestDb) -> TestResult<MongoVersion> {
    let server = test_connection(
        &config_for(db, db.database()),
        &password(db),
        Timeouts::default(),
    )
    .await?;
    Ok(server.version)
}

async fn dump_to_file(
    tools: &MongoTools,
    config: &MongoConnectionConfig,
    password: &Password,
    path: &Path,
) -> TestResult {
    let mut process = tools.dump(config, password)?;
    let mut stdout = process.take_stdout().ok_or("stdout ausente")?;
    let mut file = tokio::fs::File::create(path).await?;
    tokio::io::copy(&mut stdout, &mut file).await?;
    process.wait().await?;
    Ok(())
}

fn container_volumes(name: &str) -> Vec<String> {
    let output = Command::new("docker")
        .args([
            "inspect",
            "--format",
            "{{range .Mounts}}{{if .Name}}{{.Name}} {{end}}{{end}}",
            name,
        ])
        .output()
        .expect("docker inspect");
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

fn remove_volumes(names: &[String]) {
    for name in names {
        let _ = Command::new("docker").args(["volume", "rm", name]).output();
    }
}

fn container_exists(name: &str) -> bool {
    let output = Command::new("docker")
        .args(["ps", "-aq", "--filter", &format!("name=^{name}$")])
        .output()
        .expect("docker ps");
    !output.stdout.is_empty()
}

async fn available_runners(server: MongoVersion) -> Vec<MongoTools> {
    let mut runners = Vec::new();
    for choice in [RunnerChoice::Docker, RunnerChoice::Native] {
        match MongoTools::detect(choice, server).await {
            Ok(tools) => runners.push(tools),
            Err(error) if choice == RunnerChoice::Native => {
                assert!(
                    std::env::var_os("DBU_REQUIRE_NATIVE_MONGO").is_none(),
                    "ferramentas MongoDB nativas exigidas: {error}"
                );
                eprintln!("runner nativo ignorado: {error}");
            }
            Err(error) => panic!("runner docker indisponível: {error}"),
        }
    }
    runners
}

#[tokio::test]
async fn dump_and_restore_preserve_documents_indexes_and_views_on_every_runner() -> TestResult {
    let db = MongoTestDb::start_seeded().await?;
    let expected = db.snapshot(db.database()).await?;
    assert_eq!(expected.collections.len(), 4);
    assert_eq!(expected.views.len(), 1);
    let admin = Admin::new(
        &config_for(&db, db.database()),
        &password(&db),
        Timeouts::default(),
    );
    let source_contents = admin.contents(db.database()).await?;
    let dir = tempfile::tempdir()?;

    for tools in available_runners(server_version(&db).await?).await {
        let runner = tools.kind().as_str();
        let file = dir.path().join(format!("{runner}.archive"));
        dump_to_file(
            &tools,
            &config_for(&db, db.database()),
            &password(&db),
            &file,
        )
        .await?;

        let info = inspect_archive(&file)?;
        assert_eq!(info.database, db.database(), "{runner}");
        assert_eq!(info.contents, source_contents, "{runner}");
        assert_eq!(
            info.contents
                .documents
                .iter()
                .map(|(name, count)| (name.as_str(), *count))
                .collect::<Vec<_>>(),
            [
                ("audit_log", 2),
                ("customers", 3),
                ("empty_collection", 0),
                ("orders", 4)
            ],
            "{runner}"
        );
        assert!(info.contents.views.contains("customer_totals"), "{runner}");
        assert_eq!(
            info.tool_version.as_deref().map(|v| v.starts_with("100.")),
            Some(true)
        );

        let target = format!("copia_{runner}");
        tools
            .restore(
                &config_for(&db, &target),
                &password(&db),
                &file,
                db.database(),
            )?
            .wait()
            .await?;

        assert_eq!(db.snapshot(&target).await?, expected, "{runner}");
        assert_eq!(admin.contents(&target).await?, info.contents, "{runner}");

        let again = tools
            .restore(
                &config_for(&db, &target),
                &password(&db),
                &file,
                db.database(),
            )?
            .wait()
            .await;
        assert!(
            matches!(
                again,
                Err(ToolError::Failed {
                    tool: "mongorestore",
                    ..
                })
            ),
            "{runner}: {again:?}"
        );
        assert_eq!(
            db.snapshot(&target).await?,
            expected,
            "o segundo restore alterou o destino ({runner})"
        );
        assert_eq!(
            db.snapshot(db.database()).await?,
            expected,
            "a origem mudou"
        );
    }
    Ok(())
}

#[tokio::test]
async fn special_characters_in_the_password_reach_the_tools_intact() -> TestResult {
    let db = MongoTestDb::start_seeded().await?;
    let client = db.client().await?;
    client
        .database(db.database())
        .run_command(doc! {
            "createUser": "exotico",
            "pwd": EXOTIC_PASSWORD,
            "roles": [{ "role": "read", "db": db.database() }],
        })
        .await?;
    let config = MongoConnectionConfig::new(
        db.host(),
        db.port(),
        db.database(),
        "exotico",
        db.database(),
        TlsMode::Auto,
    )?;
    let secret = Password::new(EXOTIC_PASSWORD.to_owned());
    let dir = tempfile::tempdir()?;
    let expected = Admin::new(&config, &secret, Timeouts::default())
        .contents(db.database())
        .await?;

    for tools in available_runners(server_version(&db).await?).await {
        let runner = tools.kind().as_str();
        let file = dir.path().join(format!("{runner}.archive"));

        dump_to_file(&tools, &config, &secret, &file).await?;

        assert_eq!(inspect_archive(&file)?.contents, expected, "{runner}");
    }
    Ok(())
}

#[tokio::test]
async fn password_never_reaches_container_arguments_environment_or_errors() -> TestResult {
    let db = MongoTestDb::start_seeded().await?;
    let client = db.client().await?;
    let big = client.database(db.database()).collection::<Document>("big");
    for chunk in 0..10_i32 {
        let documents: Vec<Document> = (0..10_000_i32)
            .map(|index| doc! { "_id": chunk * 10_000 + index, "payload": "x".repeat(120) })
            .collect();
        big.insert_many(documents).await?;
    }
    let tools = MongoTools::detect(RunnerChoice::Docker, server_version(&db).await?).await?;

    let mut process = tools.dump(&config_for(&db, db.database()), &password(&db))?;
    let container = process
        .container_name()
        .ok_or("o modo docker deve nomear o container")?
        .to_owned();
    let mut stdout = process.take_stdout().ok_or("stdout ausente")?;
    let mut first = [0_u8; 4];
    stdout.read_exact(&mut first).await?;
    let inspected = Command::new("docker")
        .args([
            "inspect",
            "--format",
            "{{json .Args}} {{json .Config.Env}} {{json .Config.Cmd}}",
            &container,
        ])
        .output()?;
    let volumes = container_volumes(&container);
    process.cancel().await;
    remove_volumes(&volumes);

    let inspected = String::from_utf8_lossy(&inspected.stdout);
    assert_eq!(first, database_backup_adapters::mongo::ARCHIVE_MAGIC);
    assert!(inspected.contains("--config="), "{inspected}");
    assert!(!inspected.contains(db.password()), "{inspected}");
    assert!(container.starts_with("dbu-mg-"));
    assert!(!container_exists(&container), "{container} ainda existe");

    let dir = tempfile::tempdir()?;
    let marker = dir.path().join("executado");
    let hostile = MongoConnectionConfig::new(
        db.host(),
        db.port(),
        db.database(),
        &format!("x;touch {0}$(touch {0})", marker.display()),
        db.auth_database(),
        TlsMode::Auto,
    )?;
    let mut failing = tools.dump(&hostile, &password(&db))?;
    let mut sink = Vec::new();
    failing
        .take_stdout()
        .ok_or("stdout ausente")?
        .read_to_end(&mut sink)
        .await?;
    let error = failing.wait().await.expect_err("o dump deve falhar");
    assert!(
        matches!(
            error,
            ToolError::Failed {
                tool: "mongodump",
                ..
            }
        ),
        "{error:?}"
    );
    assert!(!error.to_string().contains(db.password()), "{error}");
    assert!(!format!("{error:?}").contains(db.password()));
    assert!(!marker.exists(), "um comando de shell foi executado");
    Ok(())
}

#[tokio::test]
async fn missing_tools_are_reported() {
    let paths = ToolPaths {
        mongodump: PathBuf::from("/nonexistent/mongodump"),
        mongorestore: PathBuf::from("/nonexistent/mongorestore"),
        docker: PathBuf::from("/nonexistent/docker"),
    };
    let server = MongoVersion { major: 8, minor: 0 };

    let native = MongoTools::detect_with(RunnerChoice::Native, server, paths.clone()).await;
    let auto = MongoTools::detect_with(RunnerChoice::Auto, server, paths.clone()).await;
    let docker = MongoTools::detect_with(RunnerChoice::Docker, server, paths).await;

    assert!(matches!(
        native,
        Err(ToolError::NotFound { tool: "mongodump", ref required })
            if required == "MongoDB Database Tools 100.10.0"
    ));
    assert!(matches!(
        auto,
        Err(ToolError::NoRunner {
            client: "MongoDB Database Tools"
        })
    ));
    assert!(matches!(docker, Err(ToolError::DockerUnavailable)));
    let message = native
        .map(|_| ())
        .expect_err("ferramenta ausente")
        .to_string();
    assert!(message.contains("--runner docker"), "{message}");
}

#[cfg(unix)]
fn fake_tool(dir: &Path, name: &str, version_output: &str) -> PathBuf {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    let path = dir.join(name);
    let mut file = std::fs::File::create(&path).expect("script");
    file.write_all(format!("#!/bin/sh\nprintf '%s' '{version_output}'\n").as_bytes())
        .expect("script");
    file.sync_all().expect("script");
    drop(file);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).expect("modo");
    path
}

#[cfg(unix)]
async fn detect_native(paths: &ToolPaths, server: MongoVersion) -> Result<MongoTools, ToolError> {
    let mut result = MongoTools::detect_with(RunnerChoice::Native, server, paths.clone()).await;
    for _ in 0..20 {
        let busy = matches!(
            &result,
            Err(ToolError::Spawn { source, .. }) if source.kind() == std::io::ErrorKind::ExecutableFileBusy
        );
        if !busy {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        result = MongoTools::detect_with(RunnerChoice::Native, server, paths.clone()).await;
    }
    result
}

#[cfg(unix)]
#[tokio::test]
async fn old_or_legacy_native_tools_are_refused() {
    let dir = tempfile::tempdir().expect("diretório temporário");
    let server = MongoVersion { major: 8, minor: 0 };
    let paths_with = |dump: PathBuf, restore: PathBuf| ToolPaths {
        mongodump: dump,
        mongorestore: restore,
        docker: PathBuf::from("/nonexistent/docker"),
    };
    let old = fake_tool(
        dir.path(),
        "dump-100.9.4",
        "mongodump version: 100.9.4\ngit version: abc\n",
    );
    let legacy = fake_tool(
        dir.path(),
        "dump-r4.2.24",
        "mongodump version: r4.2.24\ngit version: abc\nGo version: go1.12.17\n",
    );
    let current = fake_tool(
        dir.path(),
        "dump-100.10.0",
        "mongodump version: 100.10.0\ngit version: abc\n",
    );
    let restore_old = fake_tool(
        dir.path(),
        "restore-100.9.4",
        "mongorestore version: 100.9.4\ngit version: abc\n",
    );
    let restore_current = fake_tool(
        dir.path(),
        "restore-100.10.0",
        "mongorestore version: 100.10.0\ngit version: abc\n",
    );

    let too_old = detect_native(&paths_with(old, restore_current.clone()), server).await;
    let legacy_tools = detect_native(&paths_with(legacy, restore_current.clone()), server).await;
    let old_restore = detect_native(&paths_with(current.clone(), restore_old), server).await;
    let accepted = detect_native(&paths_with(current, restore_current), server).await;

    assert!(
        matches!(
            &too_old,
            Err(ToolError::Incompatible { tool: "mongodump", found, required })
                if found == "100.9.4" && required == "100.10.0"
        ),
        "{too_old:?}"
    );
    assert!(
        matches!(
            &legacy_tools,
            Err(ToolError::Incompatible { tool: "mongodump", found, required })
                if found == "r4.2.24 (ferramentas legadas)" && required == "100.10.0"
        ),
        "{legacy_tools:?}"
    );
    assert!(
        matches!(
            &old_restore,
            Err(ToolError::Incompatible { tool: "mongorestore", found, .. }) if found == "100.9.4"
        ),
        "{old_restore:?}"
    );
    let accepted = accepted.expect("ferramentas compatíveis");
    assert_eq!(accepted.version().to_string(), "100.10.0");
    assert_eq!(accepted.kind().as_str(), "native");
}
