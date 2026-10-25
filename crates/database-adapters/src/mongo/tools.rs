use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use database_backup_core::{MongoConnectionConfig, Password, TlsRequirement};
use tempfile::TempDir;
use tokio::process::Command;

use super::connection::MongoVersion;
use crate::process::{
    DumpProcess, Launch, RestoreProcess, RunnerChoice, RunnerKind, ToolError, ToolProcess,
    absolute, ca_bundle, container_name, docker_available, exit_code, mount, sanitize, secret_dir,
    tail, write_private,
};

const CONTAINER_CONFIG: &str = "/run/dbu/mongo-tools.yaml";
const CONTAINER_CA: &str = "/run/dbu/ca.pem";
const CONTAINER_PREFIX: &str = "dbu-mg-";
const CLIENT: &str = "MongoDB Database Tools";
const CONFIG_FILE: &str = "mongo-tools.yaml";
const MIN_TOOLS: ToolsVersion = ToolsVersion::new(100, 3, 0);
const COMPATIBILITY: [(MongoVersion, ToolsVersion); 9] = [
    (MongoVersion { major: 4, minor: 4 }, MIN_TOOLS),
    (
        MongoVersion { major: 5, minor: 0 },
        ToolsVersion::new(100, 4, 0),
    ),
    (
        MongoVersion { major: 6, minor: 0 },
        ToolsVersion::new(100, 5, 0),
    ),
    (
        MongoVersion { major: 7, minor: 0 },
        ToolsVersion::new(100, 8, 0),
    ),
    (
        MongoVersion { major: 8, minor: 0 },
        ToolsVersion::new(100, 10, 0),
    ),
    (
        MongoVersion { major: 8, minor: 1 },
        ToolsVersion::new(100, 12, 0),
    ),
    (
        MongoVersion { major: 8, minor: 2 },
        ToolsVersion::new(100, 13, 0),
    ),
    (
        MongoVersion { major: 8, minor: 3 },
        ToolsVersion::new(100, 16, 0),
    ),
    (
        MongoVersion { major: 9, minor: 0 },
        ToolsVersion::new(100, 18, 0),
    ),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ToolsVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl ToolsVersion {
    const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = version_payload(text)?.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = leading_number(parts.next()?)?;
        Some(Self {
            major,
            minor,
            patch,
        })
    }

    #[must_use]
    pub fn required_for(server: MongoVersion) -> Self {
        COMPATIBILITY
            .iter()
            .rev()
            .find(|(listed, _)| *listed <= server)
            .map_or(MIN_TOOLS, |(_, tools)| *tools)
    }
}

impl fmt::Display for ToolsVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[must_use]
pub const fn latest_known_server() -> MongoVersion {
    COMPATIBILITY[COMPATIBILITY.len() - 1].0
}

fn version_payload(text: &str) -> Option<&str> {
    text.lines().find_map(|line| {
        let (label, payload) = line.split_once("version:")?;
        matches!(label.trim(), "mongodump" | "mongorestore").then(|| payload.trim())
    })
}

