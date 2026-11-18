//! The tickets module (§13.2): settings, tickets, the journal, transcripts.

mod journal;

use quack_core::ids::{AccountId, ChannelId, GuildId, MemberId, TicketId};
use quack_core::modules::{ModuleDisabledReason, TicketCloseReason, TicketState};
use sqlx::PgConnection;
use time::OffsetDateTime;

use crate::DbError;
use crate::obs::{self, Insert, Select, Update};
use crate::pg::{Pg, TimeCursor, count, cursor_parts, id, int, limit, opt_id};

pub use journal::{
    JournalFile, JournalMessage, Transcript, add_journal_message, journal, mark_journal_gaps,
    save_transcript, transcript,
};

/// `ticket_settings`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TicketSettings {
    /// Whether tickets are on.
    pub enabled: bool,
    /// Why Quack turned them off.
    pub disabled_reason: Option<ModuleDisabledReason>,
    /// Where members open tickets.
    pub entry_channel: Option<ChannelId>,
    /// Where staff see ticket posts.
    pub queue_channel: Option<ChannelId>,
    /// How long transcripts are kept.
    pub transcript_retention_days: u16,
    /// Incremented by every change.
    pub version: u64,
}

struct SettingsRow {
    enabled: bool,
    disabled_reason: Option<Pg<ModuleDisabledReason>>,
    entry_channel_id: Option<i64>,
    queue_channel_id: Option<i64>,
    transcript_retention_days: i16,
    version: i64,
}

impl TryFrom<SettingsRow> for TicketSettings {
    type Error = DbError;

    fn try_from(r: SettingsRow) -> Result<Self, DbError> {
        Ok(Self {
            enabled: r.enabled,
            disabled_reason: r.disabled_reason.map(|d| d.0),
            entry_channel: opt_id(r.entry_channel_id)?,
            queue_channel: opt_id(r.queue_channel_id)?,
            transcript_retention_days: count(r.transcript_retention_days)?,
            version: count(r.version)?,
        })
    }
}

/// The ticket settings of a server.
pub async fn settings(
    conn: &mut PgConnection,
    guild: GuildId,
) -> Result<Option<TicketSettings>, DbError> {
    let row = obs::run(
        Select,
        "ticket_settings",
        sqlx::query_as!(
            SettingsRow,
            "SELECT enabled, disabled_reason, entry_channel_id, queue_channel_id, transcript_retention_days, version
             FROM ticket_settings WHERE guild_id = $1",
            i64::from(guild)
        )
        .fetch_optional(conn),
    )
    .await?;
    row.map(TicketSettings::try_from).transpose()
}

/// Locks the ticket settings `FOR UPDATE` (§4.12 step 3).
pub async fn lock_settings(
    conn: &mut PgConnection,
    guild: GuildId,
) -> Result<Option<TicketSettings>, DbError> {
    let row = obs::run(
        Select,
        "ticket_settings",
        sqlx::query_as!(
            SettingsRow,
            "SELECT enabled, disabled_reason, entry_channel_id, queue_channel_id, transcript_retention_days, version
             FROM ticket_settings WHERE guild_id = $1 FOR UPDATE",
            i64::from(guild)
        )
        .fetch_optional(conn),
    )
    .await?;
    row.map(TicketSettings::try_from).transpose()
}

/// Writes the ticket settings (every field validated by the caller) if
/// still at `expected_version`. Returns the new version.
pub async fn update_settings(
    conn: &mut PgConnection,
    guild: GuildId,
    entry: Option<ChannelId>,
    queue: Option<ChannelId>,
    retention_days: u16,
    expected_version: u64,
    by: AccountId,
) -> Result<Option<u64>, DbError> {
    let version = obs::run(
        Update,
        "ticket_settings",
        sqlx::query_scalar!(
            "UPDATE ticket_settings SET entry_channel_id = $3, queue_channel_id = $4, transcript_retention_days = $5,
                                        version = version + 1, updated_at = now(), updated_by = $6
             WHERE guild_id = $1 AND version = $2 RETURNING version",
            i64::from(guild),
            int::<u64, i64>(expected_version)?,
            entry.map(i64::from),
            queue.map(i64::from),
            int::<u16, i16>(retention_days)?,
            i64::from(by),
        )
        .fetch_optional(conn),
    )
    .await?;
    version.map(count).transpose()
}

