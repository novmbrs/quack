//! Core server settings (`guild_settings`, §13.1) and what modules share.

use quack_core::ids::{AccountId, ChannelId, GuildId, RoleId, RuleId};
use quack_core::modules::ChannelSetting;
use sqlx::PgConnection;
use time::OffsetDateTime;

use crate::DbError;
use crate::obs::{self, Select, Update};
use crate::pg::{count, id, int, opt_id};

/// A server's core settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuildSettings {
    /// The server.
    pub guild: GuildId,
    /// Incremented by every change (the optimistic check for staff roles).
    pub version: u64,
    /// Configured moderator roles.
    pub moderator_role_ids: Vec<RoleId>,
    /// Configured rules manager roles.
    pub rules_manager_role_ids: Vec<RoleId>,
    /// The log channel.
    pub log_channel: Option<ChannelId>,
    /// The appeal channel.
    pub appeal_channel: Option<ChannelId>,
    /// The evidence channel.
    pub evidence_channel: Option<ChannelId>,
    /// `https://discord.gg/<code>` for rejoining.
    pub appeal_invite_url: Option<String>,
    /// The DM introduction.
    pub dm_intro: Option<String>,
    /// The DM footer.
    pub dm_footer: Option<String>,
    /// The starter rule created at the first join.
    pub starter_rule: Option<RuleId>,
    /// When the starter notice was acknowledged (first time kept).
    pub starter_notice_acknowledged_at: Option<OffsetDateTime>,
    /// When it last changed.
    pub updated_at: OffsetDateTime,
    /// Who changed it last.
    pub updated_by: Option<AccountId>,
}

struct SettingsRow {
    guild_id: i64,
    version: i64,
    moderator_role_ids: Vec<i64>,
    rules_manager_role_ids: Vec<i64>,
    log_channel_id: Option<i64>,
    appeal_channel_id: Option<i64>,
    evidence_channel_id: Option<i64>,
    appeal_invite_url: Option<String>,
    dm_intro: Option<String>,
    dm_footer: Option<String>,
    starter_rule_id: Option<uuid::Uuid>,
    starter_notice_acknowledged_at: Option<OffsetDateTime>,
    updated_at: OffsetDateTime,
    updated_by: Option<i64>,
}

impl TryFrom<SettingsRow> for GuildSettings {
    type Error = DbError;

    fn try_from(r: SettingsRow) -> Result<Self, DbError> {
        Ok(Self {
            guild: id(r.guild_id)?,
            version: count(r.version)?,
            moderator_role_ids: r
                .moderator_role_ids
                .into_iter()
                .map(id)
                .collect::<Result<_, _>>()?,
            rules_manager_role_ids: r
                .rules_manager_role_ids
                .into_iter()
                .map(id)
                .collect::<Result<_, _>>()?,
            log_channel: opt_id(r.log_channel_id)?,
            appeal_channel: opt_id(r.appeal_channel_id)?,
            evidence_channel: opt_id(r.evidence_channel_id)?,
            appeal_invite_url: r.appeal_invite_url,
            dm_intro: r.dm_intro,
            dm_footer: r.dm_footer,
            starter_rule: r.starter_rule_id.map(RuleId::from_uuid),
            starter_notice_acknowledged_at: r.starter_notice_acknowledged_at,
            updated_at: r.updated_at,
            updated_by: opt_id(r.updated_by)?,
        })
    }
}

macro_rules! query_settings {
    ($tail:literal $(, $arg:expr)* $(,)?) => {
        ::sqlx::sqlx_macros::expand_query!(
            record = SettingsRow,
            source = "SELECT guild_id, version, moderator_role_ids, rules_manager_role_ids, log_channel_id,
                             appeal_channel_id, evidence_channel_id, appeal_invite_url, dm_intro, dm_footer,
                             starter_rule_id, starter_notice_acknowledged_at, updated_at, updated_by
                      FROM guild_settings " + $tail,
            args = [$($arg),*]
        )
    };
}

/// A server's settings.
pub async fn get(
    conn: &mut PgConnection,
    guild: GuildId,
) -> Result<Option<GuildSettings>, DbError> {
    let row = obs::run(
        Select,
        "guild_settings",
        query_settings!("WHERE guild_id = $1", i64::from(guild)).fetch_optional(conn),
    )
    .await?;
    row.map(GuildSettings::try_from).transpose()
}

