//! Tracked Discord messages (§10): one row per receipt, appeal post, ticket
//! panel, ticket post, and honeypot warning, written to Discord only by its
//! `sync_message` job.

use quack_core::ids::{
    AppealId, AttachmentId, CaseId, ChannelId, GuildId, MessageId, MessageRowId, TicketId,
};
use quack_core::messages::{MessageKind, MessageState};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use sqlx::types::Json;
use time::OffsetDateTime;

use crate::jobs::{self, NewJob};
use crate::obs::{self, Insert, Select, Update};
use crate::pg::{Pg, count, id, int, opt_id};
use crate::{DbError, Tx};

/// Who a tracked message belongs to; fixes its kind for owned messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    /// The case's receipt.
    Case(CaseId),
    /// The appeal's staff post.
    Appeal(AppealId),
    /// The ticket's staff post.
    Ticket(TicketId),
    /// A one-per-server message (`ticket_panel` or `honeypot_warning`).
    Server(MessageKind),
}

impl Owner {
    /// The message kind this owner has.
    pub fn kind(self) -> MessageKind {
        match self {
            Self::Case(_) => MessageKind::CaseReceipt,
            Self::Appeal(_) => MessageKind::AppealPost,
            Self::Ticket(_) => MessageKind::TicketPost,
            Self::Server(kind) => kind,
        }
    }

    fn columns(self) -> (Option<uuid::Uuid>, Option<uuid::Uuid>, Option<uuid::Uuid>) {
        match self {
            Self::Case(c) => (Some(c.get()), None, None),
            Self::Appeal(a) => (None, Some(a.get()), None),
            Self::Ticket(t) => (None, None, Some(t.get())),
            Self::Server(_) => (None, None, None),
        }
    }
}

/// An earlier location still to clean up after a move.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    /// The channel.
    pub channel_id: ChannelId,
    /// The message.
    pub message_id: MessageId,
}

/// A tracked message row.
#[derive(Clone, PartialEq, Eq)]
pub struct TrackedMessage {
    /// The row.
    pub id: MessageRowId,
    /// Its server.
    pub guild: GuildId,
    /// Its owner (and so its kind).
    pub owner: Owner,
    /// Where it lives; `None` for an appeal post waiting for a channel.
    pub channel: Option<ChannelId>,
    /// The Discord message, once posted.
    pub message: Option<MessageId>,
    /// Incremented whenever it must be posted anew.
    pub generation: u32,
    /// When the current create began (its nonce window).
    pub create_started_at: Option<OffsetDateTime>,
    /// A private receipt, written only through the interaction token.
    pub private: bool,
    /// The interaction token and when it stops working. Secret: never logged.
    pub interaction_token: Option<(String, OffsetDateTime)>,
    /// The revision the message should show.
    pub wanted_revision: u32,
    /// The highest revision on Discord.
    pub posted_revision: u32,
    /// When `wanted_revision` last changed.
    pub wanted_at: OffsetDateTime,
    /// `active` or `gone`.
    pub state: MessageState,
    /// Earlier locations to clean up (at most 5).
    pub stale: Vec<Location>,
    /// The attached file (the ticket transcript), kept on later edits.
    pub attachment_id: Option<AttachmentId>,
    /// Machine code of the last failed sync.
    pub last_error: Option<String>,
}

impl std::fmt::Debug for TrackedMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrackedMessage")
            .field("id", &self.id)
            .field("owner", &self.owner)
            .field("channel", &self.channel)
            .field("message", &self.message)
            .field("generation", &self.generation)
            .field("wanted_revision", &self.wanted_revision)
            .field("posted_revision", &self.posted_revision)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

/// A new tracked message.
#[derive(Clone, PartialEq, Eq)]
pub struct NewTrackedMessage {
    /// A fresh `UUIDv7`.
    pub id: MessageRowId,
    /// The server.
    pub guild: GuildId,
    /// Its owner.
    pub owner: Owner,
    /// Where to post it (`None` only for an appeal post with no channel).
    pub channel: Option<ChannelId>,
    /// A private receipt.
    pub private: bool,
    /// The interaction token and its expiry (receipts only).
    pub interaction_token: Option<(String, OffsetDateTime)>,
}