/// Channel reconciliation (§13.1.5): clears the entry or queue channel if it
/// still holds `channel`, and turns tickets off (`channel_deleted`).
/// Returns `(cleared, turned_off)`.
pub async fn clear_channel(
    conn: &mut PgConnection,
    guild: GuildId,
    channel: ChannelId,
) -> Result<(bool, bool), DbError> {
    let row = obs::run(
        Update,
        "ticket_settings",
        sqlx::query!(
            r#"UPDATE ticket_settings SET
                 entry_channel_id = NULLIF(entry_channel_id, $2), queue_channel_id = NULLIF(queue_channel_id, $2),
                 disabled_reason = CASE WHEN enabled THEN 'channel_deleted' ELSE disabled_reason END,
                 enabled = false, version = version + 1, updated_at = now(), updated_by = NULL
               WHERE guild_id = $1 AND (entry_channel_id = $2 OR queue_channel_id = $2)
               RETURNING old.enabled AS "was_enabled!""#,
            i64::from(guild),
            i64::from(channel),
        )
        .fetch_optional(conn),
    )
    .await?;
    Ok(row.map_or((false, false), |r| (true, r.was_enabled)))
}

/// A ticket.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ticket {
    /// The ticket.
    pub id: TicketId,
    /// Its server.
    pub guild: GuildId,
    /// Its member.
    pub member: MemberId,
    /// Its state.
    pub state: TicketState,
    /// The thread's parent.
    pub entry_channel: ChannelId,
    /// The private thread.
    pub thread: Option<ChannelId>,
    /// When it became `open`.
    pub opened_at: Option<OffsetDateTime>,
    /// When closing was requested.
    pub close_requested_at: Option<OffsetDateTime>,
    /// When it closed.
    pub closed_at: Option<OffsetDateTime>,
    /// Who closed it; `None` = Quack.
    pub closed_by: Option<AccountId>,
    /// Why it closed without opening.
    pub close_reason: Option<TicketCloseReason>,
    /// The journal may be missing messages.
    pub journal_gaps: bool,
    /// The welcome's nonce window start.
    pub welcome_started_at: Option<OffsetDateTime>,
    /// When the welcome was posted.
    pub welcome_posted_at: Option<OffsetDateTime>,
    /// Close step 1 done.
    pub frozen_at: Option<OffsetDateTime>,
    /// Close step 2 done.
    pub transcript_saved_at: Option<OffsetDateTime>,
    /// Close step 4 done.
    pub thread_deleted_at: Option<OffsetDateTime>,
    /// Discord refused to delete the thread.
    pub thread_delete_failed: bool,
    /// When it was created.
    pub created_at: OffsetDateTime,
}

struct TicketRow {
    id: uuid::Uuid,
    guild_id: i64,
    member_id: i64,
    state: Pg<TicketState>,
    entry_channel_id: i64,
    thread_id: Option<i64>,
    opened_at: Option<OffsetDateTime>,
    close_requested_at: Option<OffsetDateTime>,
    closed_at: Option<OffsetDateTime>,
    closed_by: Option<i64>,
    close_reason: Option<Pg<TicketCloseReason>>,
    journal_gaps: bool,
    welcome_started_at: Option<OffsetDateTime>,
    welcome_posted_at: Option<OffsetDateTime>,
    frozen_at: Option<OffsetDateTime>,
    transcript_saved_at: Option<OffsetDateTime>,
    thread_deleted_at: Option<OffsetDateTime>,
    thread_delete_failed: bool,
    created_at: OffsetDateTime,
}

impl TryFrom<TicketRow> for Ticket {
    type Error = DbError;

    fn try_from(r: TicketRow) -> Result<Self, DbError> {
        Ok(Self {
            id: TicketId::from_uuid(r.id),
            guild: id(r.guild_id)?,
            member: id(r.member_id)?,
            state: r.state.0,
            entry_channel: id(r.entry_channel_id)?,
            thread: opt_id(r.thread_id)?,
            opened_at: r.opened_at,
            close_requested_at: r.close_requested_at,
            closed_at: r.closed_at,
            closed_by: opt_id(r.closed_by)?,
            close_reason: r.close_reason.map(|c| c.0),
            journal_gaps: r.journal_gaps,
            welcome_started_at: r.welcome_started_at,
            welcome_posted_at: r.welcome_posted_at,
            frozen_at: r.frozen_at,
            transcript_saved_at: r.transcript_saved_at,
            thread_deleted_at: r.thread_deleted_at,
            thread_delete_failed: r.thread_delete_failed,
            created_at: r.created_at,
        })
    }
}

macro_rules! query_ticket {
    ($tail:literal $(, $arg:expr)* $(,)?) => {
        ::sqlx::sqlx_macros::expand_query!(
            record = TicketRow,
            source = "SELECT id, guild_id, member_id, state, entry_channel_id, thread_id, opened_at, close_requested_at,
                             closed_at, closed_by, close_reason, journal_gaps, welcome_started_at, welcome_posted_at,
                             frozen_at, transcript_saved_at, thread_deleted_at, thread_delete_failed, created_at
                      FROM tickets " + $tail,
            args = [$($arg),*]
        )
    };
}

