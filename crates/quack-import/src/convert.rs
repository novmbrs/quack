//! Value conversions shared by every table (§19.3): ULIDs to UUIDs,
//! snowflake strings to `bigint`, sentinel strings to NULL, `datetime(3)` to
//! UTC, MySQL booleans and unsigned ranges, and text cleaning.

use quack_core::ids::ulid_to_uuid;
use time::{OffsetDateTime, PrimitiveDateTime};
use uuid::Uuid;

/// The first moment a Discord snowflake can name (2015-01-01 UTC). Message
/// times before it are Go zero values, not real times.
pub(crate) const DISCORD_EPOCH_UNIX: i64 = 1_420_070_400;

/// A legacy ULID's UUID (§19.3), or `None` when it is not a ULID.
pub(crate) fn uuid(ulid: &str) -> Option<Uuid> {
    ulid_to_uuid(ulid.trim()).ok()
}

/// What a snowflake column held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Snowflake {
    /// NULL, empty, or a sentinel meaning Quack or nobody.
    None,
    /// A valid snowflake in `1..=i64::MAX`.
    Id(i64),
    /// Anything else.
    Invalid,
}

impl Snowflake {
    /// The id, or `None` for both absent and invalid values.
    pub(crate) const fn id(self) -> Option<i64> {
        match self {
            Self::Id(id) => Some(id),
            Self::None | Self::Invalid => None,
        }
    }
}

/// Reads a snowflake string (§19.3): trimmed decimal of 1–20 digits in
/// `1..=i64::MAX`. NULL, `''`, `quack-system`, `system`, and `unknown` are
/// [`Snowflake::None`].
pub(crate) fn snowflake(raw: Option<&str>) -> Snowflake {
    let Some(raw) = raw else {
        return Snowflake::None;
    };
    let s = raw.trim();
    if s.is_empty() || matches!(s, "quack-system" | "system" | "unknown") {
        return Snowflake::None;
    }
    if s.len() > 20 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Snowflake::Invalid;
    }
    match s.parse::<u64>().ok().and_then(|v| i64::try_from(v).ok()) {
        Some(v) if v > 0 => Snowflake::Id(v),
        _ => Snowflake::Invalid,
    }
}

/// A `datetime(3)` read as UTC (the session time zone is `+00:00` and Go
/// wrote UTC).
pub(crate) const fn utc(value: PrimitiveDateTime) -> OffsetDateTime {
    value.assume_utc()
}

/// A Discord message time, or `None` for a Go zero value before Discord
/// existed.
pub(crate) fn message_time(value: PrimitiveDateTime) -> Option<OffsetDateTime> {
    let at = utc(value);
    (at.unix_timestamp() >= DISCORD_EPOCH_UNIX).then_some(at)
}

/// `tinyint(1)`: must be 0 or 1.
pub(crate) fn flag(value: i8) -> Option<bool> {
    match value {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}

/// `''` → `None` for optional text.
pub(crate) fn opt_text(value: Option<String>) -> Option<String> {
    value.filter(|s| !s.trim().is_empty())
}

/// Replaces control characters other than tab, newline, and carriage
/// return (and NUL, which PostgreSQL text cannot hold) with U+FFFD. Returns
/// whether anything changed.
pub(crate) fn clean(text: String) -> (String, bool) {
    let bad = |c: char| c.is_control() && !matches!(c, '\t' | '\n' | '\r');
    if !text.chars().any(bad) {
        return (text, false);
    }
    let cleaned = text
        .chars()
        .map(|c| if bad(c) { '\u{FFFD}' } else { c })
        .collect();
    (cleaned, true)
}

/// Number of characters (Unicode scalar values), as PostgreSQL's
/// `char_length` counts them.
pub(crate) fn chars(text: &str) -> usize {
    text.chars().count()
}

/// The first `max` characters of `text`.
pub(crate) fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// The icon hash from a Discord CDN icon URL
/// (`…/icons/<guild>/<hash>.<ext>`), or `None`.
pub(crate) fn icon_hash(url: &str) -> Option<String> {
    let path = url.split(['?', '#']).next()?;
    let mut segments = path.rsplit('/');
    let file = segments.next()?;
    let _guild = segments.next()?;
    if segments.next()? != "icons" {
        return None;
    }
    let hash = file.split('.').next()?;
    let ok = (1..=64).contains(&hash.len())
        && hash.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    ok.then(|| hash.to_owned())
}

/// The attachment id from a Discord CDN attachment URL
/// (`…/attachments/<channel>/<attachment>/<name>`).
pub(crate) fn attachment_parts(url: &str) -> Option<(i64, i64)> {
    let path = url.split(['?', '#']).next()?;
    let parts: Vec<&str> = path.split('/').collect();
    let at = parts.iter().position(|p| *p == "attachments")?;
    let channel = snowflake(parts.get(at + 1).copied()).id()?;
    let attachment = snowflake(parts.get(at + 2).copied()).id()?;
    parts.get(at + 3).filter(|name| !name.is_empty())?;
    Some((channel, attachment))
}

/// Normalizes a Go appeal invite link to `https://discord.gg/<code>`, or
/// `None` when it is not a Discord invite (§19.4).
pub(crate) fn invite_url(raw: &str) -> Option<String> {
    let s = raw.trim();
    let rest = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))?;
    let code = [
        "discord.gg/",
        "discord.com/invite/",
        "discordapp.com/invite/",
        "www.discord.com/invite/",
    ]
    .iter()
    .find_map(|p| rest.strip_prefix(p))?;
    let code = code.split(['?', '#']).next()?.trim_end_matches('/');
    let ok = (2..=64).contains(&code.len())
        && code
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    ok.then(|| format!("https://discord.gg/{code}"))
}

