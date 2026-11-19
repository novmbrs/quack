//! The MySQL importer's bookkeeping (§19): runs, per-server completion, and
//! the case counter after imported cases.

use quack_core::ids::GuildId;
use sqlx::PgConnection;
use time::OffsetDateTime;

use crate::DbError;
use crate::obs::{self, Insert, Select, Update};
use crate::pg::int;

/// How the importer runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportMode {
    /// Validate only; the transaction is rolled back.
    DryRun,
    /// Write.
    Apply,
}

impl ImportMode {
    const fn as_str(self) -> &'static str {
        match self {
            Self::DryRun => "dry_run",
            Self::Apply => "apply",
        }
    }
}

/// Records the start of a run and returns its ID.
pub async fn start_run(
    conn: &mut PgConnection,
    mode: ImportMode,
    source_fingerprint: &str,
    transform_version: i32,
    evaluated_at: OffsetDateTime,
) -> Result<uuid::Uuid, DbError> {
    let id = uuid::Uuid::now_v7();
    obs::run(
        Insert,
        "import_runs",
        sqlx::query!(
            "INSERT INTO import_runs (id, mode, source_fingerprint, transform_version, evaluated_at)
             VALUES ($1, $2, $3, $4, $5)",
            id,
            mode.as_str(),
            source_fingerprint,
            transform_version,
            evaluated_at,
        )
        .execute(conn),
    )
    .await?;
    Ok(id)
}

/// Records the end of a run with its validation report, once;
/// [`DbError::Stale`] when the run is unknown or already finished.
pub async fn finish_run(
    conn: &mut PgConnection,
    run: uuid::Uuid,
    report: &serde_json::Value,
) -> Result<(), DbError> {
    let done = obs::run(
        Update,
        "import_runs",
        sqlx::query!(
            "UPDATE import_runs SET finished_at = now(), report = $2 WHERE id = $1 AND finished_at IS NULL",
            run,
            report
        )
        .execute(conn),
    )
    .await?;
    if done.rows_affected() == 1 {
        Ok(())
    } else {
        Err(DbError::Stale)
    }
}

/// The source fingerprint a server was imported from, if it was.
pub async fn imported_from(
    conn: &mut PgConnection,
    guild: GuildId,
) -> Result<Option<String>, DbError> {
    obs::run(
        Select,
        "import_guilds",
        sqlx::query_scalar!(
            "SELECT source_fingerprint FROM import_guilds WHERE guild_id = $1",
            i64::from(guild)
        )
        .fetch_optional(conn),
    )
    .await
}

/// Marks a server as imported by `run`.
pub async fn mark_imported(
    conn: &mut PgConnection,
    guild: GuildId,
    run: uuid::Uuid,
    source_fingerprint: &str,
) -> Result<(), DbError> {
    obs::run(
        Insert,
        "import_guilds",
        sqlx::query!(
            "INSERT INTO import_guilds (guild_id, run_id, source_fingerprint) VALUES ($1, $2, $3)",
            i64::from(guild),
            run,
            source_fingerprint
        )
        .execute(conn),
    )
    .await?;
    Ok(())
}

/// Raises the server's case counter to at least `last_number` (the highest
/// imported case number).
pub async fn raise_case_counter(
    conn: &mut PgConnection,
    guild: GuildId,
    last_number: u64,
) -> Result<(), DbError> {
    obs::run(
        Insert,
        "case_counters",
        sqlx::query!(
            "INSERT INTO case_counters (guild_id, last_number) VALUES ($1, $2)
             ON CONFLICT (guild_id) DO UPDATE SET last_number = GREATEST(case_counters.last_number, EXCLUDED.last_number)",
            i64::from(guild),
            int::<u64, i64>(last_number)?,
        )
        .execute(conn),
    )
    .await?;
    Ok(())
}
