use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use tempfile::TempDir;
use thiserror::Error;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, ChildStderr, ChildStdout, Command};
use tokio::task::JoinHandle;

const STDERR_LIMIT: usize = 8 * 1024;
const REMOVE_ATTEMPTS: u32 = 10;
const REMOVE_INTERVAL: Duration = Duration::from_millis(200);
const CA_BUNDLES: [&str; 3] = [
    "/etc/ssl/certs/ca-certificates.crt",
    "/etc/pki/tls/certs/ca-bundle.crt",
    "/etc/ssl/cert.pem",
];

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

#[derive(Debug, Error)]
pub enum ToolError {
    #[error("{tool} não encontrado; instale o cliente {required} ou use --runner docker")]
    NotFound {
        tool: &'static str,
        required: String,
    },
    #[error(
        "{tool} {found} é incompatível com o servidor; é necessária a versão {required} ou superior"
    )]
    Incompatible {
        tool: &'static str,
        found: String,
        required: String,
    },
    #[error("nem o cliente {client} nativo nem o Docker estão disponíveis")]
    NoRunner { client: &'static str },
    #[error("o Docker não está disponível")]
    DockerUnavailable,
    #[error("falha ao executar {tool}")]
    Spawn {
        tool: &'static str,
        #[source]
        source: io::Error,
    },
    #[error("nenhum pacote de certificados raiz do sistema encontrado para validar o TLS")]
    NoCaBundle,
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

pub(crate) struct Launch {
    pub tool: &'static str,
    pub command: Command,
    pub stdin: Stdio,
    pub capture_stdout: bool,
    pub container: Option<String>,
    pub docker: PathBuf,
    pub redact: Vec<String>,
    pub secrets: TempDir,
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
    _secrets: TempDir,
    finished: bool,
}

impl ToolProcess {
    pub(crate) fn spawn(launch: Launch) -> Result<Self, ToolError> {
        let Launch {
            tool,
            mut command,
            stdin,
            capture_stdout,
            container,
            docker,
            redact,
            secrets,
        } = launch;
        let stdout = if capture_stdout {
            Stdio::piped()
        } else {
            Stdio::null()
        };
        let mut child = command
            .stdin(stdin)
            .stdout(stdout)
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|source| ToolError::Spawn { tool, source })?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take().map(collect_stderr);

        Ok(Self {
            tool,
            child,
            stdout,
            stderr,
            container,
            docker,
            redact,
            _secrets: secrets,
            finished: false,
        })
    }

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

pub(crate) async fn docker_available(docker: &Path) -> bool {
    Command::new(docker)
        .args(["version", "--format", "{{.Server.Version}}"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .is_ok_and(|status| status.success())
}

pub(crate) fn secret_dir() -> Result<TempDir, ToolError> {
    tempfile::Builder::new()
        .prefix("dbu-")
        .tempdir()
        .map_err(ToolError::Credentials)
}

pub(crate) fn write_private(path: &Path, content: &str) -> Result<(), ToolError> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(path).map_err(ToolError::Credentials)?;
    io::Write::write_all(&mut file, content.as_bytes()).map_err(ToolError::Credentials)
}

pub(crate) fn ca_bundle() -> Result<PathBuf, ToolError> {
    CA_BUNDLES
        .iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
        .ok_or(ToolError::NoCaBundle)
}

pub(crate) fn absolute(tool: &'static str, path: &Path) -> Result<PathBuf, ToolError> {
    std::path::absolute(path).map_err(|source| ToolError::Spawn { tool, source })
}

pub(crate) fn container_name(prefix: &str) -> String {
    format!("{prefix}{}", uuid::Uuid::now_v7().simple())
}

pub(crate) fn mount(host: &Path, container: &str) -> OsString {
    let mut value = host.as_os_str().to_owned();
    value.push(format!(":{container}:ro"));
    value
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

pub(crate) fn tail(bytes: &[u8]) -> String {
    let start = bytes.len().saturating_sub(STDERR_LIMIT);
    String::from_utf8_lossy(&bytes[start..]).trim().to_owned()
}

pub(crate) fn sanitize(text: &str, secrets: &[String]) -> String {
    secrets
        .iter()
        .filter(|secret| !secret.is_empty())
        .fold(text.to_owned(), |text, secret| {
            text.replace(secret.as_str(), "***")
        })
}

pub(crate) fn exit_code(status: ExitStatus) -> String {
    status.code().map_or_else(
        || "desconhecido (sinal)".to_owned(),
        |code| code.to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