/// The `n`-th error code suffix test Go used: `ban_rate_limited` has the
/// class `rate_limited`.
pub(crate) fn has_class(code: &str, class: &str) -> bool {
    code == class
        || code
            .strip_suffix(class)
            .is_some_and(|prefix| prefix.ends_with('_'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn snowflake_sentinels_and_ranges() {
        assert_eq!(snowflake(None), Snowflake::None);
        for s in ["", "  ", "quack-system", "unknown", "system"] {
            assert_eq!(snowflake(Some(s)), Snowflake::None, "{s:?}");
        }
        assert_eq!(snowflake(Some(" 42 ")), Snowflake::Id(42));
        assert_eq!(
            snowflake(Some("9223372036854775807")),
            Snowflake::Id(i64::MAX)
        );
        for s in [
            "0",
            "-1",
            "9223372036854775808",
            "18446744073709551615",
            "123456789012345678901",
            "1e5",
            "abc",
            "12 3",
        ] {
            assert_eq!(snowflake(Some(s)), Snowflake::Invalid, "{s:?}");
        }
    }

    #[test]
    fn flags_are_zero_or_one() {
        assert_eq!(flag(0), Some(false));
        assert_eq!(flag(1), Some(true));
        assert_eq!(flag(2), None);
        assert_eq!(flag(-1), None);
    }

    #[test]
    fn cleaning_keeps_whitespace_and_replaces_controls() {
        assert_eq!(clean("a\tb\nc\r\n".into()), ("a\tb\nc\r\n".into(), false));
        assert_eq!(
            clean("a\0b\u{7}".into()),
            ("a\u{FFFD}b\u{FFFD}".into(), true)
        );
    }

    #[test]
    fn icon_hashes() {
        assert_eq!(
            icon_hash("https://cdn.discordapp.com/icons/123/a_abcdef0123.png?size=128").as_deref(),
            Some("a_abcdef0123")
        );
        assert_eq!(icon_hash(""), None);
        assert_eq!(icon_hash("https://example.com/x.png"), None);
    }

    #[test]
    fn attachment_urls() {
        assert_eq!(
            attachment_parts("https://cdn.discordapp.com/attachments/11/22/a.png?ex=1"),
            Some((11, 22))
        );
        assert_eq!(
            attachment_parts("https://cdn.discordapp.com/attachments/11/x/a.png"),
            None
        );
        assert_eq!(
            attachment_parts("https://cdn.discordapp.com/attachments/11/22/"),
            None
        );
    }

    #[test]
    fn invites() {
        assert_eq!(
            invite_url("https://discord.gg/abc").as_deref(),
            Some("https://discord.gg/abc")
        );
        assert_eq!(
            invite_url("https://discord.com/invite/Ab_-9/").as_deref(),
            Some("https://discord.gg/Ab_-9")
        );
        assert_eq!(invite_url("https://example.com/abc"), None);
        assert_eq!(invite_url("https://discord.gg/a"), None);
        assert_eq!(invite_url(""), None);
    }

    #[test]
    fn error_classes() {
        assert!(has_class("ban_rate_limited", "rate_limited"));
        assert!(has_class("rate_limited", "rate_limited"));
        assert!(!has_class("ban_not_rate_limited_x", "rate_limited"));
        assert!(!has_class("xrate_limited", "rate_limited"));
    }

    fn ulid(value: u128) -> String {
        const A: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
        (0..26)
            .rev()
            .map(|i| char::from(A[usize::try_from((value >> (i * 5)) & 31).unwrap()]))
            .collect()
    }

    proptest! {
        #[test]
        fn ulids_map_deterministically_and_keep_order(a: u128, b: u128) {
            let (ua, ub) = (uuid(&ulid(a)).unwrap(), uuid(&ulid(b)).unwrap());
            prop_assert_eq!(uuid(&ulid(a).to_lowercase()), Some(ua));
            prop_assert_eq!(ua.get_version_num(), 7);
            // Order is kept down to the dropped 6 bits.
            if a >> 6 < b >> 6 { prop_assert!(ua < ub); }
            if a >> 6 == b >> 6 { prop_assert_eq!(ua, ub); }
        }

        #[test]
        fn snowflakes_round_trip(v in 1..=i64::MAX) {
            prop_assert_eq!(snowflake(Some(&v.to_string())), Snowflake::Id(v));
            prop_assert_eq!(snowflake(Some(&format!(" {v}\t"))), Snowflake::Id(v));
        }

        #[test]
        fn snowflakes_never_accept_garbage(s in "\\PC{0,24}") {
            if let Snowflake::Id(v) = snowflake(Some(&s)) {
                prop_assert_eq!(s.trim().parse::<i64>().ok(), Some(v));
                prop_assert!(v > 0);
            }
        }

        #[test]
        fn datetimes_are_utc(secs in 0i64..4_102_444_800, ms in 0u16..1000) {
            let at = OffsetDateTime::from_unix_timestamp(secs).unwrap()
                + time::Duration::milliseconds(i64::from(ms));
            let naive = PrimitiveDateTime::new(at.date(), at.time());
            prop_assert_eq!(utc(naive), at);
            prop_assert_eq!(message_time(naive).is_some(), secs >= DISCORD_EPOCH_UNIX);
        }

        #[test]
        fn cleaned_text_has_no_bad_controls(s in "\\PC*|[\\x00-\\x1f]*") {
            let (out, changed) = clean(s.clone());
            prop_assert!(!out.chars().any(|c| c.is_control() && !matches!(c, '\t' | '\n' | '\r')));
            prop_assert_eq!(changed, out != s);
            prop_assert_eq!(chars(&out), chars(&s));
        }
    }
}
