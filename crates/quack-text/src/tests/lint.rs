//! Limits, the glossary lint, and "couldn't" versus "couldn't confirm".

use super::fixtures::{commands, sam};
use super::samples::{Out, Sample, all};
use super::snapshots::fitted;
use crate::action::{self, Detail};
use crate::budget;
use crate::dm;
use crate::log;
use crate::model::{Block, ButtonTarget, InputKind, Msg, Row, SelectOptions};
use crate::render;
use crate::word::Outcome;

/// Every string of a sample a person could read, on both surfaces.
fn strings(s: &Sample) -> Vec<String> {
    let ids = commands();
    let mut v = Vec::new();
    match &s.out {
        Out::Msg(m) => {
            v.push(render::msg_markdown(m, &ids));
            v.push(m.plain());
            for row in &m.rows {
                match row {
                    Row::Buttons(b) => v.extend(b.iter().map(|b| b.label.clone())),
                    Row::Select(sel) => {
                        v.push(sel.placeholder.clone());
                        if let SelectOptions::Strings(o) = &sel.options {
                            v.extend(o.iter().map(|o| o.label.clone()));
                        }
                    }
                }
            }
        }
        Out::Text(t) => {
            v.push(render::markdown(t, &ids));
            v.push(t.plain());
        }
        Out::Modal(m) => {
            v.push(m.title.clone());
            for i in &m.inputs {
                v.push(i.label.clone());
                v.extend(i.description.clone());
                match &i.kind {
                    InputKind::Text {
                        placeholder, value, ..
                    } => {
                        v.extend(placeholder.clone());
                        v.extend(value.clone());
                    }
                    InputKind::YesNo { placeholder, .. } => v.push(placeholder.clone()),
                }
            }
        }
        Out::Str(text, _) => v.push(text.clone()),
    }
    v
}

