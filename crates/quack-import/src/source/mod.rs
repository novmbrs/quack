//! The Go MySQL database: one connection in one consistent snapshot, its
//! shape (§19.1), keyed batch reads, and typed column access.

pub mod digest;
pub mod manifest;

use std::str::FromStr;

use sqlx::mysql::{MySqlConnectOptions, MySqlRow};
use sqlx::{ConnectOptions as _, Connection as _, MySqlConnection, Row as _};
use time::{OffsetDateTime, PrimitiveDateTime};

use crate::ImportError;
use crate::convert;

/// Rows per keyed batch read.
pub(crate) const BATCH: usize = 500;

/// The Go database, read inside one `REPEATABLE READ` snapshot.
pub(crate) struct Source {
    conn: MySqlConnection,
    /// What the source looks like.
    pub(crate) shape: manifest::Shape,
}

impl Source {
    /// Connects, pins the session to UTC, and opens the snapshot. The URL
    /// is never logged.
    pub(crate) async fn open(url: &str) -> Result<Self, ImportError> {
        let options = MySqlConnectOptions::from_str(url)
            .map_err(ImportError::source)?
            .disable_statement_logging()
            .timezone(Some("+00:00".to_owned()));
        let mut conn = MySqlConnection::connect_with(&options)
            .await
            .map_err(ImportError::source)?;
        for statement in [
            "SET SESSION TRANSACTION ISOLATION LEVEL REPEATABLE READ",
            "SET SESSION time_zone = '+00:00'",
            "SET SESSION group_concat_max_len = 1048576",
            "START TRANSACTION WITH CONSISTENT SNAPSHOT, READ ONLY",
        ] {
            sqlx::raw_sql(statement)
                .execute(&mut conn)
                .await
                .map_err(ImportError::source)?;
        }
        let shape = manifest::inspect(&mut conn).await?;
        Ok(Self { conn, shape })
    }

    /// Ends the snapshot.
    pub(crate) async fn close(mut self) -> Result<(), ImportError> {
        sqlx::raw_sql("ROLLBACK")
            .execute(&mut self.conn)
            .await
            .map_err(ImportError::source)?;
        self.conn.close().await.map_err(ImportError::source)
    }

    /// The connection, for one-off reads.
    pub(crate) fn conn(&mut self) -> &mut MySqlConnection {
        &mut self.conn
    }

    /// Runs `sql` with string parameters.
    pub(crate) async fn rows(
        &mut self,
        sql: &str,
        binds: &[&str],
    ) -> Result<Vec<MySqlRow>, ImportError> {
        // Every SQL string the importer builds is a constant with spliced
        // manifest names or placeholder lists; values are always bound.
        let mut query = sqlx::query(sqlx::AssertSqlSafe(sql.to_owned()));
        for bind in binds {
            query = query.bind(*bind);
        }
        query
            .fetch_all(&mut self.conn)
            .await
            .map_err(ImportError::source)
    }

    /// One keyed batch: `sql` must end with `… AND <key> > ? ORDER BY <key>`;
    /// `binds` come first, then `after`, and `LIMIT` is appended.
    pub(crate) async fn page(
        &mut self,
        sql: &str,
        binds: &[&str],
        after: &str,
    ) -> Result<Vec<MySqlRow>, ImportError> {
        let sql = format!("{sql} LIMIT {BATCH}");
        let mut all: Vec<&str> = binds.to_vec();
        all.push(after);
        self.rows(&sql, &all).await
    }

    /// `SELECT NOW(3)` and `VERSION()` inside the snapshot.
    pub(crate) async fn clock(&mut self) -> Result<(OffsetDateTime, String), ImportError> {
        let row = sqlx::query("SELECT NOW(3) AS now, VERSION() AS version")
            .fetch_one(&mut self.conn)
            .await
            .map_err(ImportError::source)?;
        let now: PrimitiveDateTime = row.try_get("now").map_err(ImportError::source)?;
        let version: String = row.try_get("version").map_err(ImportError::source)?;
        Ok((convert::utc(now), version))
    }
}

/// A keyed walk over a query in [`Source::page`] batches, each batch
/// starting after the last row's `key` column.
pub(crate) struct Pages<'a> {
    sql: &'a str,
    table: &'static str,
    key: &'static str,
    after: String,
    done: bool,
}

impl<'a> Pages<'a> {
    /// A walk from the first row; `sql` is as [`Source::page`] takes it.
    pub(crate) const fn new(sql: &'a str, table: &'static str, key: &'static str) -> Self {
        Self {
            sql,
            table,
            key,
            after: String::new(),
            done: false,
        }
    }

