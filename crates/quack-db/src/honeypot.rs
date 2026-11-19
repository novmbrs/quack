//! The honeypot module (§13.4): settings and hits.

use quack_core::ids::{AccountId, CaseId, ChannelId, GuildId, MemberId, MessageId, RuleId};
use quack_core::modules::{HoneypotHitState, ModuleDisabledReason};
use sqlx::PgConnection;
use time::OffsetDateTime;

use crate::DbError;
use crate::obs::{self, Insert, Select, Update};
use crate::pg::{Pg, count, id, int, opt_id};

/// `honeypot_settings`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoneypotSettings {
    /// Whether the honeypot is on.
    pub enabled: bool,
    /// Why Quack turned it off.
    pub disabled_reason: Option<ModuleDisabledReason>,
    /// The trap channel.
    pub channel: Option<ChannelId>,
    /// The rule its cases use.
    pub rule: Option<RuleId>,
    /// Custom warning text.
    pub warning_text: Option<String>,
    /// Incremented by every change; stamped on every hit.
    pub version: u64,
}

struct SettingsRow {
    enabled: bool,
    disabled_reason: Option<Pg<ModuleDisabledReason>>,
    channel_id: Option<i64>,
    rule_id: Option<uuid::Uuid>,
    warning_text: Option<String>,
    version: i64,
}

impl TryFrom<SettingsRow> for HoneypotSettings {
    type Error = DbError;

    fn try_from(r: SettingsRow) -> Result<Self, DbError> {
        Ok(Self {
            enabled: r.enabled,
            disabled_reason: r.disabled_reason.map(|d| d.0),
            channel: opt_id(r.channel_id)?,
            rule: r.rule_id.map(RuleId::from_uuid),
            warning_text: r.warning_text,
            version: count(r.version)?,
        })
    }
}

/// A server's honeypot settings.
pub async fn settings(
    conn: &mut PgConnection,
    guild: GuildId,
) -> Result<Option<HoneypotSettings>, DbError> {
    let row = obs::run(
        Select,
        "honeypot_settings",
        sqlx::query_as!(
            SettingsRow,
            "SELECT enabled, disabled_reason, channel_id, rule_id, warning_text, version
             FROM honeypot_settings WHERE guild_id = $1",
            i64::from(guild)
        )
        .fetch_optional(conn),
    )
    .await?;
    row.map(HoneypotSettings::try_from).transpose()
}

/// Locks the honeypot settings: `FOR SHARE` (case creation, rule edits)
/// or `FOR UPDATE` (settings writes), §4.12 step 3.
pub async fn lock_settings(
    conn: &mut PgConnection,
    guild: GuildId,
    exclusive: bool,
) -> Result<Option<HoneypotSettings>, DbError> {
    let g = i64::from(guild);
    let row = if exclusive {
        obs::run(
            Select,
            "honeypot_settings",
            sqlx::query_as!(
                SettingsRow,
                "SELECT enabled, disabled_reason, channel_id, rule_id, warning_text, version
                 FROM honeypot_settings WHERE guild_id = $1 FOR UPDATE",
                g
            )
            .fetch_optional(conn),
        )
        .await?
    } else {
        obs::run(
            Select,
            "honeypot_settings",
            sqlx::query_as!(
                SettingsRow,
                "SELECT enabled, disabled_reason, channel_id, rule_id, warning_text, version
                 FROM honeypot_settings WHERE guild_id = $1 FOR SHARE",
                g
            )
            .fetch_optional(conn),
        )
        .await?
    };
    row.map(HoneypotSettings::try_from).transpose()
}

/// Writes the honeypot settings (validated by the caller) if still at
/// `expected_version`. Returns the new version.
pub async fn update_settings(
    conn: &mut PgConnection,
    guild: GuildId,
    channel: Option<ChannelId>,
    rule: Option<RuleId>,
    warning_text: Option<&str>,
    expected_version: u64,
    by: AccountId,
) -> Result<Option<u64>, DbError> {
    let version = obs::run(
        Update,
        "honeypot_settings",
        sqlx::query_scalar!(
            "UPDATE honeypot_settings SET channel_id = $3, rule_id = $4, warning_text = $5, version = version + 1,
                                          updated_at = now(), updated_by = $6
             WHERE guild_id = $1 AND version = $2 RETURNING version",
            i64::from(guild),
            int::<u64, i64>(expected_version)?,
            channel.map(i64::from),
            rule.map(RuleId::get),
            warning_text,
            i64::from(by),
        )
        .fetch_optional(conn),
    )
    .await?;
    version.map(count).transpose()
}

