use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use database_backup_core::{MysqlConnectionConfig, Password, TlsRequirement};
use tempfile::TempDir;
use tokio::process::Command;

use crate::process::{
    DumpProcess, Launch, RestoreProcess, RunnerChoice, RunnerKind, ToolError, ToolProcess,
    absolute, container_name, docker_available, exit_code, mount, sanitize, secret_dir, tail,
    write_private,
};

const CONTAINER_OPTIONS: &str = "/run/dbu/client.cnf";
const CONTAINER_CA: &str = "/run/dbu/ca.pem";
const CONTAINER_PREFIX: &str = "dbu-my-";
const CLIENT: &str = "MySQL";
const OPTIONS_FILE: &str = "client.cnf";
const CA_BUNDLES: [&str; 3] = [
    "/etc/ssl/certs/ca-certificates.crt",
    "/etc/pki/tls/certs/ca-bundle.crt",
    "/etc/ssl/cert.pem",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MysqlVersion {
    pub major: u32,
    pub minor: u32,
}

impl MysqlVersion {
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        if text.contains("MariaDB") {
            return None;
        }
        let text = text.split_once("Distrib ").map_or(text, |(_, rest)| rest);
        let token = text
            .split_whitespace()
            .find(|token| token.starts_with(|c: char| c.is_ascii_digit()))?;
        let mut parts = token.split(|c: char| !c.is_ascii_digit());
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        Some(Self { major, minor })
    }
}

impl fmt::Display for MysqlVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

#[derive(Debug, Clone)]
pub struct ToolPaths {
    pub mysqldump: PathBuf,
    pub mysql: PathBuf,
    pub docker: PathBuf,
}

impl Default for ToolPaths {
    fn default() -> Self {
        Self {
            mysqldump: PathBuf::from("mysqldump"),
            mysql: PathBuf::from("mysql"),
            docker: PathBuf::from("docker"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MysqlTools {
    kind: RunnerKind,
    version: MysqlVersion,
    paths: ToolPaths,
    image: Option<String>,
}

impl MysqlTools {
    pub async fn detect(choice: RunnerChoice, server: MysqlVersion) -> Result<Self, ToolError> {
        Self::detect_with(choice, server, ToolPaths::default()).await
    }

    pub async fn detect_with(
        choice: RunnerChoice,
        server: MysqlVersion,
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
                        _ => Err(ToolError::NoRunner { client: CLIENT }),
                    },
                    Err(error) => Err(error),
                },
            },
        }
    }

    async fn native(server: MysqlVersion, paths: ToolPaths) -> Result<Self, ToolError> {
        let dump = tool_version("mysqldump", &paths.mysqldump, server).await?;
        let client = tool_version("mysql", &paths.mysql, server).await?;
        check_compatible("mysqldump", dump, server)?;
        check_compatible("mysql", client, server)?;
        Ok(Self {
            kind: RunnerKind::Native,
            version: dump,
            paths,
            image: None,
        })
    }

