//! Interaction payloads shaped like Discord's documented objects
//! (`INTERACTION_CREATE`'s `d`), for the standard test server.

use std::sync::atomic::{AtomicU64, Ordering};

use quack_core::ids::{ActionId, AppealId, CaseId, ChannelId, RuleId};
use quack_core::permissions::{Permission, Permissions};
use quack_testkit::{ADMIN, GUILD, MEMBER, MODERATOR, OWNER};
use serde_json::{Value, json};

use super::{ADMIN_ROLE, MEMBER_ROLE, MOD_ROLE};

/// Discord's epoch (2015-01-01) in Unix milliseconds.
const DISCORD_EPOCH_MS: u64 = 1_420_070_400_000;

static SEQ: AtomicU64 = AtomicU64::new(1);

/// A fresh interaction id made now (its timestamp is the creation time).
pub fn snowflake() -> u64 {
    let now_ms =
        u64::try_from(time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000).unwrap();
    ((now_ms - DISCORD_EPOCH_MS) << 22) | (SEQ.fetch_add(1, Ordering::Relaxed) & 0x3F_FFFF)
}

/// An interaction id made 20 minutes ago (its token has expired).
pub fn old_snowflake() -> u64 {
    snowflake() - ((20 * 60 * 1_000) << 22)
}

/// Who clicks or runs a command.
#[derive(Clone, Copy, Debug)]
pub struct Who {
    /// Their account.
    pub id: u64,
    /// Their role.
    pub role: Option<u64>,
    /// Their permissions in the channel.
    pub permissions: u64,
}

const fn bits(p: Permission) -> u64 {
    Permissions(p.bit()).0
}

/// The standard moderator (role with Moderate Members).
pub const fn moderator() -> Who {
    Who {
        id: MODERATOR,
        role: Some(MOD_ROLE),
        permissions: bits(Permission::ModerateMembers),
    }
}

/// The administrator.
pub const fn admin() -> Who {
    Who {
        id: ADMIN,
        role: Some(ADMIN_ROLE),
        permissions: bits(Permission::Administrator),
    }
}

/// An ordinary member (no staff permissions).
pub const fn member() -> Who {
    Who {
        id: MEMBER,
        role: Some(MEMBER_ROLE),
        permissions: 0,
    }
}

/// The owner (no roles; Discord gives them every permission).
pub const fn owner() -> Who {
    Who {
        id: OWNER,
        role: None,
        permissions: bits(Permission::Administrator),
    }
}

/// A user object.
pub fn user(id: u64) -> Value {
    json!({
        "id": id.to_string(),
        "username": format!("user{id}"),
        "discriminator": "0",
        "global_name": null,
        "avatar": null,
        "bot": false
    })
}

/// An interaction payload of `kind` (2 command, 3 component, 4
/// autocomplete, 5 modal submit) in the standard server's `channel`.
pub fn interaction(kind: u8, who: Who, channel: ChannelId, data: &Value) -> Value {
    interaction_with(kind, who, channel, data, snowflake())
}

/// The same, with a chosen interaction id.
pub fn interaction_with(kind: u8, who: Who, channel: ChannelId, data: &Value, id: u64) -> Value {
    json!({
        "id": id.to_string(),
        "application_id": "42",
        "type": kind,
        "token": format!("token-{id}"),
        "version": 1,
        "guild_id": GUILD.to_string(),
        "channel": {"id": channel.to_string(), "type": 0},
        "channel_id": channel.to_string(),
        "member": {
            "user": user(who.id),
            "roles": who.role.map_or_else(Vec::new, |r| vec![r.to_string()]),
            "joined_at": "2024-01-02T03:04:05.000000+00:00",
            "deaf": false,
            "mute": false,
            "flags": 0,
            "pending": false,
            "permissions": who.permissions.to_string(),
            "nick": null,
            "communication_disabled_until": null
        },
        "app_permissions": "8",
        "locale": "en-US",
        "guild_locale": "en-US",
        "entitlements": [],
        "authorizing_integration_owners": {"0": GUILD.to_string()},
        "context": 0,
        "attachment_size_limit": 10_485_760,
        "data": data
    })
}

/// A payload from a DM (no server, `user` instead of `member`).
pub fn dm_interaction(kind: u8, account: u64, data: &Value) -> Value {
    let id = snowflake();
    json!({
        "id": id.to_string(),
        "application_id": "42",
        "type": kind,
        "token": format!("token-{id}"),
        "version": 1,
        "channel": {"id": "777", "type": 1},
        "channel_id": "777",
        "user": user(account),
        "locale": "en-US",
        "entitlements": [],
        "authorizing_integration_owners": {"1": account.to_string()},
        "context": 1,
        "attachment_size_limit": 10_485_760,
        "data": data
    })
}

fn resolved_member(role: u64) -> Value {
    json!({
        "roles": [role.to_string()],
        "joined_at": "2024-01-02T03:04:05.000000+00:00",
        "permissions": "0",
        "nick": null,
        "flags": 0,
        "pending": false,
        "communication_disabled_until": null
    })
}

/// `/case <sub>` with options; `member` is resolved when given.
pub fn case(sub: &str, options: &Value, member: Option<u64>) -> Value {
    let mut data = json!({
        "id": "100",
        "name": "case",
        "type": 1,
        "options": [{"name": sub, "type": 1, "options": options}]
    });
    if let Some(m) = member {
        data["resolved"] = json!({
            "users": {m.to_string(): user(m)},
            "members": {m.to_string(): resolved_member(MEMBER_ROLE)}
        });
    }
    data
}

