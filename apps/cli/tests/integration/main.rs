#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../common/mod.rs"]
mod common;
#[path = "../../../../crates/database-adapters/tests/support/mod.rs"]
mod support;

mod backup;
mod cli;
mod mongo;
mod mysql;
mod profiles;
mod restore;
mod sqlite;
mod test_connection;