#[test]
fn every_message_fits_discord_limits() {
    let mut failures = Vec::new();
    for s in all() {
        let problems = match &s.out {
            Out::Msg(m) => {
                let f = fitted(m);
                let headings = |m: &Msg| {
                    m.blocks
                        .iter()
                        .filter(|b| matches!(b, Block::Heading(_)))
                        .count()
                };
                let mut p = budget::check_msg(&f);
                if headings(&f) != headings(m) || f.rows != m.rows {
                    p.push("trimming dropped a heading or a row".to_owned());
                }
                for row in &f.rows {
                    if let Row::Buttons(b) = row {
                        for b in b {
                            if let ButtonTarget::Link(url) = &b.target {
                                assert!(url.starts_with("https://"), "{url}");
                            }
                        }
                    }
                }
                p
            }
            Out::Modal(m) => budget::check_modal(m),
            Out::Text(t) => {
                let len = render::markdown(t, &commands()).chars().count();
                if len > budget::TEXT_BUDGET {
                    vec![format!("text has {len} characters")]
                } else {
                    vec![]
                }
            }
            Out::Str(text, limit) => {
                let len = text.chars().count();
                if len > *limit {
                    vec![format!("{len} characters (limit {limit})")]
                } else {
                    vec![]
                }
            }
        };
        for p in problems {
            failures.push(format!("{} [{}]: {p}", s.id, s.variant));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn command_texts_fit() {
    for c in crate::command::ALL {
        assert!(
            c.description.chars().count() <= budget::COMMAND_DESCRIPTION,
            "{}",
            c.name
        );
        assert!(
            c.name.len() <= budget::COMMAND_NAME && c.name == c.name.to_lowercase(),
            "{}",
            c.name
        );
    }
}

/// Words the glossary says never to use (§1.3, §1.4).
const BANNED: &[&str] = &[
    "template",
    "policy",
    "policies",
    "infraction",
    "punish",
    "punishment",
    "enforce",
    "enforcement",
    "execution",
    "executed",
    "notification",
    "notice",
    "mirror",
    "general logging",
    "offender",
    "offense count",
    "user",
    "users",
    "target",
    "incident",
    "revoke",
    "revoked",
    "tier",
    "step",
    "metadata",
    "attachment",
    "decline",
    "declined",
    "deny",
    "denied",
    "request",
    "requests",
    "resource",
    "resources",
    "delivery",
    "delivery updated",
    "status code",
    "mod",
    "mods",
    "admin",
    "admins",
    "audit log",
    "record",
    "records",
    "lease",
    "deleted the case",
    "delete a case",
    "disable",
    "disabled",
];

/// Legitimate uses: Discord's own permission and UI names (where staff
/// check a kick), and two fixed
/// phrases (a DM's "for your records", the starter rule notice setting).
const ALLOWED: &[&str] = &[
    "View Audit Log",
    "Server Settings → Audit Log",
    "Deny View Channel",
    "for your records",
    "starter rule notice",
];

fn words_of(s: &str) -> String {
    let mut s = s.to_owned();
    for a in ALLOWED {
        s = s.replace(a, " ");
    }
    // Drop URLs, mentions, and timestamps; they are not words.
    s.split_whitespace()
        .filter(|w| !w.contains("://") && !w.starts_with('<'))
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn contains_word(haystack: &str, word: &str) -> bool {
    let bytes = haystack.as_bytes();
    let mut from = 0;
    while let Some(i) = haystack[from..].find(word) {
        let start = from + i;
        let end = start + word.len();
        let before = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
        let after = end == bytes.len() || !bytes[end].is_ascii_alphanumeric();
        if before && after {
            return true;
        }
        from = end;
    }
    false
}

/// A machine name in a sentence: `snake_case` or `CamelCase`.
fn looks_like_identifier(word: &str) -> bool {
    let w = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '_');
    let snake = w.contains('_')
        && w.chars()
            .all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit());
    let chars: Vec<char> = w.chars().collect();
    let camel = chars
        .windows(2)
        .any(|p| p[0].is_ascii_lowercase() && p[1].is_ascii_uppercase());
    snake || camel
}

/// Samples that show a machine key or id as sent: the API's unknown and
/// duplicate context keys, and the ticket thread name.
const SHOWS_KEYS: &[&str] = &[
    "context.unknown_field",
    "context.duplicate_field",
    "ticket.thread_name",
];

fn hostile(s: &Sample) -> bool {
    s.variant.contains("hostile")
        || s.variant.contains("maximal")
        || s.id.starts_with("activity.message")
}

#[test]
fn glossary_lint() {
    let mut failures = Vec::new();
    for s in all() {
        if hostile(&s) {
            continue;
        }
        for text in strings(&s) {
            let words = words_of(&text);
            for b in BANNED {
                if contains_word(&words, b) {
                    failures.push(format!("{} [{}]: says {b:?}: {text}", s.id, s.variant));
                }
            }
            for w in text
                .split_whitespace()
                .filter(|w| !w.contains("://") && !w.starts_with('<'))
            {
                if looks_like_identifier(w) && !SHOWS_KEYS.contains(&s.id) {
                    failures.push(format!(
                        "{} [{}]: machine name {w:?}: {text}",
                        s.id, s.variant
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

const CONFIRM: &str = "couldn't confirm";

fn says_couldnt(s: &str) -> bool {
    s.replace(CONFIRM, "").contains("couldn't")
}

#[test]
fn couldnt_confirm_only_for_unknown_outcomes() {
    let m = sam();
    for (name, a) in super::samples::actions_views() {
        let outcome = action::outcome(&a);
        for detail in [Detail::Full, Detail::Receipt] {
            let s = action::sentence(&a, &m, detail).plain();
            assert_eq!(
                s.contains(CONFIRM),
                outcome == Outcome::CouldntConfirm,
                "{name}: {s}"
            );
            assert_eq!(says_couldnt(&s), outcome == Outcome::Failed, "{name}: {s}");
        }
        let member = action::member_sentence(&a).plain();
        assert_eq!(
            member.contains(CONFIRM),
            outcome == Outcome::CouldntConfirm,
            "{name}: {member}"
        );
        assert!(!says_couldnt(&member), "{name}: {member}");
    }
    for (name, d) in super::samples::dms_views() {
        let outcome = dm::outcome(d.as_ref());
        for s in [
            dm::sentence(d.as_ref(), &m, 12).plain(),
            dm::member_sentence(d.as_ref()).plain(),
        ] {
            assert_eq!(
                s.contains(CONFIRM),
                outcome == Outcome::CouldntConfirm,
                "{name}: {s}"
            );
            assert_eq!(says_couldnt(&s), outcome == Outcome::Failed, "{name}: {s}");
        }
    }
    let facts = super::samples::log_facts();
    for (id, name, e) in super::samples::log_entries() {
        let s = log::text(&e, &facts).plain();
        let unknown = id.ends_with("unknown")
            || id.ends_with("unconfirmed")
            || id.ends_with("retries_exhausted");
        assert_eq!(s.contains(CONFIRM), unknown, "{id} {name}: {s}");
        if s.contains(CONFIRM) {
            assert!(!says_couldnt(&s), "{id} {name}: says both: {s}");
        }
    }
}

#[test]
fn no_sentence_says_both() {
    for s in all() {
        for text in strings(&s) {
            for sentence in text.split(". ") {
                assert!(
                    !(sentence.contains(CONFIRM) && says_couldnt(sentence)),
                    "{} [{}]: {sentence}",
                    s.id,
                    s.variant
                );
            }
        }
    }
}
