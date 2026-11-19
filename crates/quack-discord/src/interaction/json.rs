//! Decoding interaction JSON, leniently when Twilight's model is stricter
//! than Discord's payload.

use twilight_model::application::interaction::Interaction as RawInteraction;

use quack_core::ids::{ChannelId, GuildId};

use super::{Interaction, Request, UnknownReason, decode};
use crate::model::InteractionToken;

/// Decodes an `INTERACTION_CREATE` payload's JSON (the `d` object). A
/// payload Twilight can't read is read again with defaults for the fields
/// Twilight 0.18 requires but Quack doesn't use (`attachment_size_limit`,
/// `authorizing_integration_owners`, `entitlements`), so a Discord payload
/// change can't stop Quack from acknowledging. `None` only when even that
/// fails: nothing can be acknowledged without an id and token.
pub fn decode_json(json: &str) -> Option<Interaction> {
    if let Ok(raw) = serde_json::from_str::<RawInteraction>(json) {
        return Some(decode(&raw));
    }
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let mut filled = value.clone();
    let object = filled.as_object_mut()?;
    for (key, default) in [
        ("attachment_size_limit", serde_json::json!(0)),
        ("authorizing_integration_owners", serde_json::json!({})),
        ("entitlements", serde_json::json!([])),
    ] {
        object.entry(key).or_insert(default);
    }
    if let Ok(raw) = serde_json::from_value::<RawInteraction>(filled) {
        tracing::warn!(
            "read an interaction Twilight couldn't read, with defaults for missing fields"
        );
        return Some(decode(&raw));
    }
    tracing::warn!("couldn't read an interaction's data; answering it as unknown");
    envelope(&value)
}

/// The least an interaction can be answered with: its id, token, and type
/// (and its server and channel, for the answer's privacy), with the rest
/// [`UnknownReason::Malformed`].
fn envelope(value: &serde_json::Value) -> Option<Interaction> {
    let id = |key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .and_then(|s| s.parse::<u64>().ok())
    };
    let kind = value.get("type").and_then(serde_json::Value::as_u64)?;
    Some(Interaction {
        id: id("id")?,
        token: InteractionToken::new(value.get("token")?.as_str()?),
        guild_id: id("guild_id").and_then(GuildId::new),
        channel_id: id("channel_id").and_then(ChannelId::new),
        invoker: None,
        app_permissions: None,
        request: Request::Unknown(UnknownReason::Malformed {
            autocomplete: kind == 4,
        }),
    })
}

/// Recovers an interaction from a whole gateway frame (`{"op": 0, "t":
/// "INTERACTION_CREATE", "d": …}`) the shard couldn't deserialize; `None`
/// for any other frame.
pub fn recover_dispatch(frame: &str) -> Option<Interaction> {
    let value: serde_json::Value = serde_json::from_str(frame).ok()?;
    if value.get("t")?.as_str()? != "INTERACTION_CREATE" {
        return None;
    }
    decode_json(&value.get("d")?.to_string())
}
