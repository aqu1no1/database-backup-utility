use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use database_backup_core::{Password, PostgresConnectionConfig, TlsRequirement};
use tempfile::TempDir;
use thiserror::Error;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, ChildStderr, ChildStdout, Command};
use tokio::task::JoinHandle;

const STDERR_LIMIT: usize = 8 * 1024;
const CONTAINER_PGPASS: &str = "/run/dbu/pgpass";
const CONTAINER_ARCHIVE: &str = "/run/dbu/archive.dump";
const CONTAINER_PREFIX: &str = "dbu-pg-";
const REMOVE_ATTEMPTS: u32 = 10;
const REMOVE_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RunnerChoice {
    #[default]
    Auto,
    Native,
    Docker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnerKind {
    Native,
    Docker,
}

impl RunnerKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Docker => "docker",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PgVersion {
    pub major: u32,
    pub minor: u32,
}

impl PgVersion {
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let token = text
            .split_whitespace()
            .find(|token| token.starts_with(|c: char| c.is_ascii_digit()))?;
        let mut parts = token.split(|c: char| !c.is_ascii_digit());
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next().and_then(|part| part.parse().ok()).unwrap_or(0);
        Some(Self { major, minor })
    }
}

impl fmt::Display for PgVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

#[derive(Debug, Clone)]
pub struct ToolPaths {
    pub pg_dump: PathBuf,
    pub pg_restore: PathBuf,
    pub docker: PathBuf,
}

impl Default for ToolPaths {
    fn default() -> Self {
        Self {
            pg_dump: PathBuf::from("pg_dump"),
            pg_restore: PathBuf::from("pg_restore"),
            docker: PathBuf::from("docker"),
        }
    }
}

