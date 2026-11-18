//! Discord-shaped JSON answers, per Discord's documented objects.

use serde_json::{Value, json};

pub(crate) fn user(id: u64, name: &str) -> Value {
    json!({
        "id": id.to_string(),
        "username": name,
        "discriminator": "0",
        "global_name": null,
        "avatar": null,
        "bot": false
    })
}

pub(crate) fn member(id: u64, roles: &[u64]) -> Value {
    json!({
        "user": user(id, "member"),
        "nick": null,
        "roles": roles.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "joined_at": "2024-01-02T03:04:05.000000+00:00",
        "communication_disabled_until": null,
        "deaf": false,
        "mute": false,
        "flags": 0,
        "pending": false
    })
}

pub(crate) fn role(id: u64, position: i64, permissions: u64) -> Value {
    json!({
        "id": id.to_string(),
        "name": format!("role {id}"),
        "color": 0,
        "colors": {"primary_color": 0, "secondary_color": null, "tertiary_color": null},
        "hoist": false,
        "icon": null,
        "unicode_emoji": null,
        "position": position,
        "permissions": permissions.to_string(),
        "managed": false,
        "mentionable": false,
        "flags": 0
    })
}

pub(crate) fn guild(id: u64, owner: u64, roles: Vec<Value>) -> Value {
    json!({
        "id": id.to_string(),
        "name": "Server",
        "icon": null,
        "splash": null,
        "discovery_splash": null,
        "owner_id": owner.to_string(),
        "afk_channel_id": null,
        "afk_timeout": 300,
        "verification_level": 0,
        "default_message_notifications": 0,
        "explicit_content_filter": 0,
        "roles": roles,
        "emojis": [],
        "features": [],
        "mfa_level": 1,
        "application_id": null,
        "system_channel_id": null,
        "system_channel_flags": 0,
        "rules_channel_id": null,
        "vanity_url_code": null,
        "description": null,
        "banner": null,
        "premium_tier": 0,
        "preferred_locale": "en-US",
        "public_updates_channel_id": null,
        "nsfw_level": 0,
        "premium_progress_bar_enabled": false,
        "safety_alerts_channel_id": null
    })
}

pub(crate) fn channel(id: u64, guild: u64, kind: u8) -> Value {
    json!({
        "id": id.to_string(),
        "guild_id": guild.to_string(),
        "type": kind,
        "name": "staff",
        "position": 0,
        "parent_id": null,
        "permission_overwrites": [
            {"id": guild.to_string(), "type": 0, "allow": "0", "deny": "1024"},
            {"id": "9", "type": 1, "allow": "1024", "deny": "0"}
        ]
    })
}

pub(crate) fn message(id: u64, channel: u64) -> Value {
    json!({
        "id": id.to_string(),
        "channel_id": channel.to_string(),
        "author": user(9, "author"),
        "content": "hello",
        "timestamp": "2026-11-17T12:00:00.000000+00:00",
        "edited_timestamp": null,
        "tts": false,
        "mention_everyone": false,
        "mentions": [],
        "mention_roles": [],
        "attachments": [{
            "id": "77",
            "filename": "SPOILER_a.png",
            "size": 3,
            "url": "https://cdn.discordapp.com/attachments/5/77/SPOILER_a.png",
            "proxy_url": "https://media.discordapp.net/attachments/5/77/SPOILER_a.png",
            "content_type": "image/png"
        }],
        "embeds": [{"type": "rich", "title": "T", "description": "D"}],
        "pinned": false,
        "type": 0,
        "flags": 0
    })
}
