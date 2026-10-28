use std::fmt::Write as _;
use std::process::ExitCode;

use clap::Args;
use database_backup_history::{BackupSummary, History, HistoryError, RestoreRecord};
use serde::Serialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::backup::human_size;
use crate::output::{fail, success};
use crate::{describe, exit};

#[derive(Debug, Args)]
pub struct BackupListArgs {
    #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..=1000), help = "Quantidade máxima de registros")]
    limit: u32,
    #[arg(short, long, help = "Mostra só os backups deste banco")]
    database: Option<String>,
}

#[derive(Debug, Args)]
pub struct RestoreListArgs {
    #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..=1000), help = "Quantidade máxima de registros")]
    limit: u32,
}

#[derive(Serialize)]
struct BackupRow {
    execution_id: String,
    database: String,
    host: Option<String>,
    port: Option<u16>,
    status: String,
    started_at: String,
    duration_ms: Option<i64>,
    file: Option<String>,
    size_bytes: Option<u64>,
    sha256: Option<String>,
    error: Option<String>,
}

#[derive(Serialize)]
struct RestoreRow {
    restore_id: String,
    source: String,
    target_database: String,
    mode: String,
    previous_database: Option<String>,
    checksum_verified: bool,
    status: String,
    started_at: String,
    duration_ms: Option<i64>,
    error: Option<String>,
}

#[derive(Serialize)]
struct Backups {
    backups: Vec<BackupRow>,
}

#[derive(Serialize)]
struct Restores {
    restores: Vec<RestoreRow>,
}

pub fn backups(args: &BackupListArgs) -> ExitCode {
    let records = match open()
        .and_then(|history| history.list_backups(args.limit, args.database.as_deref()))
    {
        Ok(records) => records,
        Err(error) => return fail(exit::FAILURE, &describe(&error)),
    };
    let rows: Vec<BackupRow> = records.into_iter().map(BackupRow::from).collect();

    let mut text = String::new();
    if rows.is_empty() {
        text.push_str("Nenhum backup registrado.");
    }
    for row in &rows {
        let size = row.size_bytes.map(human_size).unwrap_or_default();
        let detail = row
            .file
            .as_deref()
            .or(row.error.as_deref())
            .unwrap_or_default();
        let _ = writeln!(
            text,
            "{}  {:<9}  {:<20}  {:>9}  {}",
            row.started_at, row.status, row.database, size, detail
        );
    }
    success(&Backups { backups: rows }, text.trim_end())
}

pub fn restores(args: &RestoreListArgs) -> ExitCode {
    let records = match open().and_then(|history| history.list_restores(args.limit)) {
        Ok(records) => records,
        Err(error) => return fail(exit::FAILURE, &describe(&error)),
    };
    let rows: Vec<RestoreRow> = records.into_iter().map(RestoreRow::from).collect();

    let mut text = String::new();
    if rows.is_empty() {
        text.push_str("Nenhum restore registrado.");
    }
    for row in &rows {
        let verified = if row.checksum_verified {
            ""
        } else {
            "  [checksum não verificado]"
        };
        let _ = writeln!(
            text,
            "{}  {:<9}  {:<9}  {:<20}  {}{verified}",
            row.started_at, row.status, row.mode, row.target_database, row.source
        );
    }
    success(&Restores { restores: rows }, text.trim_end())
}

fn open() -> Result<History, HistoryError> {
    History::default_path().and_then(|path| History::open(&path))
}

fn timestamp(value: OffsetDateTime) -> String {
    value.format(&Rfc3339).unwrap_or_default()
}

impl From<BackupSummary> for BackupRow {
    fn from(record: BackupSummary) -> Self {
        Self {
            execution_id: record.execution_id,
            database: record.database,
            host: record.host,
            port: record.port,
            status: record.status.to_string(),
            started_at: timestamp(record.started_at),
            duration_ms: record.duration_ms,
            file: record.location,
            size_bytes: record.size_bytes,
            sha256: record.checksum_sha256,
            error: record.error_message,
        }
    }
}

impl From<RestoreRecord> for RestoreRow {
    fn from(record: RestoreRecord) -> Self {
        Self {
            restore_id: record.id,
            source: record.source_location,
            target_database: record.target_database,
            mode: record.mode,
            previous_database: record.previous_database,
            checksum_verified: record.checksum_verified,
            status: record.status.to_string(),
            started_at: timestamp(record.started_at),
            duration_ms: record.duration_ms,
            error: record.error_message,
        }
    }
}