fn leading_number(text: &str) -> Option<u32> {
    let digits: String = text.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

fn legacy_version(text: &str) -> Option<&str> {
    let payload = version_payload(text)?;
    let mut chars = payload.chars();
    (chars.next() == Some('r') && chars.next().is_some_and(|c| c.is_ascii_digit()))
        .then_some(payload)
}

#[derive(Debug, Clone)]
pub struct ToolPaths {
    pub mongodump: PathBuf,
    pub mongorestore: PathBuf,
    pub docker: PathBuf,
}

impl Default for ToolPaths {
    fn default() -> Self {
        Self {
            mongodump: PathBuf::from("mongodump"),
            mongorestore: PathBuf::from("mongorestore"),
            docker: PathBuf::from("docker"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MongoTools {
    kind: RunnerKind,
    version: ToolsVersion,
    paths: ToolPaths,
    image: Option<String>,
}

impl MongoTools {
    pub async fn detect(choice: RunnerChoice, server: MongoVersion) -> Result<Self, ToolError> {
        Self::detect_with(choice, server, ToolPaths::default()).await
    }

    pub async fn detect_with(
        choice: RunnerChoice,
        server: MongoVersion,
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

    async fn native(server: MongoVersion, paths: ToolPaths) -> Result<Self, ToolError> {
        let dump = tool_version("mongodump", &paths.mongodump, server).await?;
        let restore = tool_version("mongorestore", &paths.mongorestore, server).await?;
        check_compatible("mongodump", dump, server)?;
        check_compatible("mongorestore", restore, server)?;
        Ok(Self {
            kind: RunnerKind::Native,
            version: dump,
            paths,
            image: None,
        })
    }

    async fn docker(server: MongoVersion, paths: ToolPaths) -> Result<Self, ToolError> {
        if !docker_available(&paths.docker).await {
            return Err(ToolError::DockerUnavailable);
        }
        let image = format!("mongo:{server}");
        let output = Command::new(&paths.docker)
            .args([
                "run",
                "--rm",
                "--network",
                "none",
                "--entrypoint",
                "mongodump",
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
        let version = parse_output(
            "mongodump",
            &String::from_utf8_lossy(&output.stdout),
            server,
        )?;
        check_compatible("mongodump", version, server)?;
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
    pub fn version(&self) -> ToolsVersion {
        self.version
    }

    pub fn dump(
        &self,
        config: &MongoConnectionConfig,
        password: &Password,
    ) -> Result<DumpProcess, ToolError> {
        self.spawn("mongodump", config, password, dump_args(config), None)
    }

    pub fn restore(
        &self,
        target: &MongoConnectionConfig,
        password: &Password,
        archive: &Path,
        source_database: &str,
    ) -> Result<RestoreProcess, ToolError> {
        let input = std::fs::File::open(archive).map_err(|source| ToolError::Spawn {
            tool: "mongorestore",
            source,
        })?;
        self.spawn(
            "mongorestore",
            target,
            password,
            restore_args(source_database, target.database()),
            Some(input),
        )
    }

    fn spawn(
        &self,
        tool: &'static str,
        config: &MongoConnectionConfig,
        password: &Password,
        extra: Vec<OsString>,
        input: Option<std::fs::File>,
    ) -> Result<ToolProcess, ToolError> {
        let secrets = write_config(password)?;
        let config_path = secrets.path().join(CONFIG_FILE);
        let ca = match config.tls() {
            TlsRequirement::Plain => None,
            TlsRequirement::VerifyFull => Some(ca_bundle()?),
        };

        let (command, container) = match self.kind {
            RunnerKind::Native => {
                let program = match tool {
                    "mongorestore" => &self.paths.mongorestore,
                    _ => &self.paths.mongodump,
                };
                let mut command = Command::new(program);
                command
                    .args(connection_args(config, &config_path, ca.as_deref()))
                    .args(&extra);
                (command, None)
            }
            RunnerKind::Docker => {
                let name = container_name(CONTAINER_PREFIX);
                let mut command = Command::new(&self.paths.docker);
                command.args(["run", "--rm", "-i", "--name", &name]);
                if config.is_local() {
                    command.args(["--network", "host"]);
                }
                command.arg("-v").arg(mount(&config_path, CONTAINER_CONFIG));
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
                        Path::new(CONTAINER_CONFIG),
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
                config_path.display().to_string(),
            ],
            secrets,
        })
    }

    fn image(&self) -> &str {
        self.image.as_deref().unwrap_or("mongo")
    }
}

fn dump_args(config: &MongoConnectionConfig) -> Vec<OsString> {
    vec![
        OsString::from(format!("--db={}", config.database())),
        OsString::from("--archive"),
    ]
}

fn restore_args(source_database: &str, target_database: &str) -> Vec<OsString> {
    vec![
        OsString::from("--archive"),
        OsString::from(format!("--nsInclude={source_database}.*")),
        OsString::from(format!("--nsFrom={source_database}.*")),
        OsString::from(format!("--nsTo={target_database}.*")),
        OsString::from("--stopOnError"),
        OsString::from("--bypassDocumentValidation"),
    ]
}

fn connection_args(
    config: &MongoConnectionConfig,
    config_file: &Path,
    ca: Option<&Path>,
) -> Vec<OsString> {
    let mut file = OsString::from("--config=");
    file.push(config_file);
    let mut args = vec![
        file,
        OsString::from(format!("--host={}", config.host())),
        OsString::from(format!("--port={}", config.port())),
        OsString::from(format!("--username={}", config.user())),
        OsString::from(format!(
            "--authenticationDatabase={}",
            config.auth_database()
        )),
    ];
    if let Some(ca) = ca {
        args.push(OsString::from("--tls"));
        let mut value = OsString::from("--tlsCAFile=");
        value.push(ca);
        args.push(value);
    }
    args
}

fn write_config(password: &Password) -> Result<TempDir, ToolError> {
    let dir = secret_dir()?;
    let content = format!("password: {}\n", yaml_quote(password.expose()));
    write_private(&dir.path().join(CONFIG_FILE), &content)?;
    Ok(dir)
}

fn yaml_quote(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for c in value.chars() {
        match c {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            c if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}' | '\u{feff}') => {
                quoted.push_str(&format!("\\u{:04X}", u32::from(c)));
            }
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted
}

async fn tool_version(
    tool: &'static str,
    path: &Path,
    server: MongoVersion,
) -> Result<ToolsVersion, ToolError> {
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
    parse_output(tool, &String::from_utf8_lossy(&output.stdout), server)
}

fn parse_output(
    tool: &'static str,
    text: &str,
    server: MongoVersion,
) -> Result<ToolsVersion, ToolError> {
    if let Some(version) = ToolsVersion::parse(text) {
        return Ok(version);
    }
    match legacy_version(text) {
        Some(found) => Err(ToolError::Incompatible {
            tool,
            found: format!("{found} (ferramentas legadas)"),
            required: ToolsVersion::required_for(server).to_string(),
        }),
        None => Err(not_found(tool, server)),
    }
}

fn not_found(tool: &'static str, server: MongoVersion) -> ToolError {
    ToolError::NotFound {
        tool,
        required: format!("{CLIENT} {}", ToolsVersion::required_for(server)),
    }
}

fn check_compatible(
    tool: &'static str,
    found: ToolsVersion,
    server: MongoVersion,
) -> Result<(), ToolError> {
    let required = ToolsVersion::required_for(server);
    if found >= required {
        Ok(())
    } else {
        Err(ToolError::Incompatible {
            tool,
            found: found.to_string(),
            required: required.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use database_backup_core::TlsMode;

    use super::*;

    fn config(host: &str, database: &str) -> MongoConnectionConfig {
        MongoConnectionConfig::new(host, 27017, database, "backup", "admin", TlsMode::Auto)
            .expect("configuração válida")
    }

    #[test]
    fn parses_tools_versions() {
        let current = "mongodump version: 100.19.1\ngit version: 9348e032419d906e40966f477278cce647b9b1eb\nGo version: go1.26.8\n   os: linux\n   arch: amd64\n   compiler: gc\n";
        let restore = "mongorestore version: 100.10.0\ngit version: 6d4f001be3fcf673de04d20176e90ee02ef233a9\n";
        let legacy = "mongodump version: r4.2.24\ngit version: 5e4ec1d24431fcdd28b579a024c5c801b8cde4e2\nGo version: go1.12.17\n";

        assert_eq!(
            ToolsVersion::parse(current),
            Some(ToolsVersion::new(100, 19, 1))
        );
        assert_eq!(
            ToolsVersion::parse(restore),
            Some(ToolsVersion::new(100, 10, 0))
        );
        assert_eq!(
            ToolsVersion::parse("mongodump version: 100.3.0-rc1\n"),
            Some(ToolsVersion::new(100, 3, 0))
        );
        assert_eq!(ToolsVersion::parse(legacy), None);
        assert_eq!(legacy_version(legacy), Some("r4.2.24"));
        assert_eq!(legacy_version(current), None);
        for text in [
            "",
            "mongodump version: \n",
            "git version: abc\nGo version: go1.22\n",
            "100.10.0",
            "mongodump version: 100.x.1\n",
        ] {
            assert_eq!(ToolsVersion::parse(text), None, "{text:?}");
        }
        assert!(ToolsVersion::new(100, 9, 4) < ToolsVersion::new(100, 10, 0));
        assert_eq!(ToolsVersion::new(100, 10, 0).to_string(), "100.10.0");
    }

    #[test]
    fn required_tools_follow_the_server_version() {
        let required = |major, minor| ToolsVersion::required_for(MongoVersion { major, minor });

        assert_eq!(required(8, 0), ToolsVersion::new(100, 10, 0));
        assert_eq!(required(7, 0), ToolsVersion::new(100, 8, 0));
        assert_eq!(required(6, 0), ToolsVersion::new(100, 5, 0));
        assert_eq!(required(5, 0), ToolsVersion::new(100, 4, 0));
        assert_eq!(required(4, 4), ToolsVersion::new(100, 3, 0));
        assert_eq!(required(4, 2), ToolsVersion::new(100, 3, 0));
        assert_eq!(required(8, 2), ToolsVersion::new(100, 13, 0));
        assert_eq!(required(9, 0), ToolsVersion::new(100, 18, 0));
        assert_eq!(required(11, 3), ToolsVersion::new(100, 18, 0));
        assert_eq!(latest_known_server(), MongoVersion { major: 9, minor: 0 });

        let server = MongoVersion { major: 8, minor: 0 };
        assert!(check_compatible("mongodump", ToolsVersion::new(100, 10, 0), server).is_ok());
        assert!(check_compatible("mongodump", ToolsVersion::new(100, 19, 1), server).is_ok());
        assert!(matches!(
            check_compatible("mongodump", ToolsVersion::new(100, 9, 4), server),
            Err(ToolError::Incompatible { ref required, .. }) if required == "100.10.0"
        ));
    }

    #[test]
    fn legacy_tools_are_incompatible_and_unknown_output_is_not_found() {
        let server = MongoVersion { major: 8, minor: 0 };

        let legacy = parse_output("mongodump", "mongodump version: r4.2.24\n", server);
        let unknown = parse_output("mongodump", "outra coisa\n", server);

        assert!(matches!(
            legacy,
            Err(ToolError::Incompatible { ref found, ref required, .. })
                if found == "r4.2.24 (ferramentas legadas)" && required == "100.10.0"
        ));
        assert!(matches!(unknown, Err(ToolError::NotFound { .. })));
    }

    #[test]
    fn yaml_values_are_quoted_and_escaped() {
        assert_eq!(yaml_quote("simples"), "\"simples\"");
        assert_eq!(yaml_quote(r#"a"b\c"#), r#""a\"b\\c""#);
        assert_eq!(yaml_quote("x\ny\r\tz"), "\"x\\ny\\r\\tz\"");
        assert_eq!(yaml_quote("a\u{1}b\u{7f}c"), "\"a\\u0001b\\u007Fc\"");
        assert_eq!(
            yaml_quote("a\u{85}b\u{2028}c\u{2029}d\u{feff}e"),
            "\"a\\u0085b\\u2028c\\u2029d\\uFEFFe\""
        );
        assert_eq!(
            yaml_quote("#: 'x' %$ç🚀 {}[]&*!|>"),
            "\"#: 'x' %$ç🚀 {}[]&*!|>\""
        );
        assert_eq!(yaml_quote(""), "\"\"");
    }

    #[cfg(unix)]
    #[test]
    fn config_file_is_private_escaped_and_removed_on_drop() {
        use std::os::unix::fs::PermissionsExt;

        let dir = write_config(&Password::new("p\"w\\x".to_owned())).expect("arquivo");
        let path = dir.path().join(CONFIG_FILE);
        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode();
        let content = std::fs::read_to_string(&path).expect("conteúdo");
        drop(dir);

        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(content, "password: \"p\\\"w\\\\x\"\n");
        assert!(!path.exists());
    }

    #[test]
    fn connection_args_start_with_the_config_file_and_never_carry_the_password() {
        let secret = "senha-secreta";
        let local = connection_args(
            &config("localhost", "-app"),
            Path::new("/tmp/dbu-x/mongo-tools.yaml"),
            None,
        );
        let remote_config = config("db.example.com", "app");
        let remote = connection_args(
            &remote_config,
            Path::new("/run/dbu/mongo-tools.yaml"),
            Some(Path::new("/run/dbu/ca.pem")),
        );

        assert_eq!(local[0], "--config=/tmp/dbu-x/mongo-tools.yaml");
        assert_eq!(
            &local[1..],
            [
                "--host=localhost",
                "--port=27017",
                "--username=backup",
                "--authenticationDatabase=admin",
            ]
            .map(OsString::from)
        );
        assert_eq!(remote_config.tls(), TlsRequirement::VerifyFull);
        assert_eq!(remote[0], "--config=/run/dbu/mongo-tools.yaml");
        assert!(remote.contains(&OsString::from("--tls")));
        assert!(remote.contains(&OsString::from("--tlsCAFile=/run/dbu/ca.pem")));
        for args in [&local, &remote] {
            for arg in args {
                let text = arg.to_string_lossy().to_lowercase();
                assert!(!text.contains(secret));
                assert!(!text.contains("--password"), "{text}");
                assert!(!text.contains("insecure"), "{text}");
                assert!(!text.contains("allowinvalid"), "{text}");
            }
        }
    }

    #[test]
    fn dump_takes_one_database_without_oplog() {
        assert_eq!(
            dump_args(&config("localhost", "-app")),
            ["--db=-app", "--archive"].map(OsString::from)
        );
    }

    #[test]
    fn restore_renames_only_the_source_database() {
        let args = restore_args("app", "copia");

        assert_eq!(
            args,
            [
                "--archive",
                "--nsInclude=app.*",
                "--nsFrom=app.*",
                "--nsTo=copia.*",
                "--stopOnError",
                "--bypassDocumentValidation",
            ]
            .map(OsString::from)
        );
        for arg in &args {
            let text = arg.to_string_lossy();
            assert!(!text.contains("drop"), "{text}");
            assert!(!text.contains("oplog"), "{text}");
        }
    }
}