impl std::fmt::Debug for NewTrackedMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NewTrackedMessage")
            .field("id", &self.id)
            .field("owner", &self.owner)
            .field("channel", &self.channel)
            .finish_non_exhaustive()
    }
}

/// Inserts a tracked message and enqueues its first sync.
pub async fn insert(tx: &mut Tx, message: &NewTrackedMessage) -> Result<(), DbError> {
    let (case, appeal, ticket) = message.owner.columns();
    let (token, expires) = message.interaction_token.clone().unzip();
    obs::run(
        Insert,
        "messages",
        sqlx::query!(
            "INSERT INTO messages (id, guild_id, kind, case_id, appeal_id, ticket_id, channel_id, private,
                                   interaction_token, token_expires_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
            message.id.get(),
            i64::from(message.guild),
            Pg(message.owner.kind()) as _,
            case,
            appeal,
            ticket,
            message.channel.map(i64::from),
            message.private,
            token,
            expires,
        )
        .execute(&mut **tx),
    )
    .await?;
    jobs::enqueue(tx, &NewJob::sync_message(message.guild, message.id)).await?;
    Ok(())
}

/// Bumps the owner's active message and enqueues its sync (§10.2).
/// Returns the row, or `None` when the owner has no active message.
pub async fn bump(
    tx: &mut Tx,
    guild: GuildId,
    owner: Owner,
) -> Result<Option<MessageRowId>, DbError> {
    let (case, appeal, ticket) = owner.columns();
    let row = obs::run(
        Update,
        "messages",
        sqlx::query_scalar!(
            "UPDATE messages SET wanted_revision = wanted_revision + 1, wanted_at = now(), updated_at = now()
             WHERE guild_id = $1 AND kind = $2 AND state = 'active'
               AND case_id IS NOT DISTINCT FROM $3 AND appeal_id IS NOT DISTINCT FROM $4
               AND ticket_id IS NOT DISTINCT FROM $5
             RETURNING id",
            i64::from(guild),
            Pg(owner.kind()) as _,
            case,
            appeal,
            ticket,
        )
        .fetch_optional(&mut **tx),
    )
    .await?;
    let Some(row) = row.map(MessageRowId::from_uuid) else {
        return Ok(None);
    };
    jobs::enqueue(tx, &NewJob::sync_message(guild, row)).await?;
    Ok(Some(row))
}

pub(crate) struct MessageRow {
    id: uuid::Uuid,
    guild_id: i64,
    kind: Pg<MessageKind>,
    case_id: Option<uuid::Uuid>,
    appeal_id: Option<uuid::Uuid>,
    ticket_id: Option<uuid::Uuid>,
    channel_id: Option<i64>,
    message_id: Option<i64>,
    generation: i32,
    create_started_at: Option<OffsetDateTime>,
    private: bool,
    interaction_token: Option<String>,
    token_expires_at: Option<OffsetDateTime>,
    wanted_revision: i32,
    posted_revision: i32,
    wanted_at: OffsetDateTime,
    state: Pg<MessageState>,
    stale: Json<Vec<Location>>,
    attachment_id: Option<i64>,
    last_error: Option<String>,
}

impl TryFrom<MessageRow> for TrackedMessage {
    type Error = DbError;

    fn try_from(r: MessageRow) -> Result<Self, DbError> {
        let owner = match (r.kind.0, r.case_id, r.appeal_id, r.ticket_id) {
            (MessageKind::CaseReceipt, Some(c), ..) => Owner::Case(CaseId::from_uuid(c)),
            (MessageKind::AppealPost, _, Some(a), _) => Owner::Appeal(AppealId::from_uuid(a)),
            (MessageKind::TicketPost, _, _, Some(t)) => Owner::Ticket(TicketId::from_uuid(t)),
            (
                kind @ (MessageKind::TicketPanel | MessageKind::HoneypotWarning),
                None,
                None,
                None,
            ) => Owner::Server(kind),
            _ => return Err(DbError::Decode("tracked message owner".to_owned())),
        };
        Ok(Self {
            id: MessageRowId::from_uuid(r.id),
            guild: id(r.guild_id)?,
            owner,
            channel: opt_id(r.channel_id)?,
            message: opt_id(r.message_id)?,
            generation: count(r.generation)?,
            create_started_at: r.create_started_at,
            private: r.private,
            interaction_token: r.interaction_token.zip(r.token_expires_at),
            wanted_revision: count(r.wanted_revision)?,
            posted_revision: count(r.posted_revision)?,
            wanted_at: r.wanted_at,
            state: r.state.0,
            stale: r.stale.0,
            attachment_id: opt_id(r.attachment_id)?,
            last_error: r.last_error,
        })
    }
}

