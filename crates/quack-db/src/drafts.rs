//! Multi-page `/case add` drafts (§11.6). The page counter is advanced by
//! compare-and-set, so a double click saves a page once.

use std::time::Duration;

use quack_core::ids::{AccountId, ChannelId, GuildId, MemberId, MessageId, RuleId};
use sqlx::PgConnection;
use time::OffsetDateTime;

use crate::DbError;
use crate::obs::{self, Delete, Insert, Select, Update};
use crate::pg::{count, id, int, opt_id};

/// How long a draft lives after it is created.
pub const DRAFT_TTL: Duration = Duration::from_mins(15);

/// A draft.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draft {
    /// The draft (its ID is also in the modal's custom ID).
    pub id: uuid::Uuid,
    /// Its server.
    pub guild: GuildId,
    /// The moderator it belongs to.
    pub actor: AccountId,
    /// The member the case will be about.
    pub member: MemberId,
    /// The rule and the version it was started with, once chosen.
    pub rule: Option<(RuleId, u32)>,
    /// Whether the receipt is private.
    pub private: bool,
    /// Where the command was used.
    pub receipt_channel: ChannelId,
    /// The message command's message.
    pub source: Option<(ChannelId, MessageId)>,
    /// The values saved so far (field key → value).
    pub values: serde_json::Value,
    /// The next page to save; past the last page means every page is saved.
    pub page: u8,
    /// When it expires.
    pub expires_at: OffsetDateTime,
}

/// A new draft.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewDraft {
    /// The draft's ID (a fresh `UUIDv7`, from the custom ID).
    pub id: uuid::Uuid,
    /// The server.
    pub guild: GuildId,
    /// The moderator.
    pub actor: AccountId,
    /// The member.
    pub member: MemberId,
    /// The rule and version, when already known.
    pub rule: Option<(RuleId, u32)>,
    /// Whether the receipt is private.
    pub private: bool,
    /// Where the command was used.
    pub receipt_channel: ChannelId,
    /// The message command's message.
    pub source: Option<(ChannelId, MessageId)>,
}

/// Creates a draft unless it exists (the first page's submit, or a double
/// click). Returns whether it was created. Expires after [`DRAFT_TTL`].
pub async fn create(conn: &mut PgConnection, draft: &NewDraft) -> Result<bool, DbError> {
    let done = obs::run(
        Insert,
        "case_drafts",
        sqlx::query!(
            "INSERT INTO case_drafts (id, guild_id, actor_id, member_id, rule_id, rule_version, private,
                                      receipt_channel_id, source_channel_id, source_message_id, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, now() + make_interval(secs => $11))
             ON CONFLICT (id) DO NOTHING",
            draft.id,
            i64::from(draft.guild),
            i64::from(draft.actor),
            i64::from(draft.member),
            draft.rule.map(|(r, _)| r.get()),
            draft.rule.map(|(_, v)| int::<u32, i32>(v)).transpose()?,
            draft.private,
            i64::from(draft.receipt_channel),
            draft.source.map(|(c, _)| i64::from(c)),
            draft.source.map(|(_, m)| i64::from(m)),
            DRAFT_TTL.as_secs_f64(),
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Locks an unexpired draft of this moderator in this server `FOR UPDATE`.
pub async fn lock(
    conn: &mut PgConnection,
    guild: GuildId,
    actor: AccountId,
    draft: uuid::Uuid,
) -> Result<Option<Draft>, DbError> {
    let row = obs::run(
        Select,
        "case_drafts",
        sqlx::query!(
            "SELECT id, guild_id, actor_id, member_id, rule_id, rule_version, private, receipt_channel_id,
                    source_channel_id, source_message_id, values, page, expires_at
             FROM case_drafts
             WHERE id = $1 AND guild_id = $2 AND actor_id = $3 AND expires_at > now()
             FOR UPDATE",
            draft,
            i64::from(guild),
            i64::from(actor),
        )
        .fetch_optional(conn),
    )
    .await?;
    row.map(|r| {
        let rule = match (r.rule_id, r.rule_version) {
            (Some(rule), Some(version)) => Some((RuleId::from_uuid(rule), count(version)?)),
            _ => None,
        };
        let source = match (opt_id(r.source_channel_id)?, opt_id(r.source_message_id)?) {
            (Some(c), Some(m)) => Some((c, m)),
            _ => None,
        };
        Ok(Draft {
            id: r.id,
            guild: id(r.guild_id)?,
            actor: id(r.actor_id)?,
            member: id(r.member_id)?,
            rule,
            private: r.private,
            receipt_channel: id(r.receipt_channel_id)?,
            source,
            values: r.values,
            page: count(r.page)?,
            expires_at: r.expires_at,
        })
    })
    .transpose()
}

/// Stores the values of page `page` (merged into the saved values) and,
/// when `advance`, moves to the next page — only if the draft is still at
/// `page`. Returns whether it saved.
pub async fn save_page(
    conn: &mut PgConnection,
    guild: GuildId,
    actor: AccountId,
    draft: uuid::Uuid,
    page: u8,
    values: &serde_json::Value,
    advance: bool,
) -> Result<bool, DbError> {
    let done = obs::run(
        Update,
        "case_drafts",
        sqlx::query!(
            "UPDATE case_drafts SET values = values || $3, page = CASE WHEN $4 THEN page + 1 ELSE page END,
                                    updated_at = now()
             WHERE id = $1 AND page = $2 AND guild_id = $5 AND actor_id = $6 AND expires_at > now()",
            draft,
            i16::from(page),
            values,
            advance,
            i64::from(guild),
            i64::from(actor),
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Sets the rule of a draft that has none (the picker's select).
pub async fn set_rule(
    conn: &mut PgConnection,
    guild: GuildId,
    draft: uuid::Uuid,
    actor: AccountId,
    rule: RuleId,
    version: u32,
) -> Result<bool, DbError> {
    let done = obs::run(
        Update,
        "case_drafts",
        sqlx::query!(
            "UPDATE case_drafts SET rule_id = $3, rule_version = $4, updated_at = now()
             WHERE id = $1 AND actor_id = $2 AND guild_id = $5 AND rule_id IS NULL AND expires_at > now()",
            draft,
            i64::from(actor),
            rule.get(),
            int::<u32, i32>(version)?,
            i64::from(guild),
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Deletes this moderator's draft in this server (used by its case, or its
/// rule changed).
pub async fn delete(
    conn: &mut PgConnection,
    guild: GuildId,
    actor: AccountId,
    draft: uuid::Uuid,
) -> Result<bool, DbError> {
    let done = obs::run(
        Delete,
        "case_drafts",
        sqlx::query!(
            "DELETE FROM case_drafts WHERE id = $1 AND guild_id = $2 AND actor_id = $3",
            draft,
            i64::from(guild),
            i64::from(actor)
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}
