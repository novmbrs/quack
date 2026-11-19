//! Quack's PostgreSQL storage.
//!
//! Migrations, every query (sqlx macros checked against the schema, offline
//! data in `.sqlx/`), transactions, the job queue (`FOR UPDATE SKIP LOCKED`
//! plus `LISTEN/NOTIFY`), sessions, and tracked Discord messages. All
//! durable state lives here so a restart loses nothing.
//!
//! Query functions are plain functions over `&mut PgConnection`, so a
//! caller composes several in one transaction ([`Tx`] derefs to the
//! connection). Writes that change a state are compare-and-set on the state
//! the caller expects and report whether they applied. Rows come back as
//! quack-core types; no state leaves this crate as a string. Every query
//! runs in a `{operation} {table}` client span and records its duration.

mod db;
mod error;
mod fence;
mod migrate;
mod obs;
mod pg;

pub mod accounts;
pub mod actions;
pub mod activity_log;
pub mod appeals;
pub mod cases;
pub mod dms;
pub mod drafts;
pub mod evidence;
pub mod guilds;
pub mod honeypot;
pub mod housekeeping;
pub mod import;
pub mod jobs;
pub mod messages;
pub mod modlog;
pub mod notify;
pub mod operation_keys;
pub mod rules;
pub mod sessions;
pub mod settings;
pub mod stats;
pub mod tickets;

pub use db::{
    ACQUIRE_TIMEOUT, CONTENTION_RETRIES, Db, IDLE_IN_TRANSACTION_TIMEOUT, STATEMENT_TIMEOUT, Tx,
};
pub use error::{Applied, Conflict, DbError, Violation};
pub use fence::{FENCE_CHECK_INTERVAL, FENCE_CHECK_TIMEOUT, FENCE_KEY, Fence};
pub use migrate::{MIGRATOR, MigrationInfo, MigrationMismatch, migrate, verify_migrations};
pub use pg::{Page, TimeCursor};
pub use sqlx::PgConnection;