macro_rules! query_message {
    ($tail:literal $(, $arg:expr)* $(,)?) => {
        ::sqlx::sqlx_macros::expand_query!(
            record = MessageRow,
            source = r#"SELECT id, guild_id, kind, case_id, appeal_id, ticket_id, channel_id, message_id, generation,
                               create_started_at, private, interaction_token, token_expires_at, wanted_revision,
                               posted_revision, wanted_at, state, stale AS "stale: Json<Vec<Location>>",
                               attachment_id, last_error
                        FROM messages "# + $tail,
            args = [$($arg),*]
        )
    };
}

/// A tracked message of this server.
pub async fn get(
    conn: &mut PgConnection,
    guild: GuildId,
    row: MessageRowId,
) -> Result<Option<TrackedMessage>, DbError> {
    let found = obs::run(
        Select,
        "messages",
        query_message!(
            "WHERE guild_id = $1 AND id = $2",
            i64::from(guild),
            row.get()
        )
        .fetch_optional(conn),
    )
    .await?;
    found.map(TrackedMessage::try_from).transpose()
}

/// Locks a tracked message `FOR UPDATE` (sync read and record steps).
pub async fn lock(
    conn: &mut PgConnection,
    guild: GuildId,
    row: MessageRowId,
) -> Result<Option<TrackedMessage>, DbError> {
    let found = obs::run(
        Select,
        "messages",
        query_message!(
            "WHERE guild_id = $1 AND id = $2 FOR UPDATE",
            i64::from(guild),
            row.get()
        )
        .fetch_optional(conn),
    )
    .await?;
    found.map(TrackedMessage::try_from).transpose()
}

/// The owner's tracked message, active or gone.
pub async fn by_owner(
    conn: &mut PgConnection,
    guild: GuildId,
    owner: Owner,
) -> Result<Option<TrackedMessage>, DbError> {
    let (case, appeal, ticket) = owner.columns();
    let found = obs::run(
        Select,
        "messages",
        query_message!(
            "WHERE guild_id = $1 AND kind = $2 AND case_id IS NOT DISTINCT FROM $3
               AND appeal_id IS NOT DISTINCT FROM $4 AND ticket_id IS NOT DISTINCT FROM $5",
            i64::from(guild),
            Pg(owner.kind()),
            case,
            appeal,
            ticket,
        )
        .fetch_optional(conn),
    )
    .await?;
    found.map(TrackedMessage::try_from).transpose()
}

/// The tracked message at a Discord location (`MESSAGE_DELETE`).
pub async fn by_location(
    conn: &mut PgConnection,
    channel: ChannelId,
    message: MessageId,
) -> Result<Option<TrackedMessage>, DbError> {
    let found = obs::run(
        Select,
        "messages",
        query_message!(
            "WHERE channel_id = $1 AND message_id = $2",
            i64::from(channel),
            i64::from(message)
        )
        .fetch_optional(conn),
    )
    .await?;
    found.map(TrackedMessage::try_from).transpose()
}

mod sync;

pub use sync::{
    bind_waiting_appeal_posts, clear_token, make_private, mark_gone, move_open_appeal_posts,
    push_stale, reactivate, record_posted, relocate, remove_stale, set_attachment, set_error,
    start_create,
};

/// Converts a revision for SQL.
pub(crate) fn rev(revision: u32) -> Result<i32, DbError> {
    int::<u32, i32>(revision)
}