/// `/case add member rule [private]`.
pub fn case_add(member: u64, rule: RuleId, private: Option<bool>) -> Value {
    let mut options = vec![
        json!({"name": "member", "type": 6, "value": member.to_string()}),
        json!({"name": "rule", "type": 3, "value": rule.get().to_string()}),
    ];
    if let Some(p) = private {
        options.push(json!({"name": "private", "type": 5, "value": p}));
    }
    case("add", &json!(options), Some(member))
}

/// Autocomplete for `/case <sub>`'s rule option.
pub fn rule_autocomplete(sub: &str, typed: &str) -> Value {
    case(
        sub,
        &json!([{"name": "rule", "type": 3, "value": typed, "focused": true}]),
        None,
    )
}

/// A top-level slash command with options.
pub fn slash(name: &str, options: &Value) -> Value {
    json!({"id": "101", "name": name, "type": 1, "options": options})
}

/// The "Add case" member command on `member`.
pub fn add_case_for(member: u64) -> Value {
    json!({
        "id": "102",
        "name": "Add case",
        "type": 2,
        "target_id": member.to_string(),
        "resolved": {
            "users": {member.to_string(): user(member)},
            "members": {member.to_string(): resolved_member(MEMBER_ROLE)}
        }
    })
}

/// The "Add case" message command on a message by `author`.
pub fn add_case_on(message: u64, channel: ChannelId, author: u64) -> Value {
    json!({
        "id": "103",
        "name": "Add case",
        "type": 3,
        "target_id": message.to_string(),
        "resolved": {
            "messages": {message.to_string(): {
                "id": message.to_string(),
                "channel_id": channel.to_string(),
                "author": user(author),
                "content": "spam spam spam",
                "timestamp": "2026-11-17T12:00:00.000000+00:00",
                "edited_timestamp": null,
                "tts": false,
                "mention_everyone": false,
                "mentions": [],
                "mention_roles": [],
                "attachments": [],
                "embeds": [],
                "pinned": false,
                "type": 0,
                "flags": 0
            }}
        }
    })
}

/// A button (`component_type` 2) or select (3, 8) with this custom id.
pub fn component(custom_id: &str, component_type: u8, values: &[String]) -> Value {
    json!({"custom_id": custom_id, "component_type": component_type, "values": values})
}

/// The message a component is on (`flags` 64 when ephemeral).
pub fn on_message(mut payload: Value, channel: ChannelId, ephemeral: bool) -> Value {
    payload["message"] = json!({
        "id": snowflake().to_string(),
        "channel_id": channel.to_string(),
        "author": user(20),
        "content": "",
        "timestamp": "2026-11-17T12:00:00.000000+00:00",
        "edited_timestamp": null,
        "tts": false,
        "mention_everyone": false,
        "mentions": [],
        "mention_roles": [],
        "attachments": [],
        "embeds": [],
        "pinned": false,
        "type": 0,
        "flags": if ephemeral { 64 } else { 0 }
    });
    payload
}

/// A modal submit with text inputs `(custom id, value)`.
pub fn modal(custom_id: &str, inputs: &[(&str, &str)]) -> Value {
    let components: Vec<Value> = inputs
        .iter()
        .enumerate()
        .map(|(i, (id, value))| {
            json!({"type": 18, "id": i * 2 + 1, "component":
                {"type": 4, "id": i * 2 + 2, "custom_id": id, "value": value}})
        })
        .collect();
    json!({"custom_id": custom_id, "components": components})
}

/// Custom ids, as `quack-discord` encodes them.
pub mod ids {
    use super::{ActionId, AppealId, CaseId, RuleId};

    /// `1:cv:<case>`.
    pub fn open_case(case: CaseId) -> String {
        format!("1:cv:{}", case.to_u22())
    }
    /// `1:vd:<case>`.
    pub fn void(case: CaseId) -> String {
        format!("1:vd:{}", case.to_u22())
    }
    /// `1:vdm:<case>`.
    pub fn void_modal(case: CaseId) -> String {
        format!("1:vdm:{}", case.to_u22())
    }
    /// `1:ar:<action>`.
    pub fn retry(action: ActionId) -> String {
        format!("1:ar:{}", action.to_u22())
    }
    /// `1:ad:<action>`.
    pub fn dismiss(action: ActionId) -> String {
        format!("1:ad:{}", action.to_u22())
    }
    /// `1:au:<action>`.
    pub fn undo(action: ActionId) -> String {
        format!("1:au:{}", action.to_u22())
    }
    /// `1:auc:<action>`.
    pub fn confirm_undo(action: ActionId) -> String {
        format!("1:auc:{}", action.to_u22())
    }
    /// `1:ao:<appeal>`.
    pub fn open_appeal(appeal: AppealId) -> String {
        format!("1:ao:{}", appeal.to_u22())
    }
    /// `1:rp:<draft>`.
    pub fn rule_picker(draft: CaseId) -> String {
        format!("1:rp:{}", draft.to_u22())
    }
    /// A rule as a select value.
    pub fn rule_value(rule: RuleId) -> String {
        rule.to_u22()
    }
}
