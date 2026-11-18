//! Decoding interaction JSON, leniently when Twilight's model is stricter
//! than Discord's payload.

use twilight_model::application::interaction::Interaction as RawInteraction;

use super::{Interaction, decode};

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
    let mut value: serde_json::Value = serde_json::from_str(json).ok()?;
    let object = value.as_object_mut()?;
    for (key, default) in [
        ("attachment_size_limit", serde_json::json!(0)),
        ("authorizing_integration_owners", serde_json::json!({})),
        ("entitlements", serde_json::json!([])),
    ] {
        object.entry(key).or_insert(default);
    }
    let raw: RawInteraction = serde_json::from_value(value).ok()?;
    tracing::warn!("read an interaction Twilight couldn't read, with defaults for missing fields");
    Some(decode(&raw))
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
