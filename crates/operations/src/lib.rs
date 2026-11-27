mod cancellation;
mod error;
mod output;
mod progress;

pub mod archive;
pub mod keychain;
pub mod mongo;
pub mod mysql;
pub mod postgres;

pub use cancellation::Cancellation;
pub use error::{ErrorCode, OperationError, describe};
pub use output::{commit, open_target, timestamp};
pub use progress::{Progress, ProgressSink, Stage};

pub const TOOL_NAME: &str = "database-backup";
