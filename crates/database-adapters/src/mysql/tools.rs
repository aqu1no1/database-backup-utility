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
    absolute, ca_bundle, container_name, docker_available, exit_code, mount, sanitize, secret_dir,
    tail, write_private,
};

const CONTAINER_OPTIONS: &str = "/run/dbu/client.cnf";
const CONTAINER_CA: &str = "/run/dbu/ca.pem";
const OPTIONS_FILE: &str = "client.cnf";
const MARIADB_MARKER: &str = "-MariaDB";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Flavor {
    Mysql,
    Mariadb,
}

impl Flavor {
    #[must_use]
    pub fn of(version_text: &str) -> Self {
        if version_text.contains("MariaDB") {
            Self::Mariadb
        } else {
            Self::Mysql
        }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Mysql => "MySQL",
            Self::Mariadb => "MariaDB",
        }
    }

    #[must_use]
    pub const fn dump_tool(self) -> &'static str {
        match self {
            Self::Mysql => "mysqldump",
            Self::Mariadb => "mariadb-dump",
        }
    }

    #[must_use]
    pub const fn client_tool(self) -> &'static str {
        match self {
            Self::Mysql => "mysql",
            Self::Mariadb => "mariadb",
        }
    }

    const fn image(self) -> &'static str {
        match self {
            Self::Mysql => "mysql",
            Self::Mariadb => "mariadb",
        }
    }

    const fn container_prefix(self) -> &'static str {
        match self {
            Self::Mysql => "dbu-my-",
            Self::Mariadb => "dbu-mdb-",
        }
    }
}

impl fmt::Display for Flavor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MysqlVersion {
    pub major: u32,
    pub minor: u32,
}