/// Locks a server's settings `FOR UPDATE` (settings writes; rule creation,
/// import, and restore, §4.12 step 2).
pub async fn lock(
    conn: &mut PgConnection,
    guild: GuildId,
) -> Result<Option<GuildSettings>, DbError> {
    let row = obs::run(
        Select,
        "guild_settings",
        query_settings!("WHERE guild_id = $1 FOR UPDATE", i64::from(guild)).fetch_optional(conn),
    )
    .await?;
    row.map(GuildSettings::try_from).transpose()
}

/// Locks a server's settings `FOR SHARE`: a write that reads a setting
/// (the appeal channel for a new appeal's post) waits for a settings change
/// in flight, and a change waits for it, so neither misses the other.
pub async fn lock_shared(
    conn: &mut PgConnection,
    guild: GuildId,
) -> Result<Option<GuildSettings>, DbError> {
    let row = obs::run(
        Select,
        "guild_settings",
        query_settings!("WHERE guild_id = $1 FOR SHARE", i64::from(guild)).fetch_optional(conn),
    )
    .await?;
    row.map(GuildSettings::try_from).transpose()
}

/// Settings of these servers, for the config cache.
pub async fn for_guilds(
    conn: &mut PgConnection,
    guilds: &[GuildId],
) -> Result<Vec<GuildSettings>, DbError> {
    let ids: Vec<i64> = guilds.iter().copied().map(i64::from).collect();
    let rows = obs::run(
        Select,
        "guild_settings",
        query_settings!("WHERE guild_id = ANY($1)", &ids).fetch_all(conn),
    )
    .await?;
    rows.into_iter().map(GuildSettings::try_from).collect()
}

/// The fields a settings update writes; `None` leaves a field alone and
/// `Some(None)` clears it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SettingsPatch {
    /// Moderator roles.
    pub moderator_role_ids: Option<Vec<RoleId>>,
    /// Rules manager roles.
    pub rules_manager_role_ids: Option<Vec<RoleId>>,
    /// The log channel.
    pub log_channel: Option<Option<ChannelId>>,
    /// The appeal channel.
    pub appeal_channel: Option<Option<ChannelId>>,
    /// The evidence channel.
    pub evidence_channel: Option<Option<ChannelId>>,
    /// The rejoin invite.
    pub appeal_invite_url: Option<Option<String>>,
    /// The DM introduction.
    pub dm_intro: Option<Option<String>>,
    /// The DM footer.
    pub dm_footer: Option<Option<String>>,
}

fn roles(ids: Option<&Vec<RoleId>>) -> Option<Vec<i64>> {
    ids.map(|ids| ids.iter().copied().map(i64::from).collect())
}

/// Writes the patch's fields if the settings are still at
/// `expected_version`, `version + 1`. Returns the new version, or `None`
/// when the settings changed.
pub async fn update(
    conn: &mut PgConnection,
    guild: GuildId,
    patch: &SettingsPatch,
    expected_version: u64,
    by: AccountId,
) -> Result<Option<u64>, DbError> {
    let p = patch;
    let moderators = roles(p.moderator_role_ids.as_ref());
    let managers = roles(p.rules_manager_role_ids.as_ref());
    let version = obs::run(
        Update,
        "guild_settings",
        sqlx::query_scalar!(
            "UPDATE guild_settings SET
               moderator_role_ids     = COALESCE($3, moderator_role_ids),
               rules_manager_role_ids = COALESCE($4, rules_manager_role_ids),
               log_channel_id         = CASE WHEN $5 THEN $6 ELSE log_channel_id END,
               appeal_channel_id      = CASE WHEN $7 THEN $8 ELSE appeal_channel_id END,
               evidence_channel_id    = CASE WHEN $9 THEN $10 ELSE evidence_channel_id END,
               appeal_invite_url      = CASE WHEN $11 THEN $12 ELSE appeal_invite_url END,
               dm_intro               = CASE WHEN $13 THEN $14 ELSE dm_intro END,
               dm_footer              = CASE WHEN $15 THEN $16 ELSE dm_footer END,
               version = version + 1, updated_at = now(), updated_by = $17
             WHERE guild_id = $1 AND version = $2
             RETURNING version",
            i64::from(guild),
            int::<u64, i64>(expected_version)?,
            moderators.as_deref(),
            managers.as_deref(),
            p.log_channel.is_some(),
            p.log_channel.flatten().map(i64::from),
            p.appeal_channel.is_some(),
            p.appeal_channel.flatten().map(i64::from),
            p.evidence_channel.is_some(),
            p.evidence_channel.flatten().map(i64::from),
            p.appeal_invite_url.is_some(),
            p.appeal_invite_url.clone().flatten(),
            p.dm_intro.is_some(),
            p.dm_intro.clone().flatten(),
            p.dm_footer.is_some(),
            p.dm_footer.clone().flatten(),
            i64::from(by),
        )
        .fetch_optional(conn),
    )
    .await?;
    version.map(count).transpose()
}

