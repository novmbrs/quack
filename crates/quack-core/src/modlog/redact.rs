//! Redaction of imported `legacy` entries (§9.5).

use serde_json::Value;

use crate::limits::truncate_chars;

/// The value that replaces anything redacted.
pub const REDACTED: &str = "[REDACTED]";

const SENSITIVE_KEYS: [&str; 13] = [
    "token",
    "secret",
    "password",
    "cookie",
    "authorization",
    "webhook",
    "session",
    "access_key",
    "private_key",
    "payload",
    "message_content",
    "member_content",
    "transcript",
];

const SENSITIVE_TEXT: [&str; 6] = [
    "token=",
    "authorization:",
    "bearer ",
    "password=",
    "secret=",
    "cookie=",
];

/// Longest `failure_reason` kept, in characters.
const FAILURE_REASON_CHARS: usize = 240;

/// Redacts a legacy detail object, recursively: values under keys that
/// contain a sensitive word (after lowercasing and `-` → `_`) become
/// `"[REDACTED]"`, and `failure_reason` strings go through
/// [`redact_failure_reason`].
pub fn redact(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, v)| {
                    let normalized = key.to_lowercase().replace('-', "_");
                    let v = if SENSITIVE_KEYS.iter().any(|s| normalized.contains(s)) {
                        Value::String(REDACTED.to_owned())
                    } else if normalized == "failure_reason"
                        && let Value::String(s) = &v
                    {
                        Value::String(redact_failure_reason(s))
                    } else {
                        redact(v)
                    };
                    (key, v)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(redact).collect()),
        other => other,
    }
}

/// Redacts a Go failure reason: one that looks like it carries a credential
/// becomes `"[REDACTED]"`; others are cut to 240 characters.
pub fn redact_failure_reason(reason: &str) -> String {
    let lower = reason.to_lowercase();
    if SENSITIVE_TEXT.iter().any(|s| lower.contains(s)) {
        REDACTED.to_owned()
    } else {
        truncate_chars(reason, FAILURE_REASON_CHARS).to_owned()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn redacts_sensitive_keys_recursively() {
        let input = json!({
            "action": "case.create",
            "Bot-Token": "abc",
            "nested": { "webhook_url": "https://x", "ok": 1, "list": [{ "SESSION_ID": "s" }, "plain"] },
            "payload": { "anything": true },
            "messageContent": "kept: camelCase does not match message_content",
            "failure_reason": "Authorization: Bearer abc",
        });
        assert_eq!(
            redact(input),
            json!({
                "action": "case.create",
                "Bot-Token": REDACTED,
                "nested": { "webhook_url": REDACTED, "ok": 1, "list": [{ "SESSION_ID": REDACTED }, "plain"] },
                "payload": REDACTED,
                "messageContent": "kept: camelCase does not match message_content",
                "failure_reason": REDACTED,
            })
        );
    }

    #[test]
    fn failure_reasons() {
        for bad in [
            "token=abc",
            "PASSWORD=x",
            "use bearer xyz",
            "secret=1",
            "cookie=a",
            "authorization: x",
        ] {
            assert_eq!(redact_failure_reason(bad), REDACTED, "{bad}");
        }
        assert_eq!(
            redact_failure_reason("missing permissions"),
            "missing permissions"
        );
        assert_eq!(redact_failure_reason(&"é".repeat(300)).chars().count(), 240);
        assert_eq!(
            redact(json!({ "failure_reason": 5 })),
            json!({ "failure_reason": 5 })
        );
    }
}
