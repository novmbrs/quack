//! DMs (§6): one row per case DM, appeal update, or ticket transcript DM,
//! sent at most once.

use quack_core::dm::{DmFailure, DmKind, DmState};
use quack_core::ids::{
    AppealEventId, CaseId, ChannelId, DmId, GuildId, MemberId, MessageId, TicketId,
};
use sqlx::PgConnection;
use time::OffsetDateTime;

use crate::DbError;
use crate::obs::{self, Insert, Select, Update};
use crate::pg::{Pg, count, id, int, opt_id};

/// A DM row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dm {
    /// The DM.
    pub id: DmId,
    /// Its server.
    pub guild: GuildId,
    /// The member it goes to.
    pub member: MemberId,
    /// What it is about.
    pub kind: DmKind,
    /// The case, for case and appeal-update DMs.
    pub case: Option<CaseId>,
    /// The appeal event, for appeal-update DMs.
    pub appeal_event: Option<AppealEventId>,
    /// The ticket, for ticket-closed DMs.
    pub ticket: Option<TicketId>,
    /// Its state.
    pub state: DmState,
    /// Why it failed.
    pub failure: Option<DmFailure>,
    /// Discord's JSON error code for the failure.
    pub failure_discord_code: Option<u32>,
    /// The DM channel, once opened.
    pub channel: Option<ChannelId>,
    /// The sent message.
    pub message: Option<MessageId>,
    /// Start of the nonce window (§10.4).
    pub first_attempt_at: Option<OffsetDateTime>,
    /// When it reached a terminal state.
    pub settled_at: Option<OffsetDateTime>,
    /// When it was created.
    pub created_at: OffsetDateTime,
}

/// What a new DM is about; fixes the kind and its owner column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DmOwner {
    /// The case DM.
    Case(CaseId),
    /// An appeal update, rendered from its event.
    AppealUpdate(CaseId, AppealEventId),
    /// A closed ticket's transcript.
    TicketClosed(TicketId),
}

/// Inserts a `pending` DM. A second one for the same owner is
/// [`Conflict::DmExists`](crate::Conflict::DmExists).
pub async fn insert(
    conn: &mut PgConnection,
    dm: DmId,
    guild: GuildId,
    member: MemberId,
    owner: DmOwner,
) -> Result<(), DbError> {
    let (kind, case, event, ticket) = match owner {
        DmOwner::Case(case) => (DmKind::Case, Some(case), None, None),
        DmOwner::AppealUpdate(case, event) => (DmKind::AppealUpdate, Some(case), Some(event), None),
        DmOwner::TicketClosed(ticket) => (DmKind::TicketClosed, None, None, Some(ticket)),
    };
    obs::run(
        Insert,
        "dms",
        sqlx::query!(
            "INSERT INTO dms (id, guild_id, member_id, kind, case_id, appeal_event_id, ticket_id)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
            dm.get(),
            i64::from(guild),
            i64::from(member),
            Pg(kind) as _,
            case.map(CaseId::get),
            event.map(AppealEventId::get),
            ticket.map(TicketId::get),
        )
        .execute(conn),
    )
    .await?;
    Ok(())
}

struct DmRow {
    id: uuid::Uuid,
    guild_id: i64,
    member_id: i64,
    kind: Pg<DmKind>,
    case_id: Option<uuid::Uuid>,
    appeal_event_id: Option<uuid::Uuid>,
    ticket_id: Option<uuid::Uuid>,
    state: Pg<DmState>,
    failure: Option<Pg<DmFailure>>,
    failure_discord_code: Option<i32>,
    channel_id: Option<i64>,
    message_id: Option<i64>,
    first_attempt_at: Option<OffsetDateTime>,
    settled_at: Option<OffsetDateTime>,
    created_at: OffsetDateTime,
}

impl TryFrom<DmRow> for Dm {
    type Error = DbError;

    fn try_from(r: DmRow) -> Result<Self, DbError> {
        Ok(Self {
            id: DmId::from_uuid(r.id),
            guild: id(r.guild_id)?,
            member: id(r.member_id)?,
            kind: r.kind.0,
            case: r.case_id.map(CaseId::from_uuid),
            appeal_event: r.appeal_event_id.map(AppealEventId::from_uuid),
            ticket: r.ticket_id.map(TicketId::from_uuid),
            state: r.state.0,
            failure: r.failure.map(|f| f.0),
            failure_discord_code: r.failure_discord_code.map(count).transpose()?,
            channel: opt_id(r.channel_id)?,
            message: opt_id(r.message_id)?,
            first_attempt_at: r.first_attempt_at,
            settled_at: r.settled_at,
            created_at: r.created_at,
        })
    }
}

macro_rules! query_dm {
    ($tail:literal $(, $arg:expr)* $(,)?) => {
        ::sqlx::sqlx_macros::expand_query!(
            record = DmRow,
            source = "SELECT id, guild_id, member_id, kind, case_id, appeal_event_id, ticket_id, state, failure,
                             failure_discord_code, channel_id, message_id, first_attempt_at, settled_at, created_at
                      FROM dms " + $tail,
            args = [$($arg),*]
        )
    };
}

/// A DM of this server.
pub async fn get(conn: &mut PgConnection, guild: GuildId, dm: DmId) -> Result<Option<Dm>, DbError> {
    let row = obs::run(
        Select,
        "dms",
        query_dm!(
            "WHERE guild_id = $1 AND id = $2",
            i64::from(guild),
            dm.get()
        )
        .fetch_optional(conn),
    )
    .await?;
    row.map(Dm::try_from).transpose()
}