/// Records the starter rule at the first join; an existing one is kept.
/// Returns whether it was recorded.
pub async fn set_starter_rule(
    conn: &mut PgConnection,
    guild: GuildId,
    rule: RuleId,
) -> Result<bool, DbError> {
    let done = obs::run(
        Update,
        "guild_settings",
        sqlx::query!(
            "UPDATE guild_settings SET starter_rule_id = $2 WHERE guild_id = $1 AND starter_rule_id IS NULL",
            i64::from(guild),
            rule.get()
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Acknowledges the starter notice; the first time is kept. Returns
/// whether this call set it.
pub async fn acknowledge_starter_notice(
    conn: &mut PgConnection,
    guild: GuildId,
    by: AccountId,
) -> Result<bool, DbError> {
    let done = obs::run(
        Update,
        "guild_settings",
        sqlx::query!(
            "UPDATE guild_settings SET starter_notice_acknowledged_at = now(), version = version + 1,
                                       updated_at = now(), updated_by = $2
             WHERE guild_id = $1 AND starter_notice_acknowledged_at IS NULL",
            i64::from(guild),
            i64::from(by)
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Compare-and-clear of a core channel setting (§13.1.5, §9.4): clears it
/// only if it still holds `channel`. Module channels are cleared by their
/// own modules. Returns whether it cleared.
pub async fn clear_channel(
    conn: &mut PgConnection,
    guild: GuildId,
    setting: ChannelSetting,
    channel: ChannelId,
) -> Result<bool, DbError> {
    let (log, appeal, evidence) = match setting {
        ChannelSetting::LogChannel => (true, false, false),
        ChannelSetting::AppealChannel => (false, true, false),
        ChannelSetting::EvidenceChannel => (false, false, true),
        _ => {
            return Err(DbError::Invalid(format!(
                "{} is a module setting",
                setting.as_str()
            )));
        }
    };
    let c = i64::from(channel);
    let done = obs::run(
        Update,
        "guild_settings",
        sqlx::query!(
            "UPDATE guild_settings SET
               log_channel_id      = CASE WHEN $3 AND log_channel_id = $2 THEN NULL ELSE log_channel_id END,
               appeal_channel_id   = CASE WHEN $4 AND appeal_channel_id = $2 THEN NULL ELSE appeal_channel_id END,
               evidence_channel_id = CASE WHEN $5 AND evidence_channel_id = $2 THEN NULL ELSE evidence_channel_id END,
               version = version + 1, updated_at = now(), updated_by = NULL
             WHERE guild_id = $1
               AND (($3 AND log_channel_id = $2) OR ($4 AND appeal_channel_id = $2) OR ($5 AND evidence_channel_id = $2))",
            i64::from(guild),
            c,
            log,
            appeal,
            evidence,
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

mod modules;

pub use modules::{disable_modules_on_leave, set_module_enabled};

/// Records that the log channel accepted a post (§9.4): sets
/// `log_channel_posted_at` in the `post_mod_log` job's settle transaction.
/// Not part of `version` and sends no notification. Only while `channel`
/// is still the log channel: a post in a replaced channel says nothing
/// about the new one's outage.
pub async fn mark_log_channel_posted(
    conn: &mut PgConnection,
    guild: GuildId,
    channel: ChannelId,
) -> Result<(), DbError> {
    obs::run(
        Update,
        "guild_settings",
        sqlx::query!(
            "UPDATE guild_settings SET log_channel_posted_at = now()
             WHERE guild_id = $1 AND log_channel_id = $2",
            i64::from(guild),
            i64::from(channel)
        )
        .execute(conn),
    )
    .await?;
    Ok(())
}

/// When the log channel last accepted a post; `None` before the first
/// (and for imported servers).
pub async fn log_channel_posted_at(
    conn: &mut PgConnection,
    guild: GuildId,
) -> Result<Option<OffsetDateTime>, DbError> {
    let at = obs::run(
        Select,
        "guild_settings",
        sqlx::query_scalar!(
            "SELECT log_channel_posted_at FROM guild_settings WHERE guild_id = $1",
            i64::from(guild)
        )
        .fetch_optional(conn),
    )
    .await?;
    Ok(at.flatten())
}