/// Opens a ticket (`opening`). A member with a live ticket is
/// [`Conflict::LiveTicketExists`](crate::Conflict::LiveTicketExists).
pub async fn insert(
    conn: &mut PgConnection,
    ticket: TicketId,
    guild: GuildId,
    member: MemberId,
    entry_channel: ChannelId,
) -> Result<(), DbError> {
    obs::run(
        Insert,
        "tickets",
        sqlx::query!(
            "INSERT INTO tickets (id, guild_id, member_id, entry_channel_id) VALUES ($1, $2, $3, $4)",
            ticket.get(),
            i64::from(guild),
            i64::from(member),
            i64::from(entry_channel),
        )
        .execute(conn),
    )
    .await?;
    Ok(())
}

/// A ticket of this server.
pub async fn get(
    conn: &mut PgConnection,
    guild: GuildId,
    ticket: TicketId,
) -> Result<Option<Ticket>, DbError> {
    let row = obs::run(
        Select,
        "tickets",
        query_ticket!(
            "WHERE guild_id = $1 AND id = $2",
            i64::from(guild),
            ticket.get()
        )
        .fetch_optional(conn),
    )
    .await?;
    row.map(Ticket::try_from).transpose()
}

/// Locks a ticket `FOR UPDATE`.
pub async fn lock(
    conn: &mut PgConnection,
    guild: GuildId,
    ticket: TicketId,
) -> Result<Option<Ticket>, DbError> {
    let row = obs::run(
        Select,
        "tickets",
        query_ticket!(
            "WHERE guild_id = $1 AND id = $2 FOR UPDATE",
            i64::from(guild),
            ticket.get()
        )
        .fetch_optional(conn),
    )
    .await?;
    row.map(Ticket::try_from).transpose()
}

/// The member's live ticket (`opening`, `open`, or `closing`).
pub async fn live_for_member(
    conn: &mut PgConnection,
    guild: GuildId,
    member: MemberId,
) -> Result<Option<Ticket>, DbError> {
    let row = obs::run(
        Select,
        "tickets",
        query_ticket!(
            "WHERE guild_id = $1 AND member_id = $2 AND state IN ('opening', 'open', 'closing')",
            i64::from(guild),
            i64::from(member)
        )
        .fetch_optional(conn),
    )
    .await?;
    row.map(Ticket::try_from).transpose()
}

/// The ticket whose thread this is.
pub async fn by_thread(
    conn: &mut PgConnection,
    thread: ChannelId,
) -> Result<Option<Ticket>, DbError> {
    let row = obs::run(
        Select,
        "tickets",
        query_ticket!("WHERE thread_id = $1", i64::from(thread)).fetch_optional(conn),
    )
    .await?;
    row.map(Ticket::try_from).transpose()
}

/// A server's tickets (optionally in one state), newest first.
pub async fn list(
    conn: &mut PgConnection,
    guild: GuildId,
    state: Option<TicketState>,
    cursor: Option<TimeCursor>,
    page: u32,
) -> Result<Vec<Ticket>, DbError> {
    let (after_time, after_id) = cursor_parts(cursor);
    let rows = obs::run(
        Select,
        "tickets",
        query_ticket!(
            "WHERE guild_id = $1 AND ($2::ticket_state IS NULL OR state = $2)
               AND ($3::timestamptz IS NULL OR (created_at, id) < ($3, $4))
             ORDER BY created_at DESC, id DESC LIMIT $5",
            i64::from(guild),
            state.map(Pg),
            after_time,
            after_id,
            limit(page, 100),
        )
        .fetch_all(conn),
    )
    .await?;
    rows.into_iter().map(Ticket::try_from).collect()
}

/// Open tickets: in one server, or across servers for the gauge.
pub async fn count_open(conn: &mut PgConnection, guild: Option<GuildId>) -> Result<u64, DbError> {
    let n = obs::run(
        Select,
        "tickets",
        sqlx::query_scalar!(
            r#"SELECT count(*) AS "n!" FROM tickets WHERE state = 'open' AND ($1::bigint IS NULL OR guild_id = $1)"#,
            guild.map(i64::from)
        )
        .fetch_one(conn),
    )
    .await?;
    count(n)
}

mod steps;

pub use steps::{
    begin_welcome, fail_open, mark_closed, mark_frozen, mark_open, mark_thread_deleted,
    mark_welcome_posted, request_close, set_thread,
};
