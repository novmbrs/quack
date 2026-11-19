//! Every limit in the design's limits table (§3.9), as named constants.
//!
//! "Characters" means Unicode scalar values after trimming surrounding
//! whitespace. Database CHECK constraints mirror these where a column holds
//! the value.

use time::Duration;

/// An inclusive range of characters a text value must fall in after trimming.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CharLimit {
    /// Fewest characters allowed.
    pub min: usize,
    /// Most characters allowed.
    pub max: usize,
}

impl CharLimit {
    /// Builds a limit of `min..=max` characters.
    pub const fn new(min: usize, max: usize) -> Self {
        Self { min, max }
    }

    /// Whether `text`, trimmed, has an allowed number of characters.
    pub fn allows(self, text: &str) -> bool {
        let n = char_count(text);
        n >= self.min && n <= self.max
    }
}

/// Counts the characters of `text` after trimming, as every limit does.
pub fn char_count(text: &str) -> usize {
    text.trim().chars().count()
}

/// Cuts `text` to at most `max` characters, never splitting a character.
pub fn truncate_chars(text: &str, max: usize) -> &str {
    match text.char_indices().nth(max) {
        Some((at, _)) => &text[..at],
        None => text,
    }
}

/// Rule name.
pub const RULE_NAME: CharLimit = CharLimit::new(1, 100);
/// Rule description (staff-facing; may be empty).
pub const RULE_DESCRIPTION: CharLimit = CharLimit::new(0, 500);
/// Characters of a rule description shown in pickers.
pub const RULE_DESCRIPTION_IN_PICKER: usize = 100;
/// Official reason shown to the member.
pub const RULE_REASON: CharLimit = CharLimit::new(1, 1_000);
/// Most case context fields per rule.
pub const MAX_FIELDS: usize = 10;
/// Field key length (the key also matches `^[a-z0-9][a-z0-9_-]{1,63}$`).
pub const FIELD_KEY: CharLimit = CharLimit::new(2, 64);
/// Field label.
pub const FIELD_LABEL: CharLimit = CharLimit::new(1, 100);
/// Fewest levels per rule.
pub const MIN_LEVELS: usize = 1;
/// Most levels per rule.
pub const MAX_LEVELS: usize = 10;
/// Highest `from_case_count` a level may start at.
pub const MAX_LEVEL_CASE_COUNT: u32 = 1_000_000;

/// The most levels an imported rule version may have: a case stores its
/// level's position in a `smallint`, so every validated version can
/// always create cases.
pub const MAX_IMPORTED_LEVELS: usize = 32_767;
/// Shortest timeout, in seconds.
pub const MIN_TIMEOUT_SECONDS: u32 = 1;
/// Longest timeout, in seconds (28 days, Discord's limit).
pub const MAX_TIMEOUT_SECONDS: u32 = 2_419_200;
/// Longest ban message deletion window, in seconds (7 days).
pub const MAX_BAN_DELETE_SECONDS: u32 = 604_800;
/// Shortest counting window, in days.
pub const MIN_COUNTING_WINDOW_DAYS: u16 = 1;
/// Longest counting window, in days.
pub const MAX_COUNTING_WINDOW_DAYS: u16 = 36_500;
/// Most active (non-archived) rules per server.
pub const MAX_ACTIVE_RULES: u32 = 100;

/// Case context `short_text` answer.
pub const CONTEXT_SHORT_TEXT: CharLimit = CharLimit::new(1, 500);
/// Case context `long_text` answer.
pub const CONTEXT_LONG_TEXT: CharLimit = CharLimit::new(1, 4_000);
/// Longest case context `message_link` input, in characters.
pub const CONTEXT_MESSAGE_LINK_MAX_CHARS: usize = 4_000;
/// Largest absolute value of a case context `number` answer (10^15).
pub const CONTEXT_NUMBER_MAX_ABS: i128 = 1_000_000_000_000_000;
/// Most decimal places of a case context `number` answer.
pub const CONTEXT_NUMBER_MAX_DECIMALS: u32 = 6;

/// Highest case number a server can reach (2^53 − 1, exact in JavaScript).
pub const MAX_CASE_NUMBER: u64 = (1 << 53) - 1;

/// Void reason.
pub const VOID_REASON: CharLimit = CharLimit::new(1, 1_000);
/// Appeal statement and information reply.
pub const APPEAL_TEXT: CharLimit = CharLimit::new(1, 4_000);
/// Appeal decision reason.
pub const APPEAL_DECISION_REASON: CharLimit = CharLimit::new(1, 2_000);
/// Characters of an appeal statement shown in the appeal post.
pub const APPEAL_POST_STATEMENT_CHARS: usize = 1_500;

