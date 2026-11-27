use std::error::Error;

use database_backup_adapters::postgres::ConnectionError;
use database_backup_core::SecretError;
use database_backup_storage::StorageError;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    InvalidInput,
    AuthFailed,
    DatabaseNotFound,
    Unreachable,
    TlsFailed,
    ToolUnavailable,
    EngineMismatch,
    ServerMismatch,
    BackupFailed,
    Canceled,
    BackupMissing,
    BackupCorrupted,
    ChecksumMissing,
    BackupIncompatible,
    RestoreFailed,
    ConfirmationRequired,
    KeychainUnavailable,
    Internal,
}

impl ErrorCode {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidInput => "invalid_input",
            Self::AuthFailed => "auth_failed",
            Self::DatabaseNotFound => "database_not_found",
            Self::Unreachable => "unreachable",
            Self::TlsFailed => "tls_failed",
            Self::ToolUnavailable => "tool_unavailable",
            Self::EngineMismatch | Self::ServerMismatch => "engine_mismatch",
            Self::BackupFailed => "backup_failed",
            Self::Canceled => "cancelled",
            Self::BackupMissing => "backup_missing",
            Self::BackupCorrupted => "backup_corrupted",
            Self::ChecksumMissing => "checksum_missing",
            Self::BackupIncompatible => "backup_incompatible",
            Self::RestoreFailed => "restore_failed",
            Self::ConfirmationRequired => "confirmation_required",
            Self::KeychainUnavailable => "keychain_unavailable",
            Self::Internal => "internal",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{message}")]
pub struct OperationError {
    code: ErrorCode,
    message: String,
}

impl OperationError {
    #[must_use]
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    #[must_use]
    pub fn code(&self) -> ErrorCode {
        self.code
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    #[must_use]
    pub fn secret(error: &SecretError) -> Self {
        let code = match error {
            SecretError::KeychainUnavailable(_) => ErrorCode::KeychainUnavailable,
            _ => ErrorCode::InvalidInput,
        };
        Self::new(code, error.to_string())
    }

    pub(crate) fn from_source(code: ErrorCode, error: &dyn Error) -> Self {
        Self::new(code, describe(error))
    }

    pub(crate) fn internal(error: &dyn Error) -> Self {
        Self::from_source(ErrorCode::Internal, error)
    }

    pub(crate) fn connection(error: &ConnectionError) -> Self {
        let code = match error {
            ConnectionError::AuthenticationFailed { .. } => ErrorCode::AuthFailed,
            ConnectionError::DatabaseNotFound { .. } => ErrorCode::DatabaseNotFound,
            ConnectionError::Unreachable { .. } | ConnectionError::Timeout { .. } => {
                ErrorCode::Unreachable
            }
            ConnectionError::Tls { .. } => ErrorCode::TlsFailed,
            ConnectionError::Other { .. } => ErrorCode::Internal,
        };
        Self::new(code, error.to_string())
    }

    pub(crate) fn source_file(error: &StorageError) -> Self {
        let code = match error {
            StorageError::SourceNotFound(_) => ErrorCode::BackupMissing,
            StorageError::Unreadable
            | StorageError::ContentMismatch(_)
            | StorageError::ChecksumMismatch { .. }
            | StorageError::SizeMismatch { .. } => ErrorCode::BackupCorrupted,
            _ => ErrorCode::BackupIncompatible,
        };
        Self::from_source(code, error)
    }
}

#[must_use]
pub fn describe(error: &dyn Error) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_unreachable_keychain_gets_its_own_code() {
        let offline =
            OperationError::secret(&SecretError::KeychainUnavailable("sem D-Bus".to_owned()));
        assert_eq!(offline.code().as_str(), "keychain_unavailable");
        assert!(offline.message().contains("--password-file"));

        let missing = OperationError::secret(&SecretError::KeychainMissing("prod".to_owned()));
        assert_eq!(missing.code(), ErrorCode::InvalidInput);
    }
}
