//! Periodic deletes of expired rows (§16.6), in batches of 1,000.

use std::time::{Duration, Instant};

use sqlx::PgConnection;

use crate::DbError;
use crate::obs::{self, Delete, Update};

/// Rows per delete statement.
pub const BATCH: i64 = 1000;

/// What one or more housekeeping passes removed or changed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Expired sessions, or sessions under another session secret.
    pub sessions: u64,
    /// Expired OAuth login states.
    pub oauth_states: u64,
    /// Expired operation keys.
    pub operation_keys: u64,
    /// Expired `/case add` drafts.
    pub drafts: u64,
    /// Dead jobs older than 7 days.
    pub dead_jobs: u64,
    /// Expired ticket transcripts.
    pub transcripts: u64,
    /// Expired journal rows, and rows over a long-open ticket's cap.
    pub journal: u64,
    /// Honeypot hits older than 30 days.
    pub honeypot_hits: u64,
    /// Private receipts whose token expired (now `gone`) and expired
    /// tokens cleared.
    pub receipt_tokens: u64,
    /// Some batch was full: rows remain, run again at once.
    pub more: bool,
}

/// Runs passes until nothing is left or `budget` is spent (60 s per run).
/// `key_fingerprint` is the current session secret's.
pub async fn run(
    conn: &mut PgConnection,
    key_fingerprint: &[u8; 8],
    budget: Duration,
) -> Result<Report, DbError> {
    let started = Instant::now();
    let mut total = Report::default();
    loop {
        let pass = pass(conn, key_fingerprint).await?;
        total.sessions += pass.sessions;
        total.oauth_states += pass.oauth_states;
        total.operation_keys += pass.operation_keys;
        total.drafts += pass.drafts;
        total.dead_jobs += pass.dead_jobs;
        total.transcripts += pass.transcripts;
        total.journal += pass.journal;
        total.honeypot_hits += pass.honeypot_hits;
        total.receipt_tokens += pass.receipt_tokens;
        total.more = pass.more;
        if !pass.more || started.elapsed() >= budget {
            return Ok(total);
        }
    }
}

/// One batch of every delete.
pub async fn pass(conn: &mut PgConnection, key_fingerprint: &[u8; 8]) -> Result<Report, DbError> {
    let auth = expired_sign_ins(conn, key_fingerprint).await?;
    let mut r = Report {
        sessions: auth.sessions,
        oauth_states: auth.oauth_states,
        operation_keys: auth.operation_keys,
        drafts: auth.drafts,
        ..expired_work(conn).await?
    };
    r.journal += trim_long_journals(conn).await?;
    r.receipt_tokens = expire_receipt_tokens(conn).await?;
    r.more = [
        r.sessions,
        r.oauth_states,
        r.operation_keys,
        r.drafts,
        r.dead_jobs,
        r.transcripts,
        r.honeypot_hits,
        r.receipt_tokens,
    ]
    .iter()
    .any(|n| *n >= BATCH.unsigned_abs())
        || r.journal >= BATCH.unsigned_abs();
    Ok(r)
}

/// Journal rows of tickets open for more than 90 days beyond their newest
/// 50,000 are deleted, marking `journal_gaps` (§13.2.5).
async fn trim_long_journals(conn: &mut PgConnection) -> Result<u64, DbError> {
    let tickets = obs::run(
        Delete,
        "ticket_messages",
        sqlx::query!(
            r#"WITH long AS (
               SELECT t.id, (SELECT m.message_id FROM ticket_messages m WHERE m.ticket_id = t.id
                             ORDER BY m.message_id DESC OFFSET 49999 LIMIT 1) AS keep_from
               FROM tickets t
               WHERE t.state IN ('opening', 'open', 'closing') AND t.opened_at < now() - interval '90 days'
             ),
             gone AS (
               DELETE FROM ticket_messages WHERE ctid IN (
                 SELECT m.ctid FROM ticket_messages m JOIN long l ON l.id = m.ticket_id
                 WHERE l.keep_from IS NOT NULL AND m.message_id < l.keep_from LIMIT $1)
               RETURNING guild_id, ticket_id
             )
             SELECT guild_id AS "guild_id!", ticket_id AS "ticket_id!" FROM gone"#,
            BATCH
        )
        .fetch_all(&mut *conn),
    )
    .await?;
    if tickets.is_empty() {
        return Ok(0);
    }
    let removed = u64::try_from(tickets.len()).unwrap_or(u64::MAX);
    let guilds: Vec<i64> = tickets.iter().map(|t| t.guild_id).collect();
    let ids: Vec<uuid::Uuid> = tickets.iter().map(|t| t.ticket_id).collect();
    obs::run(
        Update,
        "tickets",
        sqlx::query!(
            "UPDATE tickets t SET journal_gaps = true, updated_at = now()
             FROM unnest($1::bigint[], $2::uuid[]) AS d(guild_id, id)
             WHERE t.guild_id = d.guild_id AND t.id = d.id AND NOT t.journal_gaps",
            &guilds,
            &ids
        )
        .execute(conn),
    )
    .await?;
    Ok(removed)
}

