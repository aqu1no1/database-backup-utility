use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use database_backup_history::RestoreMode;
use database_backup_operations::Progress;
use database_backup_operations::sqlite::{
    self as operation, BackupRequest, prepare_backup, prepare_restore,
};
use database_backup_storage::Encoding;

use crate::backup::{BackupResult, human_size};
use crate::output::{fail, success};
use crate::restore::{RestoreResult, confirm_overwrite};
use crate::{exit, fail_with, interrupt};

pub struct RestoreRequest<'a> {
    pub file: &'a Path,
    pub source: &'a Path,
    pub target: &'a Path,
    pub overwrite: bool,
    pub confirm: Option<&'a str>,
    pub allow_unverified: bool,
    pub timeout: Option<u64>,
}

pub async fn backup(
    source: &Path,
    output_dir: Option<PathBuf>,
    timeout: Option<u64>,
    encoding: &Encoding,
) -> ExitCode {
    let prepared = match prepare_backup(BackupRequest {
        source: source.to_owned(),
        output_dir,
        timeout: timeout.map(Duration::from_secs),
        encoding: encoding.clone(),
    })
    .await
    {
        Ok(prepared) => prepared,
        Err(error) => return fail_with(&error),
    };
    let outcome = match prepared.run(&interrupt(), &|_: Progress| {}).await {
        Ok(outcome) => outcome,
        Err(error) => return fail_with(&error),
    };
    let artifact = &outcome.artifact;
    let mut text = format!(
        "Backup concluído: {} ({}, sha256 {}…) em {:.1}s",
        artifact.path.display(),
        human_size(artifact.size_bytes),
        &artifact.sha256[..12],
        outcome.elapsed.as_secs_f64()
    );
    if let Some(summary) = crate::sealing::summary(artifact) {
        text.push_str(&format!("\n{summary}"));
    }
    success(
        &BackupResult {
            execution_id: &outcome.execution_id,
            database: &outcome.database,
            file: artifact.path.display().to_string(),
            size_bytes: artifact.size_bytes,
            sha256: &artifact.sha256,
            duration_ms: outcome.elapsed.as_millis(),
            non_transactional_tables: None,
            topology: None,
            compression: crate::sealing::compression_name(artifact),
            encryption: crate::sealing::encryption_name(artifact),
        },
        &text,
    )
}

pub async fn restore(request: &RestoreRequest<'_>) -> ExitCode {
    let file = match std::path::absolute(request.file) {
        Ok(path) => path,
        Err(error) => return fail(exit::INVALID_ARCHIVE, &error.to_string()),
    };
    let prepared = match prepare_restore(operation::RestoreRequest {
        file,
        source: request.source.to_owned(),
        target: request.target.to_owned(),
        overwrite: request.overwrite,
        allow_unverified: request.allow_unverified,
        timeout: request.timeout.map(Duration::from_secs),
    })
    .await
    {
        Ok(prepared) => prepared,
        Err(error) => return fail_with(&error),
    };
    let name = request.target.display().to_string();
    let confirmation = match prepared.mode() {
        RestoreMode::Create => None,
        RestoreMode::Overwrite => {
            if let Err(code) = confirm_overwrite(
                &name,
                &format!(
                    "O arquivo {name} será substituído; o atual será mantido como cópia.\nDigite o caminho do arquivo para confirmar: "
                ),
                request.confirm,
            ) {
                return code;
            }
            Some(name.as_str())
        }
    };
    let outcome = match prepared.run(confirmation, &interrupt()).await {
        Ok(outcome) => outcome,
        Err(error) => return fail_with(&error),
    };
    let mut text = format!(
        "Restore concluído em {} ({} objetos) em {:.1}s",
        outcome.database,
        outcome.objects,
        outcome.elapsed.as_secs_f64()
    );
    if let Some(previous) = &outcome.previous_database {
        text.push_str(&format!("\nArquivo anterior mantido como {previous}"));
    }
    success(
        &RestoreResult {
            restore_id: &outcome.restore_id,
            database: &outcome.database,
            mode: outcome.mode.as_str(),
            previous_database: outcome.previous_database.as_deref(),
            objects: outcome.objects,
            checksum_verified: outcome.checksum_verified,
            duration_ms: outcome.elapsed.as_millis(),
        },
        &text,
    )
}