#[derive(Debug, Error)]
pub enum ToolError {
    #[error(
        "{tool} não encontrado; instale o cliente PostgreSQL {required} ou use --runner docker"
    )]
    NotFound { tool: &'static str, required: u32 },
    #[error(
        "{tool} {found} é incompatível com o servidor; é necessária a versão {required} ou superior"
    )]
    Incompatible {
        tool: &'static str,
        found: PgVersion,
        required: u32,
    },
    #[error("nem o cliente PostgreSQL nativo nem o Docker estão disponíveis")]
    NoRunner,
    #[error("o Docker não está disponível")]
    DockerUnavailable,
    #[error("falha ao executar {tool}")]
    Spawn {
        tool: &'static str,
        #[source]
        source: io::Error,
    },
    #[error("falha ao preparar as credenciais temporárias")]
    Credentials(#[source] io::Error),
    #[error("{tool} terminou com código {code}: {stderr}")]
    Failed {
        tool: &'static str,
        code: String,
        stderr: String,
    },
    #[error("tempo esgotado executando {tool}")]
    Timeout { tool: &'static str },
    #[error("{tool} foi cancelado")]
    Canceled { tool: &'static str },
}

#[derive(Debug, Clone)]
pub struct PgTools {
    kind: RunnerKind,
    version: PgVersion,
    paths: ToolPaths,
    image: Option<String>,
}

impl PgTools {
    pub async fn detect(choice: RunnerChoice, server: PgVersion) -> Result<Self, ToolError> {
        Self::detect_with(choice, server, ToolPaths::default()).await
    }

    pub async fn detect_with(
        choice: RunnerChoice,
        server: PgVersion,
        paths: ToolPaths,
    ) -> Result<Self, ToolError> {
        match choice {
            RunnerChoice::Native => Self::native(server, paths).await,
            RunnerChoice::Docker => Self::docker(server, paths).await,
            RunnerChoice::Auto => match Self::native(server, paths.clone()).await {
                Ok(tools) => Ok(tools),
                Err(native_error) => match Self::docker(server, paths).await {
                    Ok(tools) => Ok(tools),
                    Err(ToolError::DockerUnavailable) => match native_error {
                        ToolError::Incompatible { .. } => Err(native_error),
                        _ => Err(ToolError::NoRunner),
                    },
                    Err(error) => Err(error),
                },
            },
        }
    }

    async fn native(server: PgVersion, paths: ToolPaths) -> Result<Self, ToolError> {
        let dump = tool_version("pg_dump", &paths.pg_dump, server.major).await?;
        let restore = tool_version("pg_restore", &paths.pg_restore, server.major).await?;
        check_compatible("pg_dump", dump, server)?;
        check_compatible("pg_restore", restore, server)?;
        Ok(Self {
            kind: RunnerKind::Native,
            version: dump,
            paths,
            image: None,
        })
    }

    async fn docker(server: PgVersion, paths: ToolPaths) -> Result<Self, ToolError> {
        let status = Command::new(&paths.docker)
            .args(["version", "--format", "{{.Server.Version}}"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
        if !status.is_ok_and(|status| status.success()) {
            return Err(ToolError::DockerUnavailable);
        }
        let image = format!("postgres:{}", server.major);
        let output = Command::new(&paths.docker)
            .args([
                "run",
                "--rm",
                "--network",
                "none",
                "--entrypoint",
                "pg_dump",
            ])
            .arg(&image)
            .arg("--version")
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output()
            .await
            .map_err(|source| ToolError::Spawn {
                tool: "docker",
                source,
            })?;
        if !output.status.success() {
            return Err(ToolError::Failed {
                tool: "docker",
                code: exit_code(output.status),
                stderr: sanitize(&tail(&output.stderr), &[]),
            });
        }
        let version = PgVersion::parse(&String::from_utf8_lossy(&output.stdout)).ok_or(
            ToolError::NotFound {
                tool: "pg_dump",
                required: server.major,
            },
        )?;
        check_compatible("pg_dump", version, server)?;
        Ok(Self {
            kind: RunnerKind::Docker,
            version,
            image: Some(image),
            paths,
        })
    }

    #[must_use]
    pub fn kind(&self) -> RunnerKind {
        self.kind
    }

    #[must_use]
    pub fn version(&self) -> PgVersion {
        self.version
    }

    pub fn dump(
        &self,
        config: &PostgresConnectionConfig,
        password: &Password,
    ) -> Result<DumpProcess, ToolError> {
        let args = connection_args(config, &["--format=custom"]);
        self.spawn("pg_dump", config, password, args, None, true)
    }

    pub fn restore(
        &self,
        config: &PostgresConnectionConfig,
        password: &Password,
        archive: &Path,
    ) -> Result<RestoreProcess, ToolError> {
        let args = connection_args(
            config,
            &[
                "--single-transaction",
                "--exit-on-error",
                "--no-owner",
                "--no-privileges",
            ],
        );
        self.spawn("pg_restore", config, password, args, Some(archive), false)
    }

    pub async fn restore_list(&self, archive: &Path) -> Result<ArchiveInfo, ToolError> {
        let mut command = match self.kind {
            RunnerKind::Native => {
                let mut command = Command::new(&self.paths.pg_restore);
                command.arg("--list").arg(archive);
                command
            }
            RunnerKind::Docker => {
                let archive = absolute("pg_restore", archive)?;
                let mut command = Command::new(&self.paths.docker);
                command
                    .args(["run", "--rm", "--network", "none", "-v"])
                    .arg(mount(&archive, CONTAINER_ARCHIVE))
                    .args(["--entrypoint", "pg_restore"])
                    .arg(self.image())
                    .args(["--list", CONTAINER_ARCHIVE]);
                command
            }
        };
        let output = command
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output()
            .await
            .map_err(|source| ToolError::Spawn {
                tool: "pg_restore",
                source,
            })?;
        if !output.status.success() {
            return Err(ToolError::Failed {
                tool: "pg_restore",
                code: exit_code(output.status),
                stderr: sanitize(&tail(&output.stderr), &[]),
            });
        }
        Ok(ArchiveInfo::parse(&String::from_utf8_lossy(&output.stdout)))
    }

    fn spawn(
        &self,
        tool: &'static str,
        config: &PostgresConnectionConfig,
        password: &Password,
        mut args: Vec<OsString>,
        archive: Option<&Path>,
        capture_stdout: bool,
    ) -> Result<ToolProcess, ToolError> {
        let secrets = PgPass::write(config, password)?;

        let (mut command, container) = match self.kind {
            RunnerKind::Native => {
                let program = match tool {
                    "pg_restore" => &self.paths.pg_restore,
                    _ => &self.paths.pg_dump,
                };
                if let Some(archive) = archive {
                    args.push(archive.as_os_str().to_owned());
                }
                let mut command = Command::new(program);
                command
                    .args(&args)
                    .env("PGPASSFILE", secrets.path())
                    .env("PGSSLMODE", ssl_mode(config.tls()))
                    .env_remove("PGPASSWORD");
                (command, None)
            }
            RunnerKind::Docker => {
                let name = container_name();
                let mut command = Command::new(&self.paths.docker);
                command.args(["run", "--rm", "-i", "--name", &name]);
                if config.is_local() {
                    command.args(["--network", "host"]);
                }
                command
                    .arg("-v")
                    .arg(mount(&secrets.path(), CONTAINER_PGPASS))
                    .args(["-e", &format!("PGPASSFILE={CONTAINER_PGPASS}")])
                    .args(["-e", &format!("PGSSLMODE={}", ssl_mode(config.tls()))]);
                if config.tls() == TlsRequirement::VerifyFull {
                    command
                        .args(["-v", "/etc/ssl/certs:/etc/ssl/certs:ro"])
                        .args(["-e", "PGSSLROOTCERT=system"]);
                }
                if let Some(archive) = archive {
                    command
                        .arg("-v")
                        .arg(mount(&absolute(tool, archive)?, CONTAINER_ARCHIVE));
                    args.push(OsString::from(CONTAINER_ARCHIVE));
                }
                command
                    .args(["--entrypoint", tool])
                    .arg(self.image())
                    .args(&args);
                (command, Some(name))
            }
        };

        let stdout = if capture_stdout {
            Stdio::piped()
        } else {
            Stdio::null()
        };
        let mut child = command
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|source| ToolError::Spawn { tool, source })?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take().map(collect_stderr);

        Ok(ToolProcess {
            tool,
            child,
            stdout,
            stderr,
            container,
            docker: self.paths.docker.clone(),
            redact: vec![
                password.expose().to_owned(),
                secrets.path().display().to_string(),
            ],
            _secrets: secrets,
            finished: false,
        })
    }

    fn image(&self) -> &str {
        self.image.as_deref().unwrap_or("postgres")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ArchiveInfo {
    pub entries: usize,
    pub tables: usize,
    pub database: Option<String>,
    pub dumped_from: Option<PgVersion>,
    pub dumped_by: Option<PgVersion>,
}

impl ArchiveInfo {
    #[must_use]
    pub fn parse(listing: &str) -> Self {
        let mut info = Self::default();
        for line in listing.lines() {
            if let Some(header) = line.strip_prefix(';') {
                let header = header.trim();
                if let Some(value) = header.strip_prefix("dbname:") {
                    info.database = Some(value.trim().to_owned());
                } else if let Some(value) = header.strip_prefix("Dumped from database version:") {
                    info.dumped_from = PgVersion::parse(value);
                } else if let Some(value) = header.strip_prefix("Dumped by pg_dump version:") {
                    info.dumped_by = PgVersion::parse(value);
                }
            } else if !line.trim().is_empty() {
                info.entries += 1;
                if line.contains(" TABLE ") && !line.contains(" TABLE DATA ") {
                    info.tables += 1;
                }
            }
        }
        info
    }
}

pub type DumpProcess = ToolProcess;
pub type RestoreProcess = ToolProcess;

pub struct ToolProcess {
    tool: &'static str,
    child: Child,
    stdout: Option<ChildStdout>,
    stderr: Option<JoinHandle<Vec<u8>>>,
    container: Option<String>,
    docker: PathBuf,
    redact: Vec<String>,
    _secrets: PgPass,
    finished: bool,
}

impl ToolProcess {
    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.stdout.take()
    }

    #[must_use]
    pub fn container_name(&self) -> Option<&str> {
        self.container.as_deref()
    }

    pub async fn wait(&mut self) -> Result<(), ToolError> {
        let tool = self.tool;
        let status = self
            .child
            .wait()
            .await
            .map_err(|source| ToolError::Spawn { tool, source })?;
        self.finished = true;
        let stderr = match self.stderr.take() {
            Some(task) => task.await.unwrap_or_default(),
            None => Vec::new(),
        };
        if status.success() {
            return Ok(());
        }
        Err(ToolError::Failed {
            tool,
            code: exit_code(status),
            stderr: sanitize(&tail(&stderr), &self.redact),
        })
    }

    pub async fn cancel(mut self) {
        let _ = self.child.start_kill();
        let _ = self.child.wait().await;
        if let Some(name) = &self.container {
            for attempt in 0..REMOVE_ATTEMPTS {
                let removed = Command::new(&self.docker)
                    .args(["rm", "--force", name])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .await
                    .is_ok_and(|status| status.success());
                if removed || attempt + 1 == REMOVE_ATTEMPTS {
                    break;
                }
                tokio::time::sleep(REMOVE_INTERVAL).await;
            }
        }
        self.finished = true;
    }
}

impl Drop for ToolProcess {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        if let Some(name) = &self.container {
            let _ = std::process::Command::new(&self.docker)
                .args(["rm", "--force", name])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
        }
    }
}

struct PgPass {
    dir: TempDir,
}

impl PgPass {
    fn write(config: &PostgresConnectionConfig, password: &Password) -> Result<Self, ToolError> {
        let dir = tempfile::Builder::new()
            .prefix("dbu-")
            .tempdir()
            .map_err(ToolError::Credentials)?;
        let line = format!(
            "*:*:*:{}:{}\n",
            escape_pgpass(config.user()),
            escape_pgpass(password.expose()),
        );
        let path = dir.path().join("pgpass");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&path).map_err(ToolError::Credentials)?;
        io::Write::write_all(&mut file, line.as_bytes()).map_err(ToolError::Credentials)?;
        Ok(Self { dir })
    }

    fn path(&self) -> PathBuf {
        self.dir.path().join("pgpass")
    }
}

async fn tool_version(
    tool: &'static str,
    path: &Path,
    required: u32,
) -> Result<PgVersion, ToolError> {
    let output = Command::new(path)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|source| match source.kind() {
            io::ErrorKind::NotFound => ToolError::NotFound { tool, required },
            _ => ToolError::Spawn { tool, source },
        })?;
    if !output.status.success() {
        return Err(ToolError::NotFound { tool, required });
    }
    PgVersion::parse(&String::from_utf8_lossy(&output.stdout))
        .ok_or(ToolError::NotFound { tool, required })
}

fn check_compatible(
    tool: &'static str,
    found: PgVersion,
    server: PgVersion,
) -> Result<(), ToolError> {
    if found.major >= server.major {
        Ok(())
    } else {
        Err(ToolError::Incompatible {
            tool,
            found,
            required: server.major,
        })
    }
}

fn connection_args(config: &PostgresConnectionConfig, extra: &[&str]) -> Vec<OsString> {
    let port = config.port().to_string();
    let dbname = dbname_conninfo(config.database());
    let base = [
        "--no-password",
        "--host",
        config.host(),
        "--port",
        &port,
        "--username",
        config.user(),
        "--dbname",
        &dbname,
    ];
    extra
        .iter()
        .chain(base.iter())
        .map(OsString::from)
        .collect()
}

fn dbname_conninfo(database: &str) -> String {
    let escaped = database.replace('\\', "\\\\").replace('\'', "\\'");
    format!("dbname='{escaped}'")
}

fn absolute(tool: &'static str, path: &Path) -> Result<PathBuf, ToolError> {
    std::path::absolute(path).map_err(|source| ToolError::Spawn { tool, source })
}

fn ssl_mode(tls: TlsRequirement) -> &'static str {
    match tls {
        TlsRequirement::Plain => "disable",
        TlsRequirement::VerifyFull => "verify-full",
    }
}