    /// The next batch, or `None` after the last.
    pub(crate) async fn next(
        &mut self,
        src: &mut Source,
        binds: &[&str],
    ) -> Result<Option<Vec<MySqlRow>>, ImportError> {
        if self.done {
            return Ok(None);
        }
        let rows = src.page(self.sql, binds, &self.after).await?;
        let Some(last) = rows.last() else {
            return Ok(None);
        };
        Cols::new(last, self.table, self.key)
            .key()
            .clone_into(&mut self.after);
        self.done = rows.len() < BATCH;
        Ok(Some(rows))
    }
}

/// Typed access to one source row, with errors naming table, column, and key.
pub(crate) struct Cols<'r> {
    row: &'r MySqlRow,
    table: &'static str,
    key: String,
}

impl<'r> Cols<'r> {
    /// Wraps `row` of `table`, keyed by the `key_col` column.
    pub(crate) fn new(row: &'r MySqlRow, table: &'static str, key_col: &str) -> Self {
        let key = row
            .try_get::<Option<String>, _>(key_col)
            .ok()
            .flatten()
            .unwrap_or_default();
        Self { row, table, key }
    }

    /// The row's key.
    pub(crate) fn key(&self) -> &str {
        &self.key
    }

    fn err(&self, column: &str, detail: impl std::fmt::Display) -> ImportError {
        ImportError::Decode {
            table: self.table,
            column: column.to_owned(),
            key: self.key.clone(),
            detail: detail.to_string(),
        }
    }

    fn get<T>(&self, column: &str) -> Result<T, ImportError>
    where
        T: for<'a> sqlx::Decode<'a, sqlx::MySql> + sqlx::Type<sqlx::MySql>,
    {
        self.row.try_get(column).map_err(|e| self.err(column, e))
    }

    /// A NOT NULL text column.
    pub(crate) fn text(&self, column: &str) -> Result<String, ImportError> {
        Ok(self.opt_text(column)?.unwrap_or_default())
    }

    /// A nullable text column, as stored (`''` kept).
    pub(crate) fn opt_text(&self, column: &str) -> Result<Option<String>, ImportError> {
        self.get(column)
    }

    /// A text column with `''` → `None`.
    pub(crate) fn nonempty(&self, column: &str) -> Result<Option<String>, ImportError> {
        Ok(convert::opt_text(self.opt_text(column)?))
    }

    /// A ULID column mapped to its UUID; `None` when NULL, empty, or not a ULID.
    pub(crate) fn uuid(&self, column: &str) -> Result<Option<uuid::Uuid>, ImportError> {
        Ok(self.nonempty(column)?.and_then(|s| convert::uuid(&s)))
    }

    /// A snowflake column.
    pub(crate) fn snowflake(&self, column: &str) -> Result<convert::Snowflake, ImportError> {
        Ok(convert::snowflake(self.opt_text(column)?.as_deref()))
    }

    /// A NOT NULL `datetime(3)`, as UTC.
    pub(crate) fn time(&self, column: &str) -> Result<OffsetDateTime, ImportError> {
        self.get::<PrimitiveDateTime>(column).map(convert::utc)
    }

    /// A nullable `datetime(3)`, as UTC.
    pub(crate) fn opt_time(&self, column: &str) -> Result<Option<OffsetDateTime>, ImportError> {
        Ok(self
            .get::<Option<PrimitiveDateTime>>(column)?
            .map(convert::utc))
    }

    /// A raw `datetime(3)` (for message times).
    pub(crate) fn naive(&self, column: &str) -> Result<Option<PrimitiveDateTime>, ImportError> {
        self.get(column)
    }

    /// A `tinyint(1)`, which must be 0 or 1.
    pub(crate) fn flag(&self, column: &str) -> Result<bool, ImportError> {
        let raw: i8 = self.get(column)?;
        convert::flag(raw).ok_or_else(|| self.err(column, format!("boolean {raw} is not 0 or 1")))
    }

    /// An unsigned integer column.
    pub(crate) fn unsigned(&self, column: &str) -> Result<u64, ImportError> {
        self.get(column)
    }

    /// A signed integer column.
    pub(crate) fn int(&self, column: &str) -> Result<i64, ImportError> {
        self.get(column)
    }

    /// A `json` column.
    pub(crate) fn json(&self, column: &str) -> Result<serde_json::Value, ImportError> {
        Ok(self
            .get::<Option<sqlx::types::Json<serde_json::Value>>>(column)?
            .map_or(serde_json::Value::Null, |j| j.0))
    }

    /// JSON stored in a text column; `None` when NULL or empty.
    pub(crate) fn json_text(&self, column: &str) -> Result<Option<serde_json::Value>, ImportError> {
        match self.nonempty(column)? {
            None => Ok(None),
            Some(text) => serde_json::from_str(&text)
                .map(Some)
                .map_err(|e| self.err(column, format!("invalid JSON: {e}"))),
        }
    }
}
