//! Writing PostgreSQL: plain inserts (never `ON CONFLICT DO NOTHING`,
//! §19.11), the quarantine for history rows a constraint refuses, and the
//! shared tracked-message, DM, and job rows several tables produce.

use quack_core::ids::{DmId, GuildId, MessageRowId};
use quack_db::Tx;
use quack_db::jobs::{self, NewJob};
use sqlx::Postgres;
use sqlx::postgres::PgArguments;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::ImportError;
use crate::error::describe;
use crate::report::Fragment;

/// A runtime-checked PostgreSQL statement.
pub(crate) type Query<'q> = sqlx::query::Query<'q, Postgres, PgArguments>;

/// Inserts a row whose refusal must stop the server's import: cases, rules,
/// actions, appeals, settings. Any error names the table and source key.
pub(crate) async fn strict(
    tx: &mut Tx,
    f: &mut Fragment,
    table: &'static str,
    key: &str,
    query: Query<'_>,
) -> Result<(), ImportError> {
    query
        .execute(&mut **tx)
        .await
        .map_err(|e| ImportError::Refused {
            table,
            key: key.to_owned(),
            detail: describe(&e),
        })?;
    f.wrote(table);
    Ok(())
}

/// Inserts a history row (an attempt, evidence, an event, a log entry …)
/// inside a savepoint. A `CHECK`, foreign key, or `NOT NULL` refusal rolls
/// back just this row and quarantines it; a unique-constraint collision
/// still stops the server (§19.11). Returns whether the row was written.
pub(crate) async fn guarded(
    tx: &mut Tx,
    f: &mut Fragment,
    table: &'static str,
    key: &str,
    query: Query<'_>,
) -> Result<bool, ImportError> {
    exec(tx, "SAVEPOINT import_row").await?;
    match query.execute(&mut **tx).await {
        Ok(_) => {
            exec(tx, "RELEASE SAVEPOINT import_row").await?;
            f.wrote(table);
            Ok(true)
        }
        Err(sqlx::Error::Database(db))
            if db
                .code()
                .is_some_and(|c| c.starts_with("23") && c != "23505") =>
        {
            exec(tx, "ROLLBACK TO SAVEPOINT import_row").await?;
            let constraint = db.constraint().unwrap_or("unnamed").to_owned();
            f.quarantine(table, &constraint, key);
            Ok(false)
        }
        Err(e) => Err(ImportError::Refused {
            table,
            key: key.to_owned(),
            detail: describe(&e),
        }),
    }
}

/// Runs a statement without results.
pub(crate) async fn exec(tx: &mut Tx, sql: &'static str) -> Result<(), ImportError> {
    sqlx::query(sql)
        .execute(&mut **tx)
        .await
        .map_err(ImportError::target)?;
    Ok(())
}

fn guild_id(id: i64) -> Result<GuildId, ImportError> {
    GuildId::try_from(id)
        .map_err(|_| ImportError::server(format!("server ID {id} is not a snowflake")))
}

/// A tracked Discord message row (§10).
#[derive(Debug)]
pub(crate) struct NewMessage {
    pub id: Uuid,
    pub kind: &'static str,
    pub case_id: Option<Uuid>,
    pub appeal_id: Option<Uuid>,
    pub ticket_id: Option<Uuid>,
    pub channel_id: Option<i64>,
    pub message_id: Option<i64>,
    pub wanted: i32,
    pub posted: i32,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

/// Inserts a tracked message; one that is behind (`wanted > posted`) and
/// has a channel gets its `sync_message` job.
pub(crate) async fn message(
    tx: &mut Tx,
    f: &mut Fragment,
    guild: i64,
    key: &str,
    m: &NewMessage,
) -> Result<bool, ImportError> {
    let query = sqlx::query(
        "INSERT INTO messages (id, guild_id, kind, case_id, appeal_id, ticket_id, channel_id, message_id,
                               wanted_revision, posted_revision, wanted_at, created_at, updated_at)
         VALUES ($1, $2, $3::message_kind, $4, $5, $6, $7, $8, $9, $10, $11, $11, $12)",
    )
    .bind(m.id)
    .bind(guild)
    .bind(m.kind)
    .bind(m.case_id)
    .bind(m.appeal_id)
    .bind(m.ticket_id)
    .bind(m.channel_id)
    .bind(m.message_id)
    .bind(m.wanted)
    .bind(m.posted)
    .bind(m.created_at)
    .bind(m.updated_at);
    if !guarded(tx, f, "messages", key, query).await? {
        return Ok(false);
    }
    if m.wanted > m.posted && m.channel_id.is_some() {
        let job = NewJob::sync_message(guild_id(guild)?, MessageRowId::from_uuid(m.id));
        jobs::enqueue(tx, &job).await?;
        f.wrote("jobs");
    }
    Ok(true)
}

