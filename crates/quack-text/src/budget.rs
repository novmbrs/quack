//! Discord's limits, a checker for them, and the trimming that keeps a long
//! message inside the text budget (§11.11).
//!
//! Message functions cut labels and titles as they build them; tests run
//! [`check_msg`] and [`check_modal`] on every message with maximal inputs,
//! so going over a limit is a test failure rather than a Discord error.

use crate::fmt;
use crate::model::{
    Block, Button, ButtonTarget, Inline, InputKind, Modal, Msg, Row, SelectOptions, Text,
};
use crate::render::{self, CommandIds};

/// Most characters in a modal title.
pub const MODAL_TITLE: usize = 45;
/// Most characters in a button label.
pub const BUTTON_LABEL: usize = 80;
/// Most buttons in one row.
pub const BUTTONS_PER_ROW: usize = 5;
/// Most characters in a select option's label.
pub const SELECT_OPTION_LABEL: usize = 100;
/// Most characters in a select option's description.
pub const SELECT_OPTION_DESCRIPTION: usize = 100;
/// Most characters in a select placeholder.
pub const SELECT_PLACEHOLDER: usize = 150;
/// Most options in one select.
pub const SELECT_OPTIONS: usize = 25;
/// Most characters in a modal input's label.
pub const INPUT_LABEL: usize = 45;
/// Most characters in a modal input's description.
pub const INPUT_DESCRIPTION: usize = 100;
/// Most characters in a modal input's placeholder.
pub const INPUT_PLACEHOLDER: usize = 100;
/// Most characters a modal text input accepts.
pub const INPUT_MAX_LENGTH: usize = 4_000;
/// Most inputs in one modal.
pub const MODAL_INPUTS: usize = 5;
/// Most characters in a command or option name.
pub const COMMAND_NAME: usize = 32;
/// Most characters in a command or option description.
pub const COMMAND_DESCRIPTION: usize = 100;
/// Most characters in a command choice's name.
pub const CHOICE_NAME: usize = 100;
/// Most characters in a channel name.
pub const CHANNEL_NAME: usize = 100;
/// Most characters in a channel topic.
pub const CHANNEL_TOPIC: usize = 1_024;
/// Most characters in an `X-Audit-Log-Reason`.
pub const AUDIT_LOG_REASON: usize = 512;
/// Most characters in a link button's URL.
pub const LINK_URL: usize = 512;
/// Most characters of text in one message, counted after escaping.
pub const TEXT_BUDGET: usize = 4_000;
/// Most components in one message, counted with nesting.
pub const COMPONENT_BUDGET: usize = 40;

/// The text Discord counts: every text block as Markdown, consecutive text
/// blocks joined by line breaks (the renderer puts each run of text blocks
/// in one Text Display).
pub fn text_len(msg: &Msg) -> usize {
    render::msg_markdown(msg, &CommandIds::EMPTY)
        .chars()
        .count()
}

/// Components counted with nesting: the container, one Text Display per run
/// of consecutive text blocks, each separator and file, and each row with
/// its buttons or select.
pub fn component_count(msg: &Msg) -> usize {
    let mut count = 1;
    let mut previous_was_text = false;
    for block in &msg.blocks {
        let text = !matches!(block, Block::Divider | Block::File(_));
        if !text || !previous_was_text {
            count += 1;
        }
        previous_was_text = text;
    }
    for row in &msg.rows {
        count += 1 + match row {
            Row::Buttons(b) => b.len(),
            Row::Select(_) => 1,
        };
    }
    count
}

fn over(what: &str, len: usize, max: usize, problems: &mut Vec<String>) {
    if len > max {
        problems.push(format!("{what} has {len} characters (limit {max})"));
    }
}

fn chars(s: &str) -> usize {
    s.chars().count()
}

fn check_button(b: &Button, problems: &mut Vec<String>) {
    over("button label", chars(&b.label), BUTTON_LABEL, problems);
    if b.label.trim().is_empty() {
        problems.push("button label is empty".to_owned());
    }
    if let ButtonTarget::Link(url) = &b.target {
        over("link URL", chars(url), LINK_URL, problems);
        if fmt::safe_url(url).is_none() {
            problems.push(format!("link URL {url:?} is not http(s)"));
        }
    }
}

/// Every Discord limit the message breaks; empty when it fits.
pub fn check_msg(msg: &Msg) -> Vec<String> {
    let mut problems = Vec::new();
    over("message text", text_len(msg), TEXT_BUDGET, &mut problems);
    let components = component_count(msg);
    if components > COMPONENT_BUDGET {
        problems.push(format!(
            "message has {components} components (limit {COMPONENT_BUDGET})"
        ));
    }
    for row in &msg.rows {
        match row {
            Row::Buttons(buttons) => {
                if buttons.is_empty() || buttons.len() > BUTTONS_PER_ROW {
                    problems.push(format!("a row has {} buttons", buttons.len()));
                }
                for b in buttons {
                    check_button(b, &mut problems);
                }
            }
            Row::Select(select) => {
                over(
                    "select placeholder",
                    chars(&select.placeholder),
                    SELECT_PLACEHOLDER,
                    &mut problems,
                );
                if let SelectOptions::Strings(options) = &select.options {
                    if options.is_empty() || options.len() > SELECT_OPTIONS {
                        problems.push(format!("a select has {} options", options.len()));
                    }
                    for o in options {
                        over(
                            "option label",
                            chars(&o.label),
                            SELECT_OPTION_LABEL,
                            &mut problems,
                        );
                        if let Some(d) = &o.description {
                            over(
                                "option description",
                                chars(d),
                                SELECT_OPTION_DESCRIPTION,
                                &mut problems,
                            );
                        }
                    }
                }
            }
        }
    }
    problems
}

