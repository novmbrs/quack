//! The importer's errors.

use quack_db::DbError;

/// Why an import (or one server's part of it) stopped.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    /// Reading MySQL failed. The message never includes the source URL.
    #[error("reading the MySQL source failed: {0}")]
    Source(String),
    /// A quack-db call failed.
    #[error("PostgreSQL: {0}")]
    Db(#[from] DbError),
    /// A PostgreSQL statement of the importer failed.
    #[error("writing PostgreSQL failed: {0}")]
    Target(String),
    /// The source does not match the manifest; nothing was read.
    #[error("the MySQL source has an unknown shape: {}", .0.join("; "))]
    Shape(Vec<String>),
    /// Another Quack process holds the singleton fence.
    #[error("another Quack process is running against this database; stop it first")]
    Fenced,
    /// A server was already imported from a different source snapshot.
    #[error(
        "server {guild} was already imported from a different source (fingerprint {existing}); \
         drop and re-create the PostgreSQL database, run `quack migrate`, and import again"
    )]
    ChangedSource {
        /// The server.
        guild: i64,
        /// The fingerprint it was imported from.
        existing: String,
    },
    /// A source value could not be read as its column's type.
    #[error("{table}.{column} of {key}: {detail}")]
    Decode {
        /// Source table.
        table: &'static str,
        /// Source column.
        column: String,
        /// The row's key.
        key: String,
        /// What was wrong.
        detail: String,
    },
    /// A server cannot be imported as it is; an operator must decide.
    #[error("{}", .0.join("; "))]
    Server(Vec<String>),
    /// A row failed a constraint that must not be skipped (§19.11).
    #[error("{table} row {key} was refused by PostgreSQL: {detail}")]
    Refused {
        /// Target table.
        table: &'static str,
        /// The source key.
        key: String,
        /// The database's message.
        detail: String,
    },
    /// Writing the report file failed.
    #[error("writing the report failed: {0}")]
    Report(#[from] std::io::Error),
}

impl ImportError {
    /// A MySQL driver error, with any connection details removed.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "used as `map_err(ImportError::source)`"
    )]
    pub(crate) fn source(error: sqlx::Error) -> Self {
        let message = match &error {
            sqlx::Error::Configuration(_) => "the source URL is not a valid MySQL URL".to_owned(),
            other => other.to_string(),
        };
        Self::Source(message)
    }

    /// A PostgreSQL driver error: its code, constraint, and message (no
    /// row values; `detail` is left out).
    #[expect(
        clippy::needless_pass_by_value,
        reason = "used as `map_err(ImportError::target)`"
    )]
    pub(crate) fn target(error: sqlx::Error) -> Self {
        Self::Target(describe(&error))
    }

    /// One server-level problem.
    pub(crate) fn server(problem: impl Into<String>) -> Self {
        Self::Server(vec![problem.into()])
    }
}

/// A short description of a PostgreSQL error without row values.
pub(crate) fn describe(error: &sqlx::Error) -> String {
    match error {
        sqlx::Error::Database(db) => {
            let code = db.code().unwrap_or_default();
            match db.constraint() {
                Some(constraint) => format!("{} [{code}, {constraint}]", db.message()),
                None => format!("{} [{code}]", db.message()),
            }
        }
        other => other.to_string(),
    }
}