    async fn docker(server: MysqlVersion, paths: ToolPaths) -> Result<Self, ToolError> {
        if !docker_available(&paths.docker).await {
            return Err(ToolError::DockerUnavailable);
        }
        let image = format!("mysql:{server}");
        let output = Command::new(&paths.docker)
            .args([
                "run",
                "--rm",
                "--network",
                "none",
                "--entrypoint",
                "mysqldump",
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
        let version = MysqlVersion::parse(&String::from_utf8_lossy(&output.stdout))
            .ok_or_else(|| not_found("mysqldump", server))?;
        check_compatible("mysqldump", version, server)?;
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
    pub fn version(&self) -> MysqlVersion {
        self.version
    }

    pub fn dump(
        &self,
        config: &MysqlConnectionConfig,
        password: &Password,
    ) -> Result<DumpProcess, ToolError> {
        let extra = [
            "--single-transaction",
            "--routines",
            "--triggers",
            "--events",
            "--set-gtid-purged=OFF",
            "--no-tablespaces",
            "--hex-blob",
            "--default-character-set=utf8mb4",
            "--",
        ];
        let mut args: Vec<OsString> = extra.iter().map(OsString::from).collect();
        args.push(OsString::from(config.database()));
        self.spawn("mysqldump", config, password, args, None)
    }

    pub fn restore(
        &self,
        config: &MysqlConnectionConfig,
        password: &Password,
        dump: &Path,
    ) -> Result<RestoreProcess, ToolError> {
        let input = std::fs::File::open(dump).map_err(|source| ToolError::Spawn {
            tool: "mysql",
            source,
        })?;
        let args = [
            "--batch",
            "--binary-mode",
            "--local-infile=0",
            "--default-character-set=utf8mb4",
        ]
        .iter()
        .map(OsString::from)
        .chain([OsString::from(format!("--database={}", config.database()))])
        .collect();
        self.spawn("mysql", config, password, args, Some(input))
    }

    fn spawn(
        &self,
        tool: &'static str,
        config: &MysqlConnectionConfig,
        password: &Password,
        extra: Vec<OsString>,
        input: Option<std::fs::File>,
    ) -> Result<ToolProcess, ToolError> {
        let secrets = write_options(password)?;
        let options_path = secrets.path().join(OPTIONS_FILE);
        let ca = match config.tls() {
            TlsRequirement::Plain => None,
            TlsRequirement::VerifyFull => Some(ca_bundle()?),
        };

        let (command, container) = match self.kind {
            RunnerKind::Native => {
                let program = match tool {
                    "mysql" => &self.paths.mysql,
                    _ => &self.paths.mysqldump,
                };
                let mut command = Command::new(program);
                command
                    .args(connection_args(config, &options_path, ca.as_deref()))
                    .args(&extra)
                    .env_remove("MYSQL_PWD")
                    .env_remove("MYSQL_HOST")
                    .env_remove("MYSQL_TCP_PORT");
                (command, None)
            }
            RunnerKind::Docker => {
                let name = container_name(CONTAINER_PREFIX);
                let mut command = Command::new(&self.paths.docker);
                command.args(["run", "--rm", "-i", "--name", &name]);
                if config.is_local() {
                    command.args(["--network", "host"]);
                }
                command
                    .arg("-v")
                    .arg(mount(&options_path, CONTAINER_OPTIONS));
                if let Some(ca) = &ca {
                    command
                        .arg("-v")
                        .arg(mount(&absolute(tool, ca)?, CONTAINER_CA));
                }
                command
                    .args(["--entrypoint", tool])
                    .arg(self.image())
                    .args(connection_args(
                        config,
                        Path::new(CONTAINER_OPTIONS),
                        ca.as_ref().map(|_| Path::new(CONTAINER_CA)),
                    ))
                    .args(&extra);
                (command, Some(name))
            }
        };

        let (stdin, capture_stdout) = match input {
            Some(file) => (Stdio::from(file), false),
            None => (Stdio::null(), true),
        };
        ToolProcess::spawn(Launch {
            tool,
            command,
            stdin,
            capture_stdout,
            container,
            docker: self.paths.docker.clone(),
            redact: vec![
                password.expose().to_owned(),
                options_path.display().to_string(),
            ],
            secrets,
        })
    }

    fn image(&self) -> &str {
        self.image.as_deref().unwrap_or("mysql")
    }
}

fn connection_args(
    config: &MysqlConnectionConfig,
    options: &Path,
    ca: Option<&Path>,
) -> Vec<OsString> {
    let mut defaults = OsString::from("--defaults-extra-file=");
    defaults.push(options);
    let mut args = vec![
        defaults,
        OsString::from("--protocol=TCP"),
        OsString::from(format!("--host={}", config.host())),
        OsString::from(format!("--port={}", config.port())),
        OsString::from(format!("--user={}", config.user())),
    ];
    match ca {
        None => {
            args.push(OsString::from("--ssl-mode=DISABLED"));
            args.push(OsString::from("--get-server-public-key"));
        }
        Some(ca) => {
            args.push(OsString::from("--ssl-mode=VERIFY_IDENTITY"));
            let mut value = OsString::from("--ssl-ca=");
            value.push(ca);
            args.push(value);
        }
    }
    args
}

fn write_options(password: &Password) -> Result<TempDir, ToolError> {
    let dir = secret_dir()?;
    let content = format!(
        "[client]\npassword=\"{}\"\n",
        escape_option(password.expose())
    );
    write_private(&dir.path().join(OPTIONS_FILE), &content)?;
    Ok(dir)
}

fn escape_option(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            other => escaped.push(other),
        }
    }
    escaped
}

fn ca_bundle() -> Result<PathBuf, ToolError> {
    CA_BUNDLES
        .iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
        .ok_or(ToolError::NoCaBundle)
}

async fn tool_version(
    tool: &'static str,
    path: &Path,
    server: MysqlVersion,
) -> Result<MysqlVersion, ToolError> {
    let output = Command::new(path)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|source| match source.kind() {
            io::ErrorKind::NotFound => not_found(tool, server),
            _ => ToolError::Spawn { tool, source },
        })?;
    if !output.status.success() {
        return Err(not_found(tool, server));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    if text.contains("MariaDB") {
        return Err(ToolError::Incompatible {
            tool,
            found: "MariaDB".to_owned(),
            required: format!("{CLIENT} {server}"),
        });
    }
    MysqlVersion::parse(&text).ok_or_else(|| not_found(tool, server))
}

fn not_found(tool: &'static str, server: MysqlVersion) -> ToolError {
    ToolError::NotFound {
        tool,
        required: format!("{CLIENT} {server}"),
    }
}

fn check_compatible(
    tool: &'static str,
    found: MysqlVersion,
    server: MysqlVersion,
) -> Result<(), ToolError> {
    if found >= server {
        Ok(())
    } else {
        Err(ToolError::Incompatible {
            tool,
            found: found.to_string(),
            required: server.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_client_and_server_versions() {
        let v84 = Some(MysqlVersion { major: 8, minor: 4 });
        assert_eq!(
            MysqlVersion::parse("mysqldump  Ver 8.4.11-0ubuntu0.26.04.1 for Linux on x86_64"),
            v84
        );
        assert_eq!(MysqlVersion::parse("8.4.13"), v84);
        assert_eq!(
            MysqlVersion::parse("mysqldump  Ver 10.13 Distrib 5.7.44, for Linux (x86_64)"),
            Some(MysqlVersion { major: 5, minor: 7 })
        );
        assert_eq!(
            MysqlVersion::parse("mysqldump from 11.4.2-MariaDB, client 10.19 for debian-linux-gnu"),
            None
        );
        assert_eq!(MysqlVersion::parse("11.4.2-MariaDB-ubu2404"), None);
        assert_eq!(MysqlVersion::parse("sem versão"), None);
    }

    #[test]
    fn client_older_than_server_minor_is_incompatible() {
        let server = MysqlVersion { major: 8, minor: 4 };
        assert!(check_compatible("mysqldump", MysqlVersion { major: 8, minor: 4 }, server).is_ok());
        assert!(check_compatible("mysqldump", MysqlVersion { major: 9, minor: 0 }, server).is_ok());
        assert!(matches!(
            check_compatible("mysqldump", MysqlVersion { major: 8, minor: 0 }, server),
            Err(ToolError::Incompatible { ref required, .. }) if required == "8.4"
        ));
    }

    #[test]
    fn option_file_values_are_quoted_and_escaped() {
        assert_eq!(escape_option(r#"a"b\c"#), r#"a\"b\\c"#);
        assert_eq!(escape_option("x\ny\tz"), "x\\ny\\tz");
        assert_eq!(escape_option("#;=[]' "), "#;=[]' ");
    }

    #[test]
    fn options_file_is_the_first_argument_and_password_never_is_an_argument() {
        let config = MysqlConnectionConfig::new(
            "localhost",
            3306,
            "-app",
            "root",
            database_backup_core::TlsMode::Auto,
        )
        .expect("configuração válida");
        let args = connection_args(&config, Path::new("/tmp/dbu-x/client.cnf"), None);

        assert_eq!(args[0], "--defaults-extra-file=/tmp/dbu-x/client.cnf");
        assert!(args.contains(&OsString::from("--protocol=TCP")));
        assert!(args.contains(&OsString::from("--ssl-mode=DISABLED")));
        assert!(
            !args
                .iter()
                .any(|arg| arg.to_string_lossy().contains("password"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn options_file_is_private_and_removed_on_drop() {
        use std::os::unix::fs::PermissionsExt;

        let dir = write_options(&Password::new("p\"w".to_owned())).expect("arquivo");
        let path = dir.path().join(OPTIONS_FILE);
        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode();
        let content = std::fs::read_to_string(&path).expect("conteúdo");
        drop(dir);

        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(content, "[client]\npassword=\"p\\\"w\"\n");
        assert!(!path.exists());
    }
}