/// Every Discord limit the modal breaks; empty when it fits.
pub fn check_modal(modal: &Modal) -> Vec<String> {
    let mut problems = Vec::new();
    over(
        "modal title",
        chars(&modal.title),
        MODAL_TITLE,
        &mut problems,
    );
    if modal.inputs.is_empty() || modal.inputs.len() > MODAL_INPUTS {
        problems.push(format!("modal has {} inputs", modal.inputs.len()));
    }
    for input in &modal.inputs {
        over(
            "input label",
            chars(&input.label),
            INPUT_LABEL,
            &mut problems,
        );
        if let Some(d) = &input.description {
            over(
                "input description",
                chars(d),
                INPUT_DESCRIPTION,
                &mut problems,
            );
        }
        if let InputKind::Text {
            placeholder,
            min_length,
            max_length,
            value,
            ..
        } = &input.kind
        {
            let max = usize::from(*max_length);
            over("input max length", max, INPUT_MAX_LENGTH, &mut problems);
            if min_length > max_length {
                problems.push("input min length is above its max length".to_owned());
            }
            if let Some(p) = placeholder {
                over(
                    "input placeholder",
                    chars(p),
                    INPUT_PLACEHOLDER,
                    &mut problems,
                );
            }
            if let Some(v) = value {
                over("input value", chars(v), max, &mut problems);
            }
        }
    }
    problems
}

/// Cuts text so its Markdown is at most `max` characters, ending with "…".
/// Atomic spans (mentions, links, times) that don't fit are dropped whole.
pub fn cut_text(t: &Text, max: usize) -> Text {
    let commands = CommandIds::EMPTY;
    if render::markdown(t, &commands).chars().count() <= max {
        return t.clone();
    }
    let mut room = max.saturating_sub(1);
    let mut out = Text::new();
    for inline in &t.0 {
        let len = render::markdown(&Text(vec![inline.clone()]), &commands)
            .chars()
            .count();
        if len <= room {
            out = out.push(inline.clone());
            room -= len;
            continue;
        }
        match inline {
            Inline::Fixed(s) => out = out.s(s.chars().take(room).collect::<String>()),
            Inline::User(s) | Inline::ServerMarkdown(s) => {
                let mut kept = String::new();
                for c in s.chars() {
                    // Escaping adds at most one character.
                    let cost = if c.is_ascii_punctuation() { 2 } else { 1 };
                    if cost > room {
                        break;
                    }
                    room -= cost;
                    kept.push(c);
                }
                out = out.push(match inline {
                    Inline::User(_) => Inline::User(kept),
                    _ => Inline::ServerMarkdown(kept),
                });
            }
            _ => {}
        }
        break;
    }
    out.s("…")
}

/// Shrinks a message that is over the text budget (§11.11): removes
/// subtext, then cuts the longest quote or paragraph, then drops trailing
/// fields and paragraphs (never headings or rows), and says that some
/// details are only on the dashboard.
pub fn fit(msg: &mut Msg) {
    if text_len(msg) <= TEXT_BUDGET {
        return;
    }
    let notice = Block::Text(crate::word::render_trimmed());
    let reserve =
        render::block_markdown(&notice, &CommandIds::EMPTY).map_or(0, |s| s.chars().count() + 1);
    let budget = TEXT_BUDGET - reserve;
    msg.blocks.retain(|b| !matches!(b, Block::Subtext(_)));
    while text_len(msg) > budget {
        let longest = msg
            .blocks
            .iter()
            .enumerate()
            .filter_map(|(i, b)| match b {
                Block::Quote(t) | Block::Text(t) => {
                    Some((i, render::markdown(t, &CommandIds::EMPTY).chars().count()))
                }
                _ => None,
            })
            .max_by_key(|(_, len)| *len);
        let overflow = text_len(msg) - budget;
        match longest {
            Some((i, len)) if len > 200 => {
                let target = len.saturating_sub(overflow).max(200);
                if let Some(Block::Quote(t) | Block::Text(t)) = msg.blocks.get_mut(i) {
                    *t = cut_text(t, target);
                }
            }
            _ => {
                let last = msg.blocks.iter().rposition(|b| {
                    matches!(b, Block::Field { .. } | Block::Text(_) | Block::Quote(_))
                });
                match last {
                    Some(i) => {
                        msg.blocks.remove(i);
                    }
                    None => break,
                }
            }
        }
    }
    msg.blocks.push(notice);
}
