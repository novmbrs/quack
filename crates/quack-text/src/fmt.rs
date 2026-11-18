//! Small formatting helpers shared by every message: escaping, cutting,
//! durations, counts and plurals, lists, and dates.

use time::{Month, OffsetDateTime, UtcOffset};

/// Characters removed from any text a person typed: controls (except line
/// breaks and tabs), bidirectional overrides that can disguise text, and
/// invisible spaces that can hide a mention or a link. Joiners used by
/// emoji and many scripts (U+200C, U+200D) are kept.
fn is_unsafe(c: char) -> bool {
    (c.is_control() && c != '\n' && c != '\t')
        || matches!(
            c,
            '\u{200B}'
                | '\u{200E}'
                | '\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2060}'..='\u{2064}'
                | '\u{2066}'..='\u{2069}'
                | '\u{180E}'
                | '\u{FEFF}'
        )
}

/// Removes control, bidirectional, and invisible characters from text a
/// person typed. Used for plain text; [`escape_markdown`] does it too.
pub fn clean(s: &str) -> String {
    s.chars().filter(|c| !is_unsafe(*c)).collect()
}

/// [`clean`], with line breaks and tabs turned into spaces, for names shown
/// inside a sentence.
pub fn one_line(s: &str) -> String {
    clean(s)
        .chars()
        .map(|c| if c == '\n' || c == '\t' { ' ' } else { c })
        .collect()
}

/// Escapes text a person typed so Discord shows it exactly as written:
/// no Markdown, no mentions (`@everyone`, `<@id>`), no masked or bare
/// links, no headings, quotes, or lists at the start of a line.
pub fn escape_markdown(s: &str) -> String {
    let s = clean(s);
    let mut out = String::with_capacity(s.len() + s.len() / 4);
    // Only spaces so far on this line; only spaces then digits so far.
    let mut line_start = true;
    let mut in_number = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        let escape = match c {
            '\\' | '*' | '_' | '~' | '`' | '|' | '<' | '>' | '[' | ']' | '(' | ')' | '@' | '#'
            | '-' | '+' => true,
            // `https://` would become a link.
            ':' => chars.peek() == Some(&'/'),
            // `1. item` at the start of a line would become a list.
            '.' => in_number,
            _ => false,
        };
        if escape {
            out.push('\\');
        }
        out.push(c);
        if c == '\n' {
            line_start = true;
            in_number = false;
        } else if !(c == ' ' && line_start) {
            in_number = c.is_ascii_digit() && (line_start || in_number);
            line_start = false;
        }
    }
    out
}

/// Keeps a server's own Markdown (DM introduction, honeypot warning) but
/// stops it from mentioning anyone: `@` and `<@` are escaped.
pub fn defuse_mentions(s: &str) -> String {
    clean(s).replace('@', "\\@").replace("<\\@", "\\<\\@")
}

/// The URL if it is safe inside `[text](<url>)`: `https://` or `http://`,
/// with characters that could end the link percent-encoded.
pub fn safe_url(url: &str) -> Option<String> {
    let url = url.trim();
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return None;
    }
    let mut out = String::with_capacity(url.len());
    for c in url.chars() {
        match c {
            '<' => out.push_str("%3C"),
            '>' => out.push_str("%3E"),
            '(' => out.push_str("%28"),
            ')' => out.push_str("%29"),
            '\\' => out.push_str("%5C"),
            c if c.is_whitespace() || is_unsafe(c) => {}
            c => out.push(c),
        }
    }
    Some(out)
}

/// Cuts text to at most `max` characters (Unicode scalar values), ending
/// with "…" when anything was removed.
pub fn cut(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let keep = max.saturating_sub(1);
    let mut out: String = s.chars().take(keep).collect();
    out.truncate(out.trim_end().len());
    out.push('…');
    out
}

/// A count with thousands separators: "1,234".
pub fn count(n: impl Into<u128>) -> String {
    let digits = n.into().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// "1 item" or "3 items": the count, then the right form of the noun.
pub fn plural(n: u64, one: &str, other: &str) -> String {
    format!("{} {}", count(n), if n == 1 { one } else { other })
}

/// "case #12".
pub fn case_ref(number: u64) -> String {
    format!("case #{number}")
}

/// "level 2 (starts at case 3)".
pub fn level(index: u8, from_case_count: u32) -> String {
    format!("level {index} (starts at case {from_case_count})")
}

/// Joins items as "a", "a and b", or "a, b, and c" (`last` is "and" or "or").
pub fn list(items: &[String], last: &str) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [a, b] => format!("{a} {last} {b}"),
        [init @ .., tail] => format!("{}, {last} {tail}", init.join(", ")),
    }
}