/// Locks a DM `FOR UPDATE` (after its case, §4.12).
pub async fn lock(
    conn: &mut PgConnection,
    guild: GuildId,
    dm: DmId,
) -> Result<Option<Dm>, DbError> {
    let row = obs::run(
        Select,
        "dms",
        query_dm!(
            "WHERE guild_id = $1 AND id = $2 FOR UPDATE",
            i64::from(guild),
            dm.get()
        )
        .fetch_optional(conn),
    )
    .await?;
    row.map(Dm::try_from).transpose()
}

/// Every DM about a case (its case DM and appeal updates), oldest first.
pub async fn for_case(
    conn: &mut PgConnection,
    guild: GuildId,
    case: CaseId,
) -> Result<Vec<Dm>, DbError> {
    let rows = obs::run(
        Select,
        "dms",
        query_dm!(
            "WHERE guild_id = $1 AND case_id = $2 ORDER BY created_at, id",
            i64::from(guild),
            case.get()
        )
        .fetch_all(conn),
    )
    .await?;
    rows.into_iter().map(Dm::try_from).collect()
}

/// `pending` → `sending`: the DM channel is open and the nonce window
/// starts now (§6.4 step 2). Returns the window's start.
pub async fn start_sending(
    conn: &mut PgConnection,
    guild: GuildId,
    dm: DmId,
    channel: ChannelId,
) -> Result<Option<OffsetDateTime>, DbError> {
    obs::run(
        Update,
        "dms",
        sqlx::query_scalar!(
            r#"UPDATE dms SET state = 'sending', channel_id = $3, first_attempt_at = now()
               WHERE guild_id = $1 AND id = $2 AND state = 'pending'
               RETURNING first_attempt_at AS "first_attempt_at!""#,
            i64::from(guild),
            dm.get(),
            i64::from(channel),
        )
        .fetch_optional(conn),
    )
    .await
}

/// `sending` → `sent` with the created message.
pub async fn mark_sent(
    conn: &mut PgConnection,
    guild: GuildId,
    dm: DmId,
    message: MessageId,
) -> Result<bool, DbError> {
    let done = obs::run(
        Update,
        "dms",
        sqlx::query!(
            "UPDATE dms SET state = 'sent', message_id = $3, settled_at = now()
             WHERE guild_id = $1 AND id = $2 AND state = 'sending'",
            i64::from(guild),
            dm.get(),
            i64::from(message),
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// `from` (`pending` or `sending`) → `failed`.
pub async fn mark_failed(
    conn: &mut PgConnection,
    guild: GuildId,
    dm: DmId,
    from: DmState,
    failure: DmFailure,
    discord_code: Option<u32>,
) -> Result<bool, DbError> {
    let done = obs::run(
        Update,
        "dms",
        sqlx::query!(
            "UPDATE dms SET state = 'failed', failure = $4, failure_discord_code = $5, settled_at = now()
             WHERE guild_id = $1 AND id = $2 AND state = $3",
            i64::from(guild),
            dm.get(),
            Pg(from) as _,
            Pg(failure) as _,
            discord_code.map(int::<u32, i32>).transpose()?,
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// `sending` → `unknown` (couldn't confirm; terminal, never resent).
pub async fn mark_unknown(
    conn: &mut PgConnection,
    guild: GuildId,
    dm: DmId,
) -> Result<bool, DbError> {
    let done = obs::run(
        Update,
        "dms",
        sqlx::query!(
            "UPDATE dms SET state = 'unknown', settled_at = now()
             WHERE guild_id = $1 AND id = $2 AND state = 'sending'",
            i64::from(guild),
            dm.get(),
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// `sending` → `pending`: nothing was sent (a rate limit that didn't fit
/// the window, with no unknown create), so the next run starts a new window.
pub async fn back_to_pending(
    conn: &mut PgConnection,
    guild: GuildId,
    dm: DmId,
) -> Result<bool, DbError> {
    let done = obs::run(
        Update,
        "dms",
        sqlx::query!(
            "UPDATE dms SET state = 'pending', first_attempt_at = NULL
             WHERE guild_id = $1 AND id = $2 AND state = 'sending'",
            i64::from(guild),
            dm.get(),
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Void (§4.7 step 2.6): the case's `pending` case DM → `not_needed`.
/// Returns it, if there was one.
pub async fn cancel_pending_case_dm(
    conn: &mut PgConnection,
    guild: GuildId,
    case: CaseId,
) -> Result<Option<DmId>, DbError> {
    let id = obs::run(
        Update,
        "dms",
        sqlx::query_scalar!(
            "UPDATE dms SET state = 'not_needed', settled_at = now()
             WHERE guild_id = $1 AND case_id = $2 AND kind = 'case' AND state = 'pending'
             RETURNING id",
            i64::from(guild),
            case.get(),
        )
        .fetch_optional(conn),
    )
    .await?;
    Ok(id.map(DmId::from_uuid))
}

/// `pending` → `not_needed` for one DM (a case DM whose case was voided
/// when its run started, §6.2).
pub async fn mark_not_needed(
    conn: &mut PgConnection,
    guild: GuildId,
    dm: DmId,
) -> Result<bool, DbError> {
    let done = obs::run(
        Update,
        "dms",
        sqlx::query!(
            "UPDATE dms SET state = 'not_needed', settled_at = now()
             WHERE guild_id = $1 AND id = $2 AND state = 'pending'",
            i64::from(guild),
            dm.get(),
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}
