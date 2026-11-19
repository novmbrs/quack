//! Case markers: the `X-Audit-Log-Reason` Quack sends with every mutating
//! action call (§5.1). Discord keeps a ban's reason, so the marker later
//! tells which case placed a ban.

use crate::limits::truncate_chars;

/// Longest `X-Audit-Log-Reason` Discord accepts, in characters.
pub const AUDIT_LOG_REASON_MAX_CHARS: usize = 512;

const PREFIX: &str = "Quack case #";

/// The reason sent with an original action: `Quack case #<n>: <rule name>`,
/// cut to 512 characters.
pub fn case_marker(case_number: u64, rule_name: &str) -> String {
    let full = format!("{PREFIX}{case_number}: {rule_name}");
    truncate_chars(&full, AUDIT_LOG_REASON_MAX_CHARS).to_owned()
}

/// The reason sent with an undo action: `Quack case #<n> voided`.
pub fn undo_marker(case_number: u64) -> String {
    format!("{PREFIX}{case_number} voided")
}

/// The case number in a ban reason that starts with `Quack case #<n>`
/// followed by `:`, a space, or the end; `None` for any other reason.
pub fn parse_case_marker(reason: &str) -> Option<u64> {
    let rest = reason.strip_prefix(PREFIX)?;
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    let (digits, after) = rest.split_at(end);
    if digits.is_empty() || !(after.is_empty() || after.starts_with([':', ' '])) {
        return None;
    }
    digits.parse().ok().filter(|n| *n > 0)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn markers() {
        assert_eq!(case_marker(12, "Spam"), "Quack case #12: Spam");
        assert_eq!(undo_marker(12), "Quack case #12 voided");
        let long = case_marker(1, &"é".repeat(600));
        assert_eq!(long.chars().count(), 512);
        assert!(long.starts_with("Quack case #1: é"));
    }

    #[test]
    fn parsing() {
        assert_eq!(parse_case_marker("Quack case #12: Spam"), Some(12));
        assert_eq!(parse_case_marker("Quack case #12 voided"), Some(12));
        assert_eq!(parse_case_marker("Quack case #7"), Some(7));
        assert_eq!(
            parse_case_marker("Quack case #9007199254740991: x"),
            Some(9_007_199_254_740_991)
        );
        for other in [
            "",
            "Quack case #",
            "Quack case #0: x",
            "Quack case #12abc",
            "Quack case #-1",
            "quack case #12: x",
            " Quack case #12: x",
            "Banned by Quack case #12",
            "Quack case #99999999999999999999999: x",
            "Quack case # 12",
        ] {
            assert_eq!(parse_case_marker(other), None, "{other:?}");
        }
    }

    proptest! {
        #[test]
        fn every_marker_names_its_case(n in 1_u64..=9_007_199_254_740_991, name in ".{0,600}") {
            prop_assert_eq!(parse_case_marker(&case_marker(n, &name)), Some(n));
            prop_assert_eq!(parse_case_marker(&undo_marker(n)), Some(n));
            prop_assert!(case_marker(n, &name).chars().count() <= AUDIT_LOG_REASON_MAX_CHARS);
        }
    }
}
