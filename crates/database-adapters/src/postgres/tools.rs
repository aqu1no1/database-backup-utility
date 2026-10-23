use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use database_backup_core::{Password, PostgresConnectionConfig, TlsRequirement};
use tempfile::TempDir;
use tokio::process::Command;

use crate::process::{
    DumpProcess, Launch, RestoreProcess, RunnerChoice, RunnerKind, ToolError, ToolProcess,
    absolute, container_name, docker_available, exit_code, mount, sanitize, secret_dir, tail,
    write_private,
};

const CONTAINER_PGPASS: &str = "/run/dbu/pgpass";
const CONTAINER_ARCHIVE: &str = "/run/dbu/archive.dump";
const CONTAINER_PREFIX: &str = "dbu-pg-";
const CLIENT: &str = "PostgreSQL";

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
                        _ => Err(ToolError::NoRunner { client: CLIENT }),
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
        if !docker_available(&paths.docker).await {
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
        let version = PgVersion::parse(&String::from_utf8_lossy(&output.stdout))
            .ok_or_else(|| not_found("pg_dump", server.major))?;
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

        let (command, container) = match self.kind {
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
                let name = container_name(CONTAINER_PREFIX);
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

        ToolProcess::spawn(Launch {
            tool,
            command,
            stdin: Stdio::null(),
            capture_stdout,
            container,
            docker: self.paths.docker.clone(),
            redact: vec![
                password.expose().to_owned(),
                secrets.path().display().to_string(),
            ],
            secrets: secrets.dir,
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

struct PgPass {
    dir: TempDir,
}

impl PgPass {
    fn write(config: &PostgresConnectionConfig, password: &Password) -> Result<Self, ToolError> {
        let dir = secret_dir()?;
        let line = format!(
            "*:*:*:{}:{}\n",
            escape_pgpass(config.user()),
            escape_pgpass(password.expose()),
        );
        write_private(&dir.path().join("pgpass"), &line)?;
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
            io::ErrorKind::NotFound => not_found(tool, required),
            _ => ToolError::Spawn { tool, source },
        })?;
    if !output.status.success() {
        return Err(not_found(tool, required));
    }
    PgVersion::parse(&String::from_utf8_lossy(&output.stdout))
        .ok_or_else(|| not_found(tool, required))
}

fn not_found(tool: &'static str, required: u32) -> ToolError {
    ToolError::NotFound {
        tool,
        required: format!("{CLIENT} {required}"),
    }
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
            found: found.to_string(),
            required: server.major.to_string(),
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

fn ssl_mode(tls: TlsRequirement) -> &'static str {
    match tls {
        TlsRequirement::Plain => "disable",
        TlsRequirement::VerifyFull => "verify-full",
    }
}

fn escape_pgpass(value: &str) -> String {
    value.replace('\\', "\\\\").replace(':', "\\:")
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
            Err(ToolError::Incompatible { ref required, .. }) if required == "17"
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
