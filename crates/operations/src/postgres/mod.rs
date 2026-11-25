pub mod backup;
pub mod restore;

use database_backup_adapters::postgres::{PgTools, PgVersion, RunnerChoice};

use crate::{ErrorCode, OperationError};

async fn detect_tools(
    runner: RunnerChoice,
    server_version: &str,
) -> Result<PgTools, OperationError> {
    let version = PgVersion::parse(server_version).ok_or_else(|| {
        OperationError::new(
            ErrorCode::Internal,
            format!("versão do servidor não reconhecida: {server_version}"),
        )
    })?;
    PgTools::detect(runner, version)
        .await
        .map_err(|error| OperationError::new(ErrorCode::ToolUnavailable, error.to_string()))
}

#[cfg(test)]
mod tests {
    use database_backup_core::SecretProvider;

    use super::{backup, restore};
    use crate::{Cancellation, ProgressSink};

    fn assert_send<T: Send>(_: T) {}

    #[allow(dead_code)]
    fn backup_futures_are_send(
        request: backup::BackupRequest,
        prepared: backup::PreparedBackup,
        cancellation: &Cancellation,
        progress: &dyn ProgressSink,
    ) {
        assert_send(backup::prepare(request));
        assert_send(prepared.run(cancellation, progress));
    }

    #[allow(dead_code)]
    fn restore_futures_are_send(
        request: restore::RestoreRequest,
        prepared: restore::PreparedRestore,
        secrets: &(dyn SecretProvider + Sync),
        cancellation: &Cancellation,
    ) {
        assert_send(restore::prepare(request, secrets));
        assert_send(prepared.run(None, cancellation));
    }
}
