//! The ticket journal (original text of every message before the thread
//! froze) and saved transcripts.

use quack_core::ids::{AccountId, GuildId, MessageId, TicketId};
use quack_core::modules::TranscriptPartial;
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use sqlx::types::Json;
use time::OffsetDateTime;

use crate::DbError;
use crate::obs::{self, Insert, Select, Update};
use crate::pg::{Pg, id};

/// A file listed on a journal message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalFile {
    /// The file name.
    pub name: String,
    /// Its size in bytes.
    pub size: u64,
}

/// One journal message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalMessage {
    /// The message.
    pub message: MessageId,
    /// Its author.
    pub author: AccountId,
    /// The author's name when it was sent.
    pub author_name: String,
    /// Its original text.
    pub text: String,
    /// Its files.
    pub files: Vec<JournalFile>,
    /// When it was sent.
    pub sent_at: OffsetDateTime,
}

/// Journals a message of a live, not yet frozen ticket (`ON CONFLICT DO
/// NOTHING`). Returns whether it was stored.
///
/// The ticket row is read `FOR SHARE`: a concurrent freeze either waits
/// for this insert to commit (so the transcript sees it) or wins, and then
/// the recheck finds the ticket frozen and nothing is stored.
pub async fn add_journal_message(
    conn: &mut PgConnection,
    guild: GuildId,
    ticket: TicketId,
    message: &JournalMessage,
) -> Result<bool, DbError> {
    let done = obs::run(
        Insert,
        "ticket_messages",
        sqlx::query!(
            "INSERT INTO ticket_messages (guild_id, ticket_id, message_id, author_id, author_name, text, files, sent_at)
             SELECT $1, $2, $3, $4, $5, $6, $7, $8 FROM tickets
             WHERE guild_id = $1 AND id = $2 AND state IN ('opening', 'open', 'closing') AND frozen_at IS NULL
             FOR SHARE
             ON CONFLICT DO NOTHING",
            i64::from(guild),
            ticket.get(),
            i64::from(message.message),
            i64::from(message.author),
            message.author_name,
            message.text,
            Json(&message.files) as _,
            message.sent_at,
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Marks that the journal may be missing messages.
pub async fn mark_journal_gaps(
    conn: &mut PgConnection,
    guild: GuildId,
    ticket: TicketId,
) -> Result<bool, DbError> {
    let done = obs::run(
        Update,
        "tickets",
        sqlx::query!(
            "UPDATE tickets SET journal_gaps = true, updated_at = now()
             WHERE guild_id = $1 AND id = $2 AND NOT journal_gaps",
            i64::from(guild),
            ticket.get(),
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// The most journal rows one [`journal`] call returns.
pub const JOURNAL_PAGE: u32 = 1000;

/// One page of a ticket's journal, oldest first: the messages after
/// `after` (by message ID), at most `limit` (capped at [`JOURNAL_PAGE`]).
/// A transcript reads it page by page and stops at its size cap, so a
/// long journal is never loaded whole.
pub async fn journal(
    conn: &mut PgConnection,
    guild: GuildId,
    ticket: TicketId,
    after: Option<MessageId>,
    limit: u32,
) -> Result<Vec<JournalMessage>, DbError> {
    let rows = obs::run(
        Select,
        "ticket_messages",
        sqlx::query!(
            r#"SELECT message_id, author_id, author_name, text, files AS "files: Json<Vec<JournalFile>>", sent_at
               FROM ticket_messages
               WHERE guild_id = $1 AND ticket_id = $2 AND message_id > $3
               ORDER BY message_id LIMIT $4"#,
            i64::from(guild),
            ticket.get(),
            after.map_or(0, i64::from),
            i64::from(limit.min(JOURNAL_PAGE)),
        )
        .fetch_all(conn),
    )
    .await?;
    rows.into_iter()
        .map(|r| {
            Ok(JournalMessage {
                message: id(r.message_id)?,
                author: id(r.author_id)?,
                author_name: r.author_name,
                text: r.text,
                files: r.files.0,
                sent_at: r.sent_at,
            })
        })
        .collect()
}

/// A saved transcript.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transcript {
    /// The rendered text (at most 8 MiB).
    pub content: String,
    /// Why it is partial; `None` when complete.
    pub partial: Option<TranscriptPartial>,
    /// When it was captured.
    pub captured_at: OffsetDateTime,
    /// When it is deleted.
    pub expires_at: OffsetDateTime,
}

/// Saves a ticket's transcript once (close step 2) and records
/// `transcript_saved_at`, both in the caller's transaction. Only a frozen,
/// `closing` ticket whose transcript was never saved gets one, so a
/// transcript deleted by retention is never recreated. The ticket row is
/// locked first (parent before child). Returns whether it saved.
pub async fn save_transcript(
    conn: &mut PgConnection,
    guild: GuildId,
    ticket: TicketId,
    content: &str,
    partial: Option<TranscriptPartial>,
    retention_days: u16,
) -> Result<bool, DbError> {
    let marked = obs::run(
        Update,
        "tickets",
        sqlx::query!(
            "UPDATE tickets SET transcript_saved_at = now(), updated_at = now()
             WHERE guild_id = $1 AND id = $2 AND state = 'closing' AND frozen_at IS NOT NULL
               AND transcript_saved_at IS NULL",
            i64::from(guild),
            ticket.get(),
        )
        .execute(&mut *conn),
    )
    .await?;
    if marked.rows_affected() == 0 {
        return Ok(false);
    }
    let done = obs::run(
        Insert,
        "ticket_transcripts",
        sqlx::query!(
            "INSERT INTO ticket_transcripts (ticket_id, guild_id, content, complete, partial_cause, expires_at)
             VALUES ($1, $2, $3, $4, $5, now() + make_interval(days => $6))
             ON CONFLICT (ticket_id) DO NOTHING",
            ticket.get(),
            i64::from(guild),
            content,
            partial.is_none(),
            partial.map(Pg) as _,
            i32::from(retention_days),
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// A ticket's unexpired transcript.
pub async fn transcript(
    conn: &mut PgConnection,
    guild: GuildId,
    ticket: TicketId,
) -> Result<Option<Transcript>, DbError> {
    let row = obs::run(
        Select,
        "ticket_transcripts",
        sqlx::query!(
            "SELECT content, partial_cause, captured_at, expires_at FROM ticket_transcripts
             WHERE guild_id = $1 AND ticket_id = $2 AND expires_at > now()",
            i64::from(guild),
            ticket.get(),
        )
        .fetch_optional(conn),
    )
    .await?;
    Ok(row.map(|r| Transcript {
        content: r.content,
        partial: r.partial_cause.map(|p| p.0),
        captured_at: r.captured_at,
        expires_at: r.expires_at,
    }))
}
