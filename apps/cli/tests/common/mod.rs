#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use sha2::{Digest, Sha256};
use tempfile::TempDir;

use crate::support::{PostgresTestDb, TestResult};

pub struct Workspace {
    _dir: TempDir,
    pub data: PathBuf,
    pub out: PathBuf,
    pub tmp: PathBuf,
}

impl Workspace {
    pub fn new() -> TestResult<Self> {
        let dir = TempDir::new()?;
        let data = dir.path().join("data");
        let out = dir.path().join("out");
        let tmp = dir.path().join("tmp");
        std::fs::create_dir(&tmp)?;
        Ok(Self {
            _dir: dir,
            data,
            out,
            tmp,
        })
    }

    pub fn cli(&self, password: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_database-backup"));
        command
            .env("DBU_DATA_DIR", &self.data)
            .env("TMPDIR", &self.tmp)
            .env_remove("RUST_LOG")
            .env("DBU_PG_PASSWORD", password)
            .stdin(Stdio::null());
        command
    }

    pub fn command(&self, db: &PostgresTestDb, user: &str, password: &str) -> Command {
        let mut command = self.cli(password);
        command
            .args(["backup", "run", "--runner", "docker", "--host", db.host()])
            .args(["--port", &db.port().to_string()])
            .args(["--database", db.database(), "--user", user])
            .arg("--output-dir")
            .arg(&self.out);
        command
    }

    pub fn restore(&self, db: &PostgresTestDb, file: &Path, target: &str) -> Command {
        let mut command = self.cli(db.password());
        command
            .args(["restore", "run", "--runner", "docker", "--host", db.host()])
            .args(["--port", &db.port().to_string()])
            .args(["--user", db.user(), "--target-database", target])
            .arg("--file")
            .arg(file);
        command
    }

    pub fn files(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.out)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    pub fn dump_path(&self) -> PathBuf {
        let name = self
            .files()
            .into_iter()
            .find(|name| name.ends_with(".dump"))
            .expect("um .dump gerado");
        self.out.join(name)
    }

    pub fn history(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.data.join("history.sqlite3")).expect("histórico")
    }
}

pub fn sha256_hex(path: &Path) -> String {
    let digest = Sha256::digest(std::fs::read(path).expect("dump"));
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