/// Channel reconciliation (§13.1.5): clears the honeypot channel if it still
/// holds `channel` and turns the honeypot off. Returns `(cleared, turned_off)`.
pub async fn clear_channel(
    conn: &mut PgConnection,
    guild: GuildId,
    channel: ChannelId,
) -> Result<(bool, bool), DbError> {
    let row = obs::run(
        Update,
        "honeypot_settings",
        sqlx::query!(
            r#"UPDATE honeypot_settings SET channel_id = NULL,
                 disabled_reason = CASE WHEN enabled THEN 'channel_deleted' ELSE disabled_reason END,
                 enabled = false, version = version + 1, updated_at = now(), updated_by = NULL
               WHERE guild_id = $1 AND channel_id = $2
               RETURNING old.enabled AS "was_enabled!""#,
            i64::from(guild),
            i64::from(channel),
        )
        .fetch_optional(conn),
    )
    .await?;
    Ok(row.map_or((false, false), |r| (true, r.was_enabled)))
}

/// One post in the honeypot channel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    /// The post.
    pub message: MessageId,
    /// Its channel.
    pub channel: ChannelId,
    /// Who posted.
    pub member: MemberId,
    /// The honeypot settings version when admitted.
    pub settings_version: u64,
    /// The honeypot rule when admitted.
    pub rule: Option<RuleId>,
    /// Its state.
    pub state: HoneypotHitState,
    /// The case it joined.
    pub case: Option<CaseId>,
    /// Why it failed (a refusal code, `exempt`, `policy_changed`, …).
    pub failure: Option<String>,
    /// When it was admitted.
    pub created_at: OffsetDateTime,
}