const UNITS: [(u64, &str, &str); 4] = [
    (86_400, "day", "days"),
    (3_600, "hour", "hours"),
    (60, "minute", "minutes"),
    (1, "second", "seconds"),
];

/// A duration in its largest whole unit and, when not zero, the next one:
/// "7 days", "1 hour 30 minutes", "45 seconds".
pub fn duration(seconds: u64) -> String {
    let Some(i) = UNITS.iter().position(|(size, ..)| seconds >= *size) else {
        return "0 seconds".to_owned();
    };
    let (size, one, other) = UNITS[i];
    let mut out = plural(seconds / size, one, other);
    if let Some((next, one, other)) = UNITS.get(i + 1) {
        let rest = seconds % size / next;
        if rest > 0 {
            out.push(' ');
            out.push_str(&plural(rest, one, other));
        }
    }
    out
}

/// A duration after "the last": "day" rather than "1 day", otherwise as
/// [`duration`] ("7 days", "1 hour 30 minutes").
pub fn last(seconds: u64) -> String {
    let d = duration(seconds);
    d.strip_prefix("1 ")
        .filter(|rest| !rest.contains(' '))
        .map_or(d.clone(), str::to_owned)
}

const fn month_name(m: Month) -> &'static str {
    match m {
        Month::January => "Jan",
        Month::February => "Feb",
        Month::March => "Mar",
        Month::April => "Apr",
        Month::May => "May",
        Month::June => "Jun",
        Month::July => "Jul",
        Month::August => "Aug",
        Month::September => "Sep",
        Month::October => "Oct",
        Month::November => "Nov",
        Month::December => "Dec",
    }
}

/// "17 Nov 2026".
pub fn date(at: OffsetDateTime) -> String {
    let at = at.to_offset(UtcOffset::UTC);
    format!("{} {} {}", at.day(), month_name(at.month()), at.year())
}

/// "17 Nov 2026 at 14:05 UTC".
pub fn datetime(at: OffsetDateTime) -> String {
    let utc = at.to_offset(UtcOffset::UTC);
    format!("{} at {:02}:{:02} UTC", date(at), utc.hour(), utc.minute())
}

/// "2026-11-17 14:05:09 UTC", for ticket transcripts.
pub fn transcript_time(at: OffsetDateTime) -> String {
    let at = at.to_offset(UtcOffset::UTC);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
        at.year(),
        u8::from(at.month()),
        at.day(),
        at.hour(),
        at.minute(),
        at.second()
    )
}

/// Lowercases the first letter, for reusing a sentence after a colon.
pub fn lower_first(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_lowercase().chain(chars).collect()
    })
}

/// Uppercases the first letter, for a name that starts a sentence.
pub fn upper_first(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(duration(0), "0 seconds");
        assert_eq!(duration(45), "45 seconds");
        assert_eq!(duration(60), "1 minute");
        assert_eq!(duration(5_400), "1 hour 30 minutes");
        assert_eq!(duration(86_400), "1 day");
        assert_eq!(duration(90_061), "1 day 1 hour");
        assert_eq!(duration(86_460), "1 day");
        assert_eq!(duration(604_800), "7 days");
        assert_eq!(duration(2_419_200), "28 days");
        assert_eq!(last(86_400), "day");
        assert_eq!(last(3_600), "hour");
        assert_eq!(last(604_800), "7 days");
        assert_eq!(last(5_400), "1 hour 30 minutes");
    }

    #[test]
    fn counts_and_lists() {
        assert_eq!(count(0u32), "0");
        assert_eq!(count(1_234u32), "1,234");
        assert_eq!(count(1_000_000u32), "1,000,000");
        assert_eq!(plural(1, "item", "items"), "1 item");
        assert_eq!(plural(2, "item", "items"), "2 items");
        let s = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(list(&s(&[]), "and"), "");
        assert_eq!(list(&s(&["a"]), "and"), "a");
        assert_eq!(list(&s(&["a", "b"]), "or"), "a or b");
        assert_eq!(list(&s(&["a", "b", "c"]), "and"), "a, b, and c");
        assert_eq!(case_ref(12), "case #12");
        assert_eq!(level(2, 3), "level 2 (starts at case 3)");
    }

    #[test]
    fn cutting() {
        assert_eq!(cut("short", 10), "short");
        assert_eq!(cut("exactly10!", 10), "exactly10!");
        assert_eq!(cut("a longer sentence", 10), "a longer…");
        assert_eq!(cut("ééééééééééé", 5).chars().count(), 5);
    }

    #[test]
    fn dates() {
        let at = time::macros::datetime!(2026-11-17 14:05:09 UTC);
        assert_eq!(date(at), "17 Nov 2026");
        assert_eq!(datetime(at), "17 Nov 2026 at 14:05 UTC");
        assert_eq!(transcript_time(at), "2026-11-17 14:05:09 UTC");
    }
}