impl MysqlVersion {
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::parse_as(Flavor::Mysql, text)
    }

    #[must_use]
    pub fn parse_as(flavor: Flavor, text: &str) -> Option<Self> {
        if Flavor::of(text) != flavor {
            return None;
        }
        let token = match flavor {
            Flavor::Mysql => {
                let text = text.split_once("Distrib ").map_or(text, |(_, rest)| rest);
                text.split_whitespace()
                    .find(|token| token.starts_with(|c: char| c.is_ascii_digit()))?
            }
            Flavor::Mariadb => {
                let (before, _) = text.split_once(MARIADB_MARKER)?;
                before
                    .rsplit(|c: char| c.is_whitespace() || c == '-')
                    .next()?
            }
        };
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

impl ToolPaths {
    #[must_use]
    pub fn for_flavor(flavor: Flavor) -> Self {
        Self {
            mysqldump: PathBuf::from(flavor.dump_tool()),
            mysql: PathBuf::from(flavor.client_tool()),
            docker: PathBuf::from("docker"),
        }
    }
}

impl Default for ToolPaths {
    fn default() -> Self {
        Self::for_flavor(Flavor::Mysql)
    }
}

#[derive(Debug, Clone)]
pub struct MysqlTools {
    flavor: Flavor,
    kind: RunnerKind,
    version: MysqlVersion,
    paths: ToolPaths,
    image: Option<String>,
}

impl MysqlTools {
    pub async fn detect(choice: RunnerChoice, server: MysqlVersion) -> Result<Self, ToolError> {
        Self::detect_flavor(Flavor::Mysql, choice, server).await
    }

    pub async fn detect_flavor(
        flavor: Flavor,
        choice: RunnerChoice,
        server: MysqlVersion,
    ) -> Result<Self, ToolError> {
        Self::detect_with_flavor(flavor, choice, server, ToolPaths::for_flavor(flavor)).await
    }

    pub async fn detect_with(
        choice: RunnerChoice,
        server: MysqlVersion,
        paths: ToolPaths,
    ) -> Result<Self, ToolError> {
        Self::detect_with_flavor(Flavor::Mysql, choice, server, paths).await
    }

    pub async fn detect_with_flavor(
        flavor: Flavor,
        choice: RunnerChoice,
        server: MysqlVersion,
        paths: ToolPaths,
    ) -> Result<Self, ToolError> {
        match choice {
            RunnerChoice::Native => Self::native(flavor, server, paths).await,
            RunnerChoice::Docker => Self::docker(flavor, server, paths).await,
            RunnerChoice::Auto => match Self::native(flavor, server, paths.clone()).await {
                Ok(tools) => Ok(tools),
                Err(native_error) => match Self::docker(flavor, server, paths).await {
                    Ok(tools) => Ok(tools),
                    Err(ToolError::DockerUnavailable) => match native_error {
                        ToolError::Incompatible { .. } => Err(native_error),
                        _ => Err(ToolError::NoRunner {
                            client: flavor.name(),
                        }),
                    },
                    Err(error) => Err(error),
                },
            },
        }
    }

    async fn native(
        flavor: Flavor,
        server: MysqlVersion,
        paths: ToolPaths,
    ) -> Result<Self, ToolError> {
        let dump_tool = flavor.dump_tool();
        let client_tool = flavor.client_tool();
        let dump = tool_version(flavor, dump_tool, &paths.mysqldump, server).await?;
        let client = tool_version(flavor, client_tool, &paths.mysql, server).await?;
        check_compatible(dump_tool, dump, server)?;
        check_compatible(client_tool, client, server)?;
        Ok(Self {
            flavor,
            kind: RunnerKind::Native,
            version: dump,
            paths,
            image: None,
        })
    }

    async fn docker(
        flavor: Flavor,
        server: MysqlVersion,
        paths: ToolPaths,
    ) -> Result<Self, ToolError> {
        if !docker_available(&paths.docker).await {
            return Err(ToolError::DockerUnavailable);
        }
        let dump_tool = flavor.dump_tool();
        let image = format!("{}:{server}", flavor.image());
        let output = Command::new(&paths.docker)
            .args([
                "run",
                "--rm",
                "--network",
                "none",
                "--entrypoint",
                dump_tool,
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
        let version = MysqlVersion::parse_as(flavor, &String::from_utf8_lossy(&output.stdout))
            .ok_or_else(|| not_found(flavor, dump_tool, server))?;
        check_compatible(dump_tool, version, server)?;
        Ok(Self {
            flavor,
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

    #[must_use]
    pub fn flavor(&self) -> Flavor {
        self.flavor
    }

    pub fn dump(
        &self,
        config: &MysqlConnectionConfig,
        password: &Password,
    ) -> Result<DumpProcess, ToolError> {
        let mut args: Vec<OsString> = dump_args(self.flavor).iter().map(OsString::from).collect();
        args.push(OsString::from(config.database()));
        self.spawn(self.flavor.dump_tool(), config, password, args, None)
    }

    pub fn restore(
        &self,
        config: &MysqlConnectionConfig,
        password: &Password,
        dump: &Path,
    ) -> Result<RestoreProcess, ToolError> {
        let tool = self.flavor.client_tool();
        let input =
            std::fs::File::open(dump).map_err(|source| ToolError::Spawn { tool, source })?;
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
        self.spawn(tool, config, password, args, Some(input))
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
                let program = if tool == self.flavor.client_tool() {
                    &self.paths.mysql
                } else {
                    &self.paths.mysqldump
                };
                let mut command = Command::new(program);
                command
                    .args(connection_args(
                        self.flavor,
                        config,
                        &options_path,
                        ca.as_deref(),
                    ))
                    .args(&extra)
                    .env_remove("MYSQL_PWD")
                    .env_remove("MYSQL_HOST")
                    .env_remove("MYSQL_TCP_PORT");
                (command, None)
            }
            RunnerKind::Docker => {
                let name = container_name(self.flavor.container_prefix());
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
                        self.flavor,
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
        self.image.as_deref().unwrap_or(self.flavor.image())
    }
}

const fn dump_args(flavor: Flavor) -> &'static [&'static str] {
    match flavor {
        Flavor::Mysql => &[
            "--single-transaction",
            "--routines",
            "--triggers",
            "--events",
            "--set-gtid-purged=OFF",
            "--no-tablespaces",
            "--hex-blob",
            "--default-character-set=utf8mb4",
            "--",
        ],
        Flavor::Mariadb => &[
            "--single-transaction",
            "--routines",
            "--triggers",
            "--events",
            "--no-tablespaces",
            "--hex-blob",
            "--default-character-set=utf8mb4",
            "--",
        ],
    }
}

fn connection_args(
    flavor: Flavor,
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
    match (flavor, ca) {
        (Flavor::Mysql, None) => {
            args.push(OsString::from("--ssl-mode=DISABLED"));
            args.push(OsString::from("--get-server-public-key"));
        }
        (Flavor::Mysql, Some(ca)) => {
            args.push(OsString::from("--ssl-mode=VERIFY_IDENTITY"));
            args.push(ca_arg(ca));
        }
        (Flavor::Mariadb, None) => args.push(OsString::from("--skip-ssl")),
        (Flavor::Mariadb, Some(ca)) => {
            args.push(OsString::from("--ssl"));
            args.push(OsString::from("--ssl-verify-server-cert"));
            args.push(ca_arg(ca));
        }
    }
    args
}

fn ca_arg(ca: &Path) -> OsString {
    let mut value = OsString::from("--ssl-ca=");
    value.push(ca);
    value
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

async fn tool_version(
    flavor: Flavor,
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
            io::ErrorKind::NotFound => not_found(flavor, tool, server),
            _ => ToolError::Spawn { tool, source },
        })?;
    if !output.status.success() {
        return Err(not_found(flavor, tool, server));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let found = Flavor::of(&text);
    if found != flavor {
        return Err(ToolError::Incompatible {
            tool,
            found: found.name().to_owned(),
            required: format!("{flavor} {server}"),
        });
    }
    MysqlVersion::parse_as(flavor, &text).ok_or_else(|| not_found(flavor, tool, server))
}

fn not_found(flavor: Flavor, tool: &'static str, server: MysqlVersion) -> ToolError {
    ToolError::NotFound {
        tool,
        required: format!("{flavor} {server}"),
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
    fn parses_mariadb_versions_and_never_takes_the_client_protocol_number() {
        let v114 = Some(MysqlVersion {
            major: 11,
            minor: 4,
        });
        let v1011 = Some(MysqlVersion {
            major: 10,
            minor: 11,
        });
        let parse = |text| MysqlVersion::parse_as(Flavor::Mariadb, text);

        assert_eq!(parse("11.4.2-MariaDB-ubu2404"), v114);
        assert_eq!(parse("10.11.6-MariaDB-1:10.11.6+maria~ubu2204-log"), v1011);
        assert_eq!(
            parse("mariadb-dump from 11.4.2-MariaDB, client 10.19 for debian-linux-gnu (x86_64)"),
            v114
        );
        assert_eq!(
            parse("mariadb  Ver 15.1 Distrib 10.11.6-MariaDB, for debian-linux-gnu (x86_64)"),
            v1011
        );
        assert_eq!(
            parse("-- MariaDB dump 10.19-11.4.2-MariaDB, for debian-linux-gnu (x86_64)"),
            v114
        );
        assert_eq!(parse("8.4.13"), None);
        assert_eq!(parse("mysqldump  Ver 8.4.11 for Linux on x86_64"), None);
    }

    #[test]
    fn mariadb_dump_has_no_gtid_option_and_mysql_dump_keeps_it() {
        assert!(dump_args(Flavor::Mysql).contains(&"--set-gtid-purged=OFF"));
        assert!(
            !dump_args(Flavor::Mariadb)
                .iter()
                .any(|arg| arg.contains("gtid"))
        );
        assert_eq!(dump_args(Flavor::Mariadb).last(), Some(&"--"));
    }

    #[test]
    fn mariadb_tls_uses_its_own_client_options() {
        let config = MysqlConnectionConfig::new(
            "db.example",
            3306,
            "app",
            "root",
            database_backup_core::TlsMode::Auto,
        )
        .expect("configuração válida");
        let options = Path::new("/tmp/dbu-x/client.cnf");
        let plain = connection_args(Flavor::Mariadb, &config, options, None);
        let verified = connection_args(
            Flavor::Mariadb,
            &config,
            options,
            Some(Path::new("/run/dbu/ca.pem")),
        );

        assert!(plain.contains(&OsString::from("--skip-ssl")));
        assert!(verified.contains(&OsString::from("--ssl-verify-server-cert")));
        assert!(verified.contains(&OsString::from("--ssl-ca=/run/dbu/ca.pem")));
        for arg in plain.iter().chain(&verified) {
            let arg = arg.to_string_lossy();
            assert!(
                !arg.starts_with("--ssl-mode") && !arg.contains("public-key"),
                "{arg}"
            );
        }
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
        let args = connection_args(
            Flavor::Mysql,
            &config,
            Path::new("/tmp/dbu-x/client.cnf"),
            None,
        );

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