/// Most evidence items (saved messages) per case.
pub const MAX_EVIDENCE_ITEMS: u32 = 50;
/// Most evidence files per case.
pub const MAX_EVIDENCE_FILES: u32 = 100;
/// Most message links in one add-evidence call.
pub const MAX_LINKS_PER_CALL: usize = 10;
/// Saved message text per evidence item, in characters.
pub const EVIDENCE_TEXT_CHARS: usize = 4_000;
/// Saved embeds per evidence item.
pub const MAX_EVIDENCE_EMBEDS: usize = 10;
/// Saved embed title, in characters.
pub const EVIDENCE_EMBED_TITLE_CHARS: usize = 256;
/// Saved embed description, in characters.
pub const EVIDENCE_EMBED_DESCRIPTION_CHARS: usize = 1_000;
/// Saved files per evidence item.
pub const MAX_FILES_PER_EVIDENCE: u32 = 10;
/// Largest file Quack copies, in bytes (10 MiB).
pub const MAX_COPY_BYTES: u64 = 10_485_760;

/// Most moderator roles, and separately most rules manager roles.
pub const MAX_STAFF_ROLES: usize = 25;
/// DM introduction (empty stores nothing).
pub const DM_INTRO: CharLimit = CharLimit::new(0, 1_000);
/// DM footer (empty stores nothing).
pub const DM_FOOTER: CharLimit = CharLimit::new(0, 500);
/// Rejoin invite code length (`[A-Za-z0-9_-]`).
pub const INVITE_CODE: CharLimit = CharLimit::new(2, 64);
/// Honeypot warning text (empty uses the generated warning).
pub const HONEYPOT_WARNING: CharLimit = CharLimit::new(0, 1_500);
/// Shortest ticket transcript retention, in days.
pub const MIN_TICKET_RETENTION_DAYS: u16 = 1;
/// Longest ticket transcript retention, in days.
pub const MAX_TICKET_RETENTION_DAYS: u16 = 365;
/// Default ticket transcript retention, in days.
pub const DEFAULT_TICKET_RETENTION_DAYS: u16 = 90;
/// Largest ticket transcript, in bytes (8 MiB).
pub const MAX_TRANSCRIPT_BYTES: u64 = 8_388_608;

/// `Idempotency-Key` header length (printable ASCII).
pub const IDEMPOTENCY_KEY: CharLimit = CharLimit::new(8, 200);
/// Default API page size.
pub const DEFAULT_PAGE_SIZE: u32 = 50;
/// Largest API page size.
pub const MAX_PAGE_SIZE: u32 = 100;
/// How long after a create's start Quack repeats it with the same nonce
/// (§10.4). Discord documents no exact window; Quack assumes 60 seconds.
pub const NONCE_WINDOW: Duration = Duration::seconds(60);
/// Largest HTTP request body, in bytes (256 KiB).
pub const MAX_BODY_BYTES: usize = 262_144;
/// Shortest statistics range.
pub const MIN_STATS_RANGE: Duration = Duration::seconds(1);
/// Longest statistics range.
pub const MAX_STATS_RANGE: Duration = Duration::days(366);

/// Whether `key` is an acceptable `Idempotency-Key` header: 8–200 printable
/// ASCII characters (space through tilde).
pub fn idempotency_key_ok(key: &str) -> bool {
    (IDEMPOTENCY_KEY.min..=IDEMPOTENCY_KEY.max).contains(&key.len())
        && key.bytes().all(|b| (0x20..=0x7e).contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_limits_count_scalar_values_after_trimming() {
        assert!(RULE_NAME.allows("  a  "));
        assert!(!RULE_NAME.allows("   "));
        assert!(RULE_NAME.allows(&"é".repeat(100)));
        assert!(!RULE_NAME.allows(&"é".repeat(101)));
        assert!(RULE_DESCRIPTION.allows(""));
    }

    #[test]
    fn truncation_respects_characters() {
        assert_eq!(truncate_chars("héllo", 2), "hé");
        assert_eq!(truncate_chars("hi", 5), "hi");
        assert_eq!(truncate_chars("", 0), "");
    }

    #[test]
    fn idempotency_keys() {
        assert!(idempotency_key_ok("abcdefgh"));
        assert!(!idempotency_key_ok("abcdefg"));
        assert!(!idempotency_key_ok(&"a".repeat(201)));
        assert!(!idempotency_key_ok("abcdefg\n"));
        assert!(!idempotency_key_ok("abcdéfgh"));
    }
}
