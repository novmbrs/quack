//! The server activity log module's settings and routes (§13.3).

use quack_core::ids::{AccountId, ChannelId, GuildId};
use quack_core::modules::{ActivityEvent, ModuleDisabledReason};
use sqlx::PgConnection;

use crate::DbError;
use crate::obs::{self, Delete, Insert, Select, Update};
use crate::pg::{Pg, count, id, int};

/// `activity_log_settings` plus its routes.
#[allow(
    clippy::struct_excessive_bools,
    reason = "the flags mirror the table's columns"
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivityLogSettings {
    /// Whether the module is on.
    pub enabled: bool,
    /// Why Quack turned it off.
    pub disabled_reason: Option<ModuleDisabledReason>,
    /// Post message text.
    pub include_message_content: bool,
    /// Post file names.
    pub include_file_names: bool,
    /// Post embed types.
    pub include_embed_types: bool,
    /// Incremented by every change.
    pub version: u64,
    /// One channel per routed event.
    pub routes: Vec<(ActivityEvent, ChannelId)>,
}

/// The privacy flags of the module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActivityFlags {
    /// Post message text.
    pub include_message_content: bool,
    /// Post file names.
    pub include_file_names: bool,
    /// Post embed types.
    pub include_embed_types: bool,
}

/// A server's activity log settings; `lock` takes the settings row
/// `FOR UPDATE` first.
pub async fn settings(
    conn: &mut PgConnection,
    guild: GuildId,
    lock: bool,
) -> Result<Option<ActivityLogSettings>, DbError> {
    let g = i64::from(guild);
    let row = if lock {
        obs::run(
            Select,
            "activity_log_settings",
            sqlx::query_as!(
                SettingsRow,
                "SELECT enabled, disabled_reason, include_message_content, include_file_names, include_embed_types,
                        version FROM activity_log_settings WHERE guild_id = $1 FOR UPDATE",
                g
            )
            .fetch_optional(&mut *conn),
        )
        .await?
    } else {
        obs::run(
            Select,
            "activity_log_settings",
            sqlx::query_as!(
                SettingsRow,
                "SELECT enabled, disabled_reason, include_message_content, include_file_names, include_embed_types,
                        version FROM activity_log_settings WHERE guild_id = $1",
                g
            )
            .fetch_optional(&mut *conn),
        )
        .await?
    };
    let Some(row) = row else {
        return Ok(None);
    };
    let routes = obs::run(
        Select,
        "activity_log_routes",
        sqlx::query!(
            "SELECT event, channel_id FROM activity_log_routes WHERE guild_id = $1 ORDER BY event",
            g
        )
        .fetch_all(conn),
    )
    .await?
    .into_iter()
    .map(|r| Ok((r.event.0, id(r.channel_id)?)))
    .collect::<Result<_, DbError>>()?;
    Ok(Some(ActivityLogSettings {
        enabled: row.enabled,
        disabled_reason: row.disabled_reason.map(|d| d.0),
        include_message_content: row.include_message_content,
        include_file_names: row.include_file_names,
        include_embed_types: row.include_embed_types,
        version: count(row.version)?,
        routes,
    }))
}

#[allow(
    clippy::struct_excessive_bools,
    reason = "the row mirrors the table's columns"
)]
struct SettingsRow {
    enabled: bool,
    disabled_reason: Option<Pg<ModuleDisabledReason>>,
    include_message_content: bool,
    include_file_names: bool,
    include_embed_types: bool,
    version: i64,
}

/// Replaces the flags and every route if the settings are still at
/// `expected_version`. Returns the new version.
pub async fn update_settings(
    conn: &mut PgConnection,
    guild: GuildId,
    flags: ActivityFlags,
    routes: &[(ActivityEvent, ChannelId)],
    expected_version: u64,
    by: AccountId,
) -> Result<Option<u64>, DbError> {
    let g = i64::from(guild);
    let version = obs::run(
        Update,
        "activity_log_settings",
        sqlx::query_scalar!(
            "UPDATE activity_log_settings SET include_message_content = $3, include_file_names = $4,
                                              include_embed_types = $5, version = version + 1, updated_at = now(),
                                              updated_by = $6
             WHERE guild_id = $1 AND version = $2 RETURNING version",
            g,
            int::<u64, i64>(expected_version)?,
            flags.include_message_content,
            flags.include_file_names,
            flags.include_embed_types,
            i64::from(by),
        )
        .fetch_optional(&mut *conn),
    )
    .await?;
    let Some(version) = version else {
        return Ok(None);
    };
    obs::run(
        Delete,
        "activity_log_routes",
        sqlx::query!("DELETE FROM activity_log_routes WHERE guild_id = $1", g).execute(&mut *conn),
    )
    .await?;
    let events: Vec<Pg<ActivityEvent>> = routes.iter().map(|(e, _)| Pg(*e)).collect();
    let channels: Vec<i64> = routes.iter().map(|(_, c)| i64::from(*c)).collect();
    obs::run(
        Insert,
        "activity_log_routes",
        sqlx::query!(
            "INSERT INTO activity_log_routes (guild_id, event, channel_id)
             SELECT $1, e, c FROM unnest($2::activity_event[], $3::bigint[]) AS r(e, c)",
            g,
            &events as &[Pg<ActivityEvent>],
            &channels,
        )
        .execute(conn),
    )
    .await?;
    Ok(Some(count(version)?))
}

/// Channel reconciliation (§13.1.5): removes the routes to `channel`; when
/// none remain the module is turned off (`channel_deleted`). Returns
/// `(routes removed, turned off)`.
pub async fn remove_channel(
    conn: &mut PgConnection,
    guild: GuildId,
    channel: ChannelId,
) -> Result<(u64, bool), DbError> {
    let g = i64::from(guild);
    let removed = obs::run(
        Delete,
        "activity_log_routes",
        sqlx::query!(
            "DELETE FROM activity_log_routes WHERE guild_id = $1 AND channel_id = $2",
            g,
            i64::from(channel)
        )
        .execute(&mut *conn),
    )
    .await?
    .rows_affected();
    if removed == 0 {
        return Ok((0, false));
    }
    let off = obs::run(
        Update,
        "activity_log_settings",
        sqlx::query!(
            "UPDATE activity_log_settings SET enabled = false, disabled_reason = 'channel_deleted',
                                              version = version + 1, updated_at = now(), updated_by = NULL
             WHERE guild_id = $1 AND enabled
               AND NOT EXISTS (SELECT 1 FROM activity_log_routes WHERE guild_id = $1)",
            g
        )
        .execute(conn),
    )
    .await?
    .rows_affected();
    Ok((removed, off == 1))
}
