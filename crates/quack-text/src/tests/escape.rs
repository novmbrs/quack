//! Text people type can't become Markdown, mentions, or links.

use super::fixtures::{HOSTILE, account, commands, long};
use crate::fmt::{clean, defuse_mentions, escape_markdown, safe_url};
use crate::model::{Inline, Person, Text};
use crate::render;

/// Every Markdown-significant character in `s` is escaped.
fn fully_escaped(s: &str) -> bool {
    let chars: Vec<char> = s.chars().collect();
    chars.iter().enumerate().all(|(i, c)| {
        let special = matches!(
            c,
            '*' | '_' | '~' | '`' | '|' | '<' | '>' | '[' | ']' | '(' | ')' | '@' | '#' | '-'
        );
        !special || (i > 0 && chars[i - 1] == '\\')
    })
}

#[test]
fn hostile_text_is_inert_on_discord() {
    let out = escape_markdown(HOSTILE);
    assert!(fully_escaped(&out), "{out}");
    assert!(!out.contains("https://"), "a bare link survived: {out}");
    assert!(!out.contains("]("), "a masked link survived: {out}");
    assert!(out.contains("\\@everyone"));
    assert!(out.contains("\\<\\@1\\>"));
    assert!(out.contains("\n\\# Heading"));
    assert!(out.contains("\n\\> quote"));
    assert!(out.contains("\n\\- list"));
    assert!(out.contains("\n1\\. one"));
    for c in ['\u{200B}', '\u{202E}', '\u{2066}'] {
        assert!(!out.contains(c), "{c:?} survived");
    }
}

#[test]
fn escaping_keeps_ordinary_text() {
    assert_eq!(
        escape_markdown("Hello, world. 3.5 stars!"),
        "Hello, world. 3.5 stars!"
    );
    assert_eq!(
        escape_markdown("  2. not at the start"),
        "  2\\. not at the start"
    );
    assert_eq!(escape_markdown("time: 10:30"), "time: 10:30");
    assert_eq!(escape_markdown("👨‍👩‍👧 family"), "👨‍👩‍👧 family");
}

#[test]
fn plain_text_keeps_what_was_typed() {
    let t = Text::new().user(HOSTILE);
    let plain = t.plain();
    assert!(plain.contains("@everyone <@1> <@&2> <#3> **bold** _it_"));
    assert!(plain.contains("[click](https://evil.example)"));
    assert!(!plain.contains('\u{200B}') && !plain.contains('\u{202E}'));
    assert_eq!(clean("a\u{7}b\u{FEFF}c\td\ne"), "abc\td\ne");
}

#[test]
fn names_never_render_as_text_on_discord() {
    let p = Person::new(account(222), "@everyone\n# Sam");
    let t = Text::new().person(&p);
    assert_eq!(render::markdown(&t, &commands()), "<@222>");
    assert_eq!(t.plain(), "@everyone # Sam");
    // An unknown name reads as words, with the ID as a labelled detail.
    assert_eq!(
        Text::new().person(&Person::unnamed(account(5))).plain(),
        "A member (ID 5)"
    );
    assert_eq!(
        Text::fixed("Ask ")
            .person(&Person::unnamed(account(5)))
            .plain(),
        "Ask a member (ID 5)"
    );
}

#[test]
fn server_markdown_keeps_formatting_but_not_mentions() {
    let out = defuse_mentions("**Read the rules** @everyone <@1> <@&2>");
    assert!(out.starts_with("**Read the rules**"));
    assert!(out.contains("\\@everyone") && out.contains("\\<\\@1>") && out.contains("\\<\\@&2>"));
    let t = Text::new().push(Inline::ServerMarkdown("hi @here".to_owned()));
    assert_eq!(render::markdown(&t, &commands()), "hi \\@here");
}

#[test]
fn links_are_safe() {
    assert_eq!(
        safe_url("https://x.example/a)b<c> d").as_deref(),
        Some("https://x.example/a%29b%3Cc%3Ed")
    );
    assert_eq!(safe_url("javascript:alert(1)"), None);
    let t = Text::new().link("Open", "javascript:alert(1)");
    assert_eq!(render::markdown(&t, &commands()), "Open");
    let t = Text::new().link("Open", "https://quack.example/x");
    assert_eq!(
        render::markdown(&t, &commands()),
        "[Open](<https://quack.example/x>)"
    );
    assert_eq!(t.plain(), "Open");
}

#[test]
fn commands_render_as_mentions_when_known() {
    let t = Text::new().command(crate::model::Command::CaseView);
    assert_eq!(render::markdown(&t, &commands()), "</case view:1001>");
    assert_eq!(
        render::markdown(&t, &crate::render::CommandIds::EMPTY),
        "`/case view`"
    );
    assert_eq!(t.plain(), "/case view");
}

#[test]
fn very_long_text_is_cut_to_the_budget() {
    let mut m = crate::model::Msg::default()
        .block(crate::model::Block::Heading(Text::fixed("Heading")))
        .block(crate::model::Block::Subtext(Text::fixed("small")))
        .block(crate::model::Block::Quote(Text::new().user(long(10_000))))
        .block(crate::model::Block::Text(Text::new().user(long(10_000))));
    crate::budget::fit(&mut m);
    assert!(
        crate::budget::check_msg(&m).is_empty(),
        "{:?}",
        crate::budget::check_msg(&m)
    );
    assert!(matches!(m.blocks[0], crate::model::Block::Heading(_)));
    assert!(
        !m.blocks
            .iter()
            .any(|b| matches!(b, crate::model::Block::Subtext(_)))
    );
    assert_eq!(
        m.blocks.last().map(
            |b| matches!(b, crate::model::Block::Text(t) if *t == crate::word::render_trimmed())
        ),
        Some(true)
    );
}
