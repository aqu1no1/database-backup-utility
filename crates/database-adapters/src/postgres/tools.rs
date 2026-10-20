use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};

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
        let secrets = PgPass::write(config, password)?;
        let pg_args: Vec<OsString> = [
            "--format=custom",
            "--no-password",
            "--host",
            config.host(),
            "--port",
            &config.port().to_string(),
            "--username",
            config.user(),
            "--dbname",
            config.database(),
        ]
        .into_iter()
        .map(OsString::from)
        .collect();

        let (mut command, container) = match self.kind {
            RunnerKind::Native => {
                let mut command = Command::new(&self.paths.pg_dump);
                command
                    .args(&pg_args)
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
                command
                    .args(["--entrypoint", "pg_dump"])
                    .arg(self.image())
                    .args(&pg_args);
                (command, Some(name))
            }
        };

        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|source| ToolError::Spawn {
                tool: "pg_dump",
                source,
            })?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take().map(collect_stderr);

        Ok(DumpProcess {
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

    pub async fn restore_list(&self, archive: &Path) -> Result<usize, ToolError> {
        let mut command = match self.kind {
            RunnerKind::Native => {
                let mut command = Command::new(&self.paths.pg_restore);
                command.arg("--list").arg(archive);
                command
            }
            RunnerKind::Docker => {
                let archive = std::path::absolute(archive).map_err(|source| ToolError::Spawn {
                    tool: "pg_restore",
                    source,
                })?;
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
        let entries = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| !line.is_empty() && !line.starts_with(';'))
            .count();
        Ok(entries)
    }

    fn image(&self) -> &str {
        self.image.as_deref().unwrap_or("postgres")
    }
}

pub struct DumpProcess {
    child: Child,
    stdout: Option<ChildStdout>,
    stderr: Option<JoinHandle<Vec<u8>>>,
    container: Option<String>,
    docker: PathBuf,
    redact: Vec<String>,
    _secrets: PgPass,
    finished: bool,
}

impl DumpProcess {
    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.stdout.take()
    }

    #[must_use]
    pub fn container_name(&self) -> Option<&str> {
        self.container.as_deref()
    }

    pub async fn wait(mut self) -> Result<(), ToolError> {
        let status = self.child.wait().await.map_err(|source| ToolError::Spawn {
            tool: "pg_dump",
            source,
        })?;
        self.finished = true;
        let stderr = match self.stderr.take() {
            Some(task) => task.await.unwrap_or_default(),
            None => Vec::new(),
        };
        if status.success() {
            return Ok(());
        }
        Err(ToolError::Failed {
            tool: "pg_dump",
            code: exit_code(status),
            stderr: sanitize(&tail(&stderr), &self.redact),
        })
    }

    pub async fn cancel(mut self) {
        if let Some(name) = &self.container {
            let _ = Command::new(&self.docker)
                .args(["rm", "--force", name])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await;
        }
        let _ = self.child.start_kill();
        let _ = self.child.wait().await;
        self.finished = true;
    }
}

impl Drop for DumpProcess {
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
            "*:*:{}:{}:{}\n",
            escape_pgpass(config.database()),
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
        assert_eq!(content, "*:*:db:user:a\\:b\n");
        assert!(!path.exists());
    }
}
