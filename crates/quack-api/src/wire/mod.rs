//! Wire conventions shared by every route (§14.1): timestamps as RFC 3339
//! UTC with milliseconds, and the OpenAPI schemas of the `quack-core`
//! types that wire types embed as they are.

pub(crate) mod schema;

use serde::Serializer;
use time::{OffsetDateTime, UtcOffset};

/// Formats a time as `2026-11-18T10:00:00.123Z`.
pub(crate) fn format_ts(at: OffsetDateTime) -> String {
    let t = at.to_offset(UtcOffset::UTC);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute(),
        t.second(),
        t.millisecond()
    )
}

/// `#[serde(with = "ts")]` for an `OffsetDateTime` field.
pub(crate) mod ts {
    use super::{OffsetDateTime, Serializer, format_ts};

    /// Serializes as RFC 3339 UTC with milliseconds.
    pub(crate) fn serialize<S: Serializer>(at: &OffsetDateTime, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format_ts(*at))
    }

    /// `#[serde(with = "ts::option")]` for an `Option<OffsetDateTime>`.
    pub(crate) mod option {
        use super::{OffsetDateTime, Serializer, format_ts};

        /// Serializes as RFC 3339 UTC with milliseconds, or null.
        #[expect(clippy::ref_option, reason = "serde's `with` passes &Option")]
        pub(crate) fn serialize<S: Serializer>(
            at: &Option<OffsetDateTime>,
            s: S,
        ) -> Result<S::Ok, S::Error> {
            match at {
                Some(at) => s.serialize_str(&format_ts(*at)),
                None => s.serialize_none(),
            }
        }
    }
}

/// An account's avatar on Discord's CDN, or its default avatar.
pub(crate) fn avatar_url(account: u64, hash: Option<&str>) -> String {
    match hash {
        Some(hash) => format!("https://cdn.discordapp.com/avatars/{account}/{hash}.png"),
        None => format!(
            "https://cdn.discordapp.com/embed/avatars/{}.png",
            (account >> 22) % 6
        ),
    }
}

/// A server's icon on Discord's CDN.
pub(crate) fn icon_url(guild: u64, hash: Option<&str>) -> Option<String> {
    hash.map(|hash| format!("https://cdn.discordapp.com/icons/{guild}/{hash}.png"))
}

/// Parses an RFC 3339 query value (`from`, `created_after`, …).
pub(crate) fn parse_ts(text: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(text, &time::format_description::well_known::Rfc3339).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_have_milliseconds_in_utc() {
        let at = parse_ts("2026-11-18T12:34:56.789123+02:00");
        assert_eq!(
            at.map(format_ts),
            Some("2026-11-18T10:34:56.789Z".to_owned())
        );
        assert_eq!(
            parse_ts("2026-01-02T03:04:05Z").map(format_ts),
            Some("2026-01-02T03:04:05.000Z".to_owned())
        );
    }
}