/// One DM row (§6).
#[derive(Debug)]
pub(crate) struct NewDm {
    pub id: Uuid,
    pub member_id: i64,
    pub kind: &'static str,
    pub case_id: Option<Uuid>,
    pub appeal_event_id: Option<Uuid>,
    pub ticket_id: Option<Uuid>,
    pub state: DmState,
    pub created_at: OffsetDateTime,
}

/// A DM's state with the columns each state requires.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DmState {
    /// Not sent yet; a `send_dm` job sends it after cutover.
    Pending,
    /// Sent: the DM channel (when Go kept it) and message.
    Sent {
        channel: Option<i64>,
        message: i64,
        at: OffsetDateTime,
    },
    /// Couldn't confirm; never sent again.
    Unknown { at: OffsetDateTime },
    /// Discord refused it.
    Failed {
        failure: &'static str,
        at: OffsetDateTime,
    },
    /// No DM was needed (the case was voided first).
    NotNeeded { at: OffsetDateTime },
}

impl DmState {
    /// The `dm_state` label.
    pub(crate) const fn label(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Sent { .. } => "sent",
            Self::Unknown { .. } => "unknown",
            Self::Failed { .. } => "failed",
            Self::NotNeeded { .. } => "not_needed",
        }
    }
}

/// Inserts a DM; a pending one gets its `send_dm` job.
pub(crate) async fn dm(
    tx: &mut Tx,
    f: &mut Fragment,
    guild: i64,
    key: &str,
    d: &NewDm,
) -> Result<bool, ImportError> {
    let (failure, channel, message, first, settled) = match &d.state {
        DmState::Pending => (None, None, None, None, None),
        DmState::Sent {
            channel,
            message,
            at,
        } => (None, *channel, Some(*message), Some(*at), Some(*at)),
        DmState::Unknown { at } => (None, None, None, Some(*at), Some(*at)),
        DmState::Failed { failure, at } => (Some(*failure), None, None, None, Some(*at)),
        DmState::NotNeeded { at } => (None, None, None, None, Some(*at)),
    };
    let query = sqlx::query(
        "INSERT INTO dms (id, guild_id, member_id, kind, case_id, appeal_event_id, ticket_id, state, failure,
                          channel_id, message_id, first_attempt_at, settled_at, created_at)
         VALUES ($1, $2, $3, $4::dm_kind, $5, $6, $7, $8::dm_state, $9::dm_failure, $10, $11, $12, $13, $14)",
    )
    .bind(d.id)
    .bind(guild)
    .bind(d.member_id)
    .bind(d.kind)
    .bind(d.case_id)
    .bind(d.appeal_event_id)
    .bind(d.ticket_id)
    .bind(d.state.label())
    .bind(failure)
    .bind(channel)
    .bind(message)
    .bind(first)
    .bind(settled)
    .bind(d.created_at);
    if !guarded(tx, f, "dms", key, query).await? {
        return Ok(false);
    }
    if d.state == DmState::Pending {
        jobs::enqueue(
            tx,
            &NewJob::send_dm(guild_id(guild)?, DmId::from_uuid(d.id)),
        )
        .await?;
        f.wrote("jobs");
    }
    Ok(true)
}

/// Enqueues a job in the server's transaction.
pub(crate) async fn job(tx: &mut Tx, f: &mut Fragment, job: NewJob) -> Result<(), ImportError> {
    jobs::enqueue(tx, &job).await?;
    f.wrote("jobs");
    Ok(())
}

/// The server's `GuildId`.
pub(crate) fn guild(id: i64) -> Result<GuildId, ImportError> {
    guild_id(id)
}
