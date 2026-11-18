//! Discord-shaped JSON (REST answers, gateway dispatches, interactions)
//! per Discord's documented objects, shared by the tests.

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

pub(crate) fn guild(id: u64, owner: u64, roles: &[Value]) -> Value {
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

pub(crate) fn thread(id: u64, parent: u64, name: &str, archived_at: &str) -> Value {
    json!({
        "id": id.to_string(),
        "guild_id": "1",
        "type": 12,
        "name": name,
        "parent_id": parent.to_string(),
        "thread_metadata": {
            "archived": true,
            "auto_archive_duration": 1440,
            "archive_timestamp": archived_at,
            "locked": false
        }
    })
}

/// A gateway dispatch (`op` 0), parsed the way the shard parses it.
pub(crate) fn dispatch(kind: &str, d: &Value) -> twilight_model::gateway::event::Event {
    let json = json!({"op": 0, "s": 1, "t": kind, "d": d}).to_string();
    twilight_gateway::parse(json, crate::gateway::wanted())
        .expect("a readable dispatch")
        .expect("a wanted event")
        .into()
}

/// `READY` for Quack's account and these servers.
pub(crate) fn ready(quack: u64, guilds: &[u64]) -> Value {
    let mut me = user(quack, "Quack");
    me["bot"] = json!(true);
    me["verified"] = json!(true);
    me["mfa_enabled"] = json!(true);
    json!({
        "v": 10,
        "user": me,
        "guilds": guilds.iter().map(|g| json!({"id": g.to_string(), "unavailable": true})).collect::<Vec<_>>(),
        "session_id": "session",
        "resume_gateway_url": "wss://gateway.example",
        "application": {"id": "42", "flags": 0},
        "shard": [0, 1]
    })
}

/// `GUILD_CREATE` with channels and members (Quack's own member among
/// them, as Discord always sends it).
pub(crate) fn guild_create(
    id: u64,
    owner: u64,
    roles: &[Value],
    channels: &[Value],
    members: &[Value],
) -> Value {
    let mut g = guild(id, owner, roles);
    g["channels"] = json!(channels);
    g["members"] = json!(members);
    g["threads"] = json!([]);
    g["large"] = json!(false);
    g["member_count"] = json!(members.len());
    g["joined_at"] = json!("2024-01-02T03:04:05.000000+00:00");
    g
}

/// A server channel with these overwrites (`(id, type, allow, deny)`).
pub(crate) fn channel_with(
    id: u64,
    guild: u64,
    kind: u8,
    parent: Option<u64>,
    overwrites: &[(u64, u8, u64, u64)],
) -> Value {
    json!({
        "id": id.to_string(),
        "guild_id": guild.to_string(),
        "type": kind,
        "name": format!("channel-{id}"),
        "position": 0,
        "parent_id": parent.map(|p| p.to_string()),
        "permission_overwrites": overwrites
            .iter()
            .map(|(id, kind, allow, deny)| json!({
                "id": id.to_string(),
                "type": kind,
                "allow": allow.to_string(),
                "deny": deny.to_string()
            }))
            .collect::<Vec<_>>()
    })
}

/// An interaction of `kind` (2 command, 3 component, 4 autocomplete, 5
/// modal) in server 1, channel 5, by member 7 with role 20 and
/// Administrator in the channel.
pub(crate) fn interaction(kind: u8, data: &Value) -> Value {
    json!({
        "id": "1310000000000000000",
        "application_id": "42",
        "type": kind,
        "token": "interaction-token",
        "version": 1,
        "guild_id": "1",
        "channel": {"id": "5", "type": 0},
        "channel_id": "5",
        "member": {
            "user": user(7, "moderator"),
            "roles": ["20"],
            "joined_at": "2024-01-02T03:04:05.000000+00:00",
            "deaf": false,
            "mute": false,
            "flags": 0,
            "pending": false,
            "permissions": "8",
            "nick": "Mod",
            "communication_disabled_until": null
        },
        "app_permissions": "1024",
        "locale": "en-US",
        "guild_locale": "en-US",
        "entitlements": [],
        "authorizing_integration_owners": {"0": "1"},
        "context": 0,
        "attachment_size_limit": 10_485_760,
        "data": data
    })
}

/// A `resolved` member entry (no `user`, no `deaf`/`mute`).
pub(crate) fn resolved_member(roles: &[u64]) -> Value {
    json!({
        "roles": roles.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "joined_at": "2024-01-02T03:04:05.000000+00:00",
        "permissions": "0",
        "nick": null,
        "flags": 0,
        "pending": false,
        "communication_disabled_until": null
    })
}
