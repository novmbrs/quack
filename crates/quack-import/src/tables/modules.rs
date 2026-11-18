//! `module_configurations` → `ticket_settings`, `activity_log_settings` +
//! `activity_log_routes`, `honeypot_settings`, and the ticket panel and
//! honeypot warning tracked messages (§19.4). A module with no row gets its
//! default (disabled) settings.

use serde_json::Value;
use time::OffsetDateTime;

use crate::ImportError;
use crate::convert::{self, Snowflake};
use crate::guild::Ctx;
use crate::source::Cols;
use crate::tables::publications::guild_row_id;
use crate::target::{self, NewMessage, strict};

/// Go's default honeypot warning; imported as "Quack words it" (NULL).
pub(crate) const LEGACY_HONEYPOT_WARNING: &str = "# Warning!\nThis channel catches spam and scam accounts. Do not post here. Posting here triggers this server's honeypot moderation rule.";

/// Go logging event keys → `activity_event`.
pub(crate) fn activity_event(key: &str) -> Option<&'static str> {
    Some(match key {
        "message_edit" => "message_edited",
        "message_delete" => "message_deleted",
        "message_bulk_delete" => "messages_bulk_deleted",
        "member_join" => "member_joined",
        "member_leave" => "member_left",
        "discord_ban" => "member_banned",
        "discord_unban" => "member_unbanned",
        "guild_change" => "server_changed",
        "channel_change" => "channel_changed",
        _ => return None,
    })
}

struct Module {
    enabled: bool,
    config: Value,
    updated_at: OffsetDateTime,
}

/// Imports the three modules' settings.
pub(crate) async fn import(cx: &mut Ctx<'_>) -> Result<(), ImportError> {
    let rows = cx
        .src
        .rows(
            "SELECT * FROM module_configurations WHERE guild_id = ? ORDER BY id",
            &[&cx.g.ulid.clone()],
        )
        .await?;
    let (mut tickets, mut logging, mut honeypot) = (None, None, None);
    for row in &rows {
        let c = Cols::new(row, "module_configurations", "id");
        let module = Module {
            enabled: c.flag("enabled")?,
            config: c.json("config_json")?,
            updated_at: c.time("updated_at")?,
        };
        let slot = match c.text("module_id")?.as_str() {
            "tickets" => &mut tickets,
            "general_logging" => &mut logging,
            "honeypots" => &mut honeypot,
            _ => {
                cx.f.skip("module_configurations", "unknown_module", cx.key(c.key()));
                continue;
            }
        };
        *slot = Some(module);
    }
    ticket_settings(cx, tickets).await?;
    activity_settings(cx, logging).await?;
    honeypot_settings(cx, honeypot).await
}

/// A snowflake from a config key; an invalid one is reported and dropped.
fn config_id(cx: &mut Ctx<'_>, config: &Value, key: &str, table: &str) -> Option<i64> {
    let raw = config.get(key).and_then(Value::as_str);
    match convert::snowflake(raw) {
        Snowflake::Id(id) => Some(id),
        Snowflake::None => None,
        Snowflake::Invalid => {
            cx.f.skip(table, "invalid_snowflake", cx.key(key));
            None
        }
    }
}