/// Admits a hit (`pending`) once per message. Returns whether it is new.
pub async fn insert_hit(
    conn: &mut PgConnection,
    guild: GuildId,
    channel: ChannelId,
    message: MessageId,
    member: MemberId,
    settings_version: u64,
    rule: Option<RuleId>,
) -> Result<bool, DbError> {
    let done = obs::run(
        Insert,
        "honeypot_hits",
        sqlx::query!(
            "INSERT INTO honeypot_hits (guild_id, message_id, channel_id, member_id, settings_version, rule_id)
             VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING",
            i64::from(guild),
            i64::from(message),
            i64::from(channel),
            i64::from(member),
            int::<u64, i64>(settings_version)?,
            rule.map(RuleId::get),
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// The member's pending hits, by message ID: the batch (§13.4.2 step 3.1).
pub async fn pending_batch(
    conn: &mut PgConnection,
    guild: GuildId,
    member: MemberId,
) -> Result<Vec<Hit>, DbError> {
    let rows = obs::run(
        Select,
        "honeypot_hits",
        sqlx::query!(
            "SELECT message_id, channel_id, member_id, settings_version, rule_id, state, case_id, failure, created_at
             FROM honeypot_hits WHERE guild_id = $1 AND member_id = $2 AND state = 'pending' ORDER BY message_id",
            i64::from(guild),
            i64::from(member),
        )
        .fetch_all(conn),
    )
    .await?;
    rows.into_iter()
        .map(|r| {
            Ok(Hit {
                message: id(r.message_id)?,
                channel: id(r.channel_id)?,
                member: id(r.member_id)?,
                settings_version: count(r.settings_version)?,
                rule: r.rule_id.map(RuleId::from_uuid),
                state: r.state.0,
                case: r.case_id.map(CaseId::from_uuid),
                failure: r.failure,
                created_at: r.created_at,
            })
        })
        .collect()
}

/// One hit of this server, by its post (the cleanup job's lookup).
pub async fn hit(
    conn: &mut PgConnection,
    guild: GuildId,
    message: MessageId,
) -> Result<Option<Hit>, DbError> {
    let row = obs::run(
        Select,
        "honeypot_hits",
        sqlx::query!(
            "SELECT message_id, channel_id, member_id, settings_version, rule_id, state, case_id, failure, created_at
             FROM honeypot_hits WHERE guild_id = $1 AND message_id = $2",
            i64::from(guild),
            i64::from(message),
        )
        .fetch_optional(conn),
    )
    .await?;
    row.map(|r| {
        Ok(Hit {
            message: id(r.message_id)?,
            channel: id(r.channel_id)?,
            member: id(r.member_id)?,
            settings_version: count(r.settings_version)?,
            rule: r.rule_id.map(RuleId::from_uuid),
            state: r.state.0,
            case: r.case_id.map(CaseId::from_uuid),
            failure: r.failure,
            created_at: r.created_at,
        })
    })
    .transpose()
}

/// Members with pending hits, across servers, at most `limit`: the
/// `honeypot_case` jobs to make sure of after a restart.
pub async fn members_with_pending_hits(
    conn: &mut PgConnection,
    limit: u32,
) -> Result<Vec<(GuildId, MemberId)>, DbError> {
    let rows = obs::run(
        Select,
        "honeypot_hits",
        sqlx::query!(
            "SELECT DISTINCT guild_id, member_id FROM honeypot_hits WHERE state = 'pending'
             ORDER BY guild_id, member_id LIMIT $1",
            i64::from(limit),
        )
        .fetch_all(conn),
    )
    .await?;
    rows.into_iter()
        .map(|r| Ok((id(r.guild_id)?, id(r.member_id)?)))
        .collect()
}

/// How long after a case's first hit the member's later hits still join
/// that case (§13.4.2 step 3.3). Measured from the first hit, so a slow
/// drip of posts can't stretch one burst.
pub const BURST_WINDOW: std::time::Duration = std::time::Duration::from_secs(30);

/// A burst (§13.4.2 step 3.3): a valid case created from this member's hits
/// under the same settings version whose first hit is less than
/// [`BURST_WINDOW`] older than `first_hit_at`.
pub async fn burst_case(
    conn: &mut PgConnection,
    guild: GuildId,
    member: MemberId,
    settings_version: u64,
    first_hit_at: OffsetDateTime,
) -> Result<Option<CaseId>, DbError> {
    let case = obs::run(
        Select,
        "honeypot_hits",
        sqlx::query_scalar!(
            r#"SELECT h.case_id AS "case_id!" FROM honeypot_hits h
               JOIN cases c ON c.guild_id = h.guild_id AND c.id = h.case_id
               WHERE h.guild_id = $1 AND h.member_id = $2 AND h.state = 'case_created'
                 AND h.settings_version = $3 AND c.voided_at IS NULL
               GROUP BY h.case_id
               HAVING min(h.created_at) > $4::timestamptz - make_interval(secs => $5)
               ORDER BY min(h.created_at) DESC LIMIT 1"#,
            i64::from(guild),
            i64::from(member),
            int::<u64, i64>(settings_version)?,
            first_hit_at,
            BURST_WINDOW.as_secs_f64(),
        )
        .fetch_optional(conn),
    )
    .await?;
    Ok(case.map(CaseId::from_uuid))
}

/// `pending` → `case_created` for these hits. Returns how many changed.
pub async fn mark_case_created(
    conn: &mut PgConnection,
    guild: GuildId,
    messages: &[MessageId],
    case: CaseId,
) -> Result<u64, DbError> {
    let ids: Vec<i64> = messages.iter().copied().map(i64::from).collect();
    let done = obs::run(
        Update,
        "honeypot_hits",
        sqlx::query!(
            "UPDATE honeypot_hits SET state = 'case_created', case_id = $3, updated_at = now()
             WHERE guild_id = $1 AND message_id = ANY($2) AND state = 'pending'",
            i64::from(guild),
            &ids,
            case.get(),
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected())
}

/// `pending` → `failed` with a code (at most 64 characters). Returns how
/// many changed.
pub async fn mark_failed(
    conn: &mut PgConnection,
    guild: GuildId,
    messages: &[MessageId],
    failure: &str,
) -> Result<u64, DbError> {
    let ids: Vec<i64> = messages.iter().copied().map(i64::from).collect();
    let failure: String = failure.chars().take(64).collect();
    let done = obs::run(
        Update,
        "honeypot_hits",
        sqlx::query!(
            "UPDATE honeypot_hits SET state = 'failed', failure = $3, updated_at = now()
             WHERE guild_id = $1 AND message_id = ANY($2) AND state = 'pending'",
            i64::from(guild),
            &ids,
            failure,
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected())
}

/// Records the cleanup of a post: deleted, or given up (`failed`).
pub async fn mark_cleanup(
    conn: &mut PgConnection,
    guild: GuildId,
    message: MessageId,
    failed: bool,
) -> Result<bool, DbError> {
    let done = obs::run(
        Update,
        "honeypot_hits",
        sqlx::query!(
            "UPDATE honeypot_hits SET cleaned_up_at = CASE WHEN $3 THEN NULL ELSE now() END,
                                      cleanup_failed_at = CASE WHEN $3 THEN now() ELSE NULL END,
                                      updated_at = now()
             WHERE guild_id = $1 AND message_id = $2 AND cleaned_up_at IS NULL AND cleanup_failed_at IS NULL",
            i64::from(guild),
            i64::from(message),
            failed,
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Pending and failed hits in a server (the module page).
pub async fn hit_counts(conn: &mut PgConnection, guild: GuildId) -> Result<(u64, u64), DbError> {
    let row = obs::run(
        Select,
        "honeypot_hits",
        sqlx::query!(
            r#"SELECT count(*) FILTER (WHERE state = 'pending') AS "pending!",
                      count(*) FILTER (WHERE state = 'failed') AS "failed!"
               FROM honeypot_hits WHERE guild_id = $1"#,
            i64::from(guild)
        )
        .fetch_one(conn),
    )
    .await?;
    Ok((count(row.pending)?, count(row.failed)?))
}