fn container_name() -> String {
    format!("{CONTAINER_PREFIX}{}", uuid::Uuid::now_v7().simple())
}

fn mount(host: &Path, container: &str) -> OsString {
    let mut value = host.as_os_str().to_owned();
    value.push(format!(":{container}:ro"));
    value
}

fn escape_pgpass(value: &str) -> String {
    value.replace('\\', "\\\\").replace(':', "\\:")
}

fn collect_stderr(mut stderr: ChildStderr) -> JoinHandle<Vec<u8>> {
    tokio::spawn(async move {
        let mut kept = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            match stderr.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    kept.extend_from_slice(&buffer[..read]);
                    if kept.len() > STDERR_LIMIT {
                        kept.drain(..kept.len() - STDERR_LIMIT);
                    }
                }
            }
        }
        kept
    })
}

fn tail(bytes: &[u8]) -> String {
    let start = bytes.len().saturating_sub(STDERR_LIMIT);
    String::from_utf8_lossy(&bytes[start..]).trim().to_owned()
}

fn sanitize(text: &str, secrets: &[String]) -> String {
    secrets
        .iter()
        .filter(|secret| !secret.is_empty())
        .fold(text.to_owned(), |text, secret| {
            text.replace(secret.as_str(), "***")
        })
}

fn exit_code(status: ExitStatus) -> String {
    status.code().map_or_else(
        || "desconhecido (sinal)".to_owned(),
        |code| code.to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tool_and_server_versions() {
        assert_eq!(
            PgVersion::parse("pg_dump (PostgreSQL) 17.11 (Ubuntu 17.11-1.pgdg24.04+1)"),
            Some(PgVersion {
                major: 17,
                minor: 11
            })
        );
        assert_eq!(
            PgVersion::parse("17.11 (Debian 17.11-1.pgdg13+2)"),
            Some(PgVersion {
                major: 17,
                minor: 11
            })
        );
        assert_eq!(
            PgVersion::parse("pg_restore (PostgreSQL) 18beta1"),
            Some(PgVersion {
                major: 18,
                minor: 0
            })
        );
        assert_eq!(PgVersion::parse("sem versão"), None);
    }

    #[test]
    fn older_client_is_incompatible() {
        let server = PgVersion {
            major: 17,
            minor: 2,
        };
        assert!(
            check_compatible(
                "pg_dump",
                PgVersion {
                    major: 17,
                    minor: 0
                },
                server
            )
            .is_ok()
        );
        assert!(
            check_compatible(
                "pg_dump",
                PgVersion {
                    major: 18,
                    minor: 1
                },
                server
            )
            .is_ok()
        );
        assert!(matches!(
            check_compatible(
                "pg_dump",
                PgVersion {
                    major: 16,
                    minor: 9
                },
                server
            ),
            Err(ToolError::Incompatible { required: 17, .. })
        ));
    }

    #[test]
    fn parses_archive_listing_header_and_entries() {
        let listing = ";
; Archive created at 2026-10-20 19:25:10 UTC
;     dbname: my_database
;     TOC Entries: 13
;     Compression: gzip
;     Format: CUSTOM
;     Dumped from database version: 17.11 (Debian 17.11-1.pgdg13+2)
;     Dumped by pg_dump version: 17.11 (Debian 17.11-1.pgdg13+2)
;
;
; Selected TOC Entries:
;
217; 1259 16386 TABLE public customers backup_test
216; 1259 16385 SEQUENCE public customers_id_seq backup_test
219; 1259 16398 TABLE public orders backup_test
3360; 0 16386 TABLE DATA public customers backup_test
3362; 0 16398 TABLE DATA public orders backup_test
";

        let info = ArchiveInfo::parse(listing);

        assert_eq!(info.entries, 5);
        assert_eq!(info.tables, 2);
        assert_eq!(info.database.as_deref(), Some("my_database"));
        assert_eq!(
            info.dumped_from,
            Some(PgVersion {
                major: 17,
                minor: 11
            })
        );
        assert_eq!(
            info.dumped_by,
            Some(PgVersion {
                major: 17,
                minor: 11
            })
        );
    }

    #[test]
    fn dbname_is_always_a_quoted_conninfo_value() {
        assert_eq!(dbname_conninfo("app"), "dbname='app'");
        assert_eq!(
            dbname_conninfo("dbname=postgres"),
            "dbname='dbname=postgres'"
        );
        assert_eq!(
            dbname_conninfo(r"it's a\b host=x"),
            r"dbname='it\'s a\\b host=x'"
        );
    }

    #[test]
    fn escapes_pgpass_separators() {
        assert_eq!(escape_pgpass(r"a:b\c"), r"a\:b\\c");
    }

    #[test]
    fn sanitize_removes_every_secret() {
        let text = "falha para senha123 em /tmp/dbu-x/pgpass (senha123)";
        let clean = sanitize(
            text,
            &["senha123".to_owned(), "/tmp/dbu-x/pgpass".to_owned()],
        );
        assert_eq!(clean, "falha para *** em *** (***)");
    }

    #[test]
    fn stderr_tail_is_bounded() {
        let big = vec![b'x'; STDERR_LIMIT * 3];
        assert_eq!(tail(&big).len(), STDERR_LIMIT);
    }

    #[cfg(unix)]
    #[test]
    fn pgpass_is_private_and_removed_on_drop() {
        use std::os::unix::fs::PermissionsExt;

        let config = PostgresConnectionConfig::new(
            "localhost",
            5432,
            "db",
            "user",
            database_backup_core::TlsMode::Auto,
        )
        .expect("configuração válida");
        let pgpass = PgPass::write(&config, &Password::new("a:b".to_owned())).expect("pgpass");
        let path = pgpass.path();

        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode();
        let content = std::fs::read_to_string(&path).expect("conteúdo");
        drop(pgpass);

        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(content, "*:*:*:user:a\\:b\n");
        assert!(!path.exists());
    }
}