async fn ticket_settings(cx: &mut Ctx<'_>, m: Option<Module>) -> Result<(), ImportError> {
    let key = cx.key("tickets");
    let Some(m) = m else {
        let q = sqlx::query("INSERT INTO ticket_settings (guild_id) VALUES ($1)").bind(cx.g.id);
        return strict(&mut cx.tx, &mut cx.f, "ticket_settings", &key, q).await;
    };
    let entry = config_id(cx, &m.config, "entry_channel_discord_id", "ticket_settings");
    let queue = config_id(cx, &m.config, "queue_channel_discord_id", "ticket_settings");
    let days = m
        .config
        .get("transcript_retention_days")
        .and_then(Value::as_i64)
        .unwrap_or(90);
    let retention = if (1..=365).contains(&days) {
        days
    } else {
        cx.f.normalized("ticket_settings.transcript_retention_days", &key);
        days.clamp(1, 365)
    };
    let mut enabled = m.enabled;
    if enabled && (entry.is_none() || queue.is_none() || entry == queue) {
        cx.f.derived("enabled_module_disabled", &key);
        enabled = false;
    }
    cx.ticket_entry_channel = entry;
    let q = sqlx::query(
        "INSERT INTO ticket_settings (guild_id, enabled, entry_channel_id, queue_channel_id, transcript_retention_days, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(cx.g.id)
    .bind(enabled)
    .bind(entry)
    .bind(queue)
    .bind(i16::try_from(retention).unwrap_or(90))
    .bind(m.updated_at);
    strict(&mut cx.tx, &mut cx.f, "ticket_settings", &key, q).await?;
    let panel_channel =
        config_id(cx, &m.config, "entry_panel_channel_id", "ticket_settings").or(entry);
    let panel_message = config_id(cx, &m.config, "entry_panel_message_id", "ticket_settings");
    if let (Some(channel), Some(message)) = (panel_channel, panel_message) {
        tracked(
            cx,
            "ticket_panel",
            1,
            channel,
            message,
            enabled,
            m.updated_at,
        )
        .await?;
    }
    Ok(())
}

/// A server-level tracked message: re-rendered (`2`/`1`) so the new
/// buttons replace the old ones, synced only while the module is on.
async fn tracked(
    cx: &mut Ctx<'_>,
    kind: &'static str,
    slot: u8,
    channel: i64,
    message: i64,
    enabled: bool,
    at: OffsetDateTime,
) -> Result<(), ImportError> {
    let key = cx.key(kind);
    let m = NewMessage {
        id: guild_row_id(cx.g.id, slot),
        kind,
        case_id: None,
        appeal_id: None,
        ticket_id: None,
        channel_id: Some(channel),
        message_id: Some(message),
        wanted: if enabled { 2 } else { 1 },
        posted: 1,
        created_at: at,
        updated_at: at,
    };
    if enabled {
        cx.f.in_flight(&format!("{kind}_rerendered"), &key);
    }
    target::message(&mut cx.tx, &mut cx.f, cx.g.id, &key, &m).await?;
    Ok(())
}

async fn activity_settings(cx: &mut Ctx<'_>, m: Option<Module>) -> Result<(), ImportError> {
    let key = cx.key("activity_log");
    let Some(m) = m else {
        let q =
            sqlx::query("INSERT INTO activity_log_settings (guild_id) VALUES ($1)").bind(cx.g.id);
        return strict(&mut cx.tx, &mut cx.f, "activity_log_settings", &key, q).await;
    };
    let mut routes = Vec::new();
    if let Some(channels) = m.config.get("channels").and_then(Value::as_object) {
        for (event, channel) in channels {
            let id = convert::snowflake(channel.as_str());
            match (activity_event(event), id) {
                (Some(e), Snowflake::Id(id)) => routes.push((e, id)),
                (_, Snowflake::None) => {}
                _ => cx.f.skip(
                    "activity_log_routes",
                    "invalid_route",
                    format!("{key}:{event}"),
                ),
            }
        }
    }
    let flag = |k: &str| m.config.get(k).and_then(Value::as_bool).unwrap_or(true);
    let mut enabled = m.enabled;
    if enabled && routes.is_empty() {
        cx.f.derived("enabled_module_disabled", &key);
        enabled = false;
    }
    let q = sqlx::query(
        "INSERT INTO activity_log_settings (guild_id, enabled, include_message_content, include_file_names,
                                            include_embed_types, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(cx.g.id)
    .bind(enabled)
    .bind(flag("include_message_content"))
    .bind(flag("include_attachment_metadata"))
    .bind(flag("include_embed_metadata"))
    .bind(m.updated_at);
    strict(&mut cx.tx, &mut cx.f, "activity_log_settings", &key, q).await?;
    for (event, channel) in routes {
        let q = sqlx::query("INSERT INTO activity_log_routes (guild_id, event, channel_id) VALUES ($1, $2::activity_event, $3)")
            .bind(cx.g.id)
            .bind(event)
            .bind(channel);
        strict(
            &mut cx.tx,
            &mut cx.f,
            "activity_log_routes",
            &format!("{key}:{event}"),
            q,
        )
        .await?;
    }
    Ok(())
}

async fn honeypot_settings(cx: &mut Ctx<'_>, m: Option<Module>) -> Result<(), ImportError> {
    let key = cx.key("honeypot");
    let Some(m) = m else {
        let q = sqlx::query("INSERT INTO honeypot_settings (guild_id) VALUES ($1)").bind(cx.g.id);
        return strict(&mut cx.tx, &mut cx.f, "honeypot_settings", &key, q).await;
    };
    let channel = config_id(cx, &m.config, "channel_discord_id", "honeypot_settings");
    let rule = m
        .config
        .get("template_id")
        .and_then(Value::as_str)
        .and_then(convert::uuid)
        .and_then(|t| cx.rule_ref.get(&t).copied());
    if m.config
        .get("template_id")
        .and_then(Value::as_str)
        .is_some_and(|t| !t.is_empty())
        && rule.is_none()
    {
        cx.f.skip(
            "honeypot_settings",
            "missing_parent",
            format!("{key}:rule_id"),
        );
    }
    let warning = m
        .config
        .get("warning_text")
        .and_then(Value::as_str)
        .filter(|t| !t.trim().is_empty() && *t != LEGACY_HONEYPOT_WARNING)
        .map(|t| convert::clean(t.to_owned()).0);
    if warning
        .as_deref()
        .is_some_and(|w| convert::chars(w) > 1_500)
    {
        cx.f.over_limit("honeypot_settings.warning_text", &key);
    }
    let disabled = m
        .config
        .get("disabled_reason")
        .and_then(Value::as_str)
        .filter(|r| !r.trim().is_empty());
    let reason = disabled
        .filter(|r| r.contains("channel was deleted"))
        .map(|_| "channel_deleted");
    let mut enabled = m.enabled && disabled.is_none();
    if enabled && (channel.is_none() || rule.is_none()) {
        cx.f.derived("enabled_module_disabled", &key);
        enabled = false;
    }
    let q = sqlx::query(
        "INSERT INTO honeypot_settings (guild_id, enabled, disabled_reason, channel_id, rule_id, warning_text, version, updated_at)
         VALUES ($1, $2, $3::module_disabled_reason, $4, $5, $6, 1, $7)",
    )
    .bind(cx.g.id)
    .bind(enabled)
    .bind(reason)
    .bind(channel)
    .bind(rule)
    .bind(warning)
    .bind(m.updated_at);
    strict(&mut cx.tx, &mut cx.f, "honeypot_settings", &key, q).await?;
    let warning_message = config_id(cx, &m.config, "warning_message_id", "honeypot_settings");
    if let (Some(channel), Some(message)) = (channel, warning_message) {
        // Go posted the warning in the honeypot channel itself.
        tracked(
            cx,
            "honeypot_warning",
            2,
            channel,
            message,
            enabled,
            m.updated_at,
        )
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_go_logging_event_maps() {
        for key in [
            "message_edit",
            "message_delete",
            "message_bulk_delete",
            "member_join",
            "member_leave",
            "discord_ban",
            "discord_unban",
            "guild_change",
            "channel_change",
        ] {
            assert!(activity_event(key).is_some(), "{key}");
        }
        assert_eq!(activity_event("other"), None);
    }
}