/// Private receipts whose token expired become `gone` (they can never be
/// written again); other expired tokens are cleared.
async fn expire_receipt_tokens(conn: &mut PgConnection) -> Result<u64, DbError> {
    let gone = obs::run(
        Update,
        "messages",
        sqlx::query!(
            "UPDATE messages SET state = 'gone', interaction_token = NULL, token_expires_at = NULL, updated_at = now()
             WHERE ctid IN (SELECT ctid FROM messages WHERE private AND token_expires_at < now() LIMIT $1)",
            BATCH
        )
        .execute(&mut *conn),
    )
    .await?
    .rows_affected();
    let cleared = obs::run(
        Update,
        "messages",
        sqlx::query!(
            "UPDATE messages SET interaction_token = NULL, token_expires_at = NULL, updated_at = now()
             WHERE ctid IN (SELECT ctid FROM messages WHERE NOT private AND token_expires_at < now() LIMIT $1)",
            BATCH
        )
        .execute(conn),
    )
    .await?
    .rows_affected();
    Ok(gone + cleared)
}

/// Sessions, login states, operation keys, and drafts past their expiry.
async fn expired_sign_ins(
    conn: &mut PgConnection,
    key_fingerprint: &[u8; 8],
) -> Result<Report, DbError> {
    Ok(Report {
        sessions: obs::run(
            Delete,
            "sessions",
            sqlx::query!(
                "DELETE FROM sessions WHERE ctid IN (SELECT ctid FROM sessions
                   WHERE expires_at < now() OR key_fingerprint <> $1 LIMIT $2)",
                &key_fingerprint[..],
                BATCH
            )
            .execute(&mut *conn),
        )
        .await?
        .rows_affected(),
        oauth_states: obs::run(
            Delete,
            "oauth_states",
            sqlx::query!(
                "DELETE FROM oauth_states WHERE ctid IN (SELECT ctid FROM oauth_states WHERE expires_at < now() LIMIT $1)",
                BATCH
            )
            .execute(&mut *conn),
        )
        .await?
        .rows_affected(),
        operation_keys: obs::run(
            Delete,
            "operation_keys",
            sqlx::query!(
                "DELETE FROM operation_keys WHERE ctid IN
                   (SELECT ctid FROM operation_keys WHERE expires_at < now() LIMIT $1)",
                BATCH
            )
            .execute(&mut *conn),
        )
        .await?
        .rows_affected(),
        drafts: obs::run(
            Delete,
            "case_drafts",
            sqlx::query!(
                "DELETE FROM case_drafts WHERE ctid IN (SELECT ctid FROM case_drafts WHERE expires_at < now() LIMIT $1)",
                BATCH
            )
            .execute(&mut *conn),
        )
        .await?
        .rows_affected(),
        ..Report::default()
    })
}

/// Dead jobs, transcripts, journal rows, and honeypot hits past their
/// retention.
async fn expired_work(conn: &mut PgConnection) -> Result<Report, DbError> {
    Ok(Report {
        dead_jobs: obs::run(
            Delete,
            "jobs",
            sqlx::query!(
                "DELETE FROM jobs WHERE ctid IN
                   (SELECT ctid FROM jobs WHERE dead_at < now() - interval '7 days' LIMIT $1)",
                BATCH
            )
            .execute(&mut *conn),
        )
        .await?
        .rows_affected(),
        transcripts: obs::run(
            Delete,
            "ticket_transcripts",
            sqlx::query!(
                "DELETE FROM ticket_transcripts WHERE ctid IN
                   (SELECT ctid FROM ticket_transcripts WHERE expires_at < now() LIMIT $1)",
                BATCH
            )
            .execute(&mut *conn),
        )
        .await?
        .rows_affected(),
        journal: obs::run(
            Delete,
            "ticket_messages",
            sqlx::query!(
                "DELETE FROM ticket_messages WHERE ctid IN
                   (SELECT ctid FROM ticket_messages WHERE expires_at < now() LIMIT $1)",
                BATCH
            )
            .execute(&mut *conn),
        )
        .await?
        .rows_affected(),
        honeypot_hits: obs::run(
            Delete,
            "honeypot_hits",
            sqlx::query!(
                "DELETE FROM honeypot_hits WHERE ctid IN
                   (SELECT ctid FROM honeypot_hits WHERE created_at < now() - interval '30 days' LIMIT $1)",
                BATCH
            )
            .execute(&mut *conn),
        )
        .await?
        .rows_affected(),
        ..Report::default()
    })
}
