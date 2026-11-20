//! Rendering the model to Discord Markdown or to plain text.
//!
//! Discord: people, channels, and roles become mentions (sent with empty
//! `allowed_mentions`, so they never ping), times become Discord timestamps,
//! and every [`Inline::User`] span is escaped. Plain text (API messages,
//! audit-log reasons, the dashboard's `text` fields, tests): people become
//! display names, times become UTC dates, and Markdown is dropped.

use std::fmt::Write as _;

use crate::fmt;
use crate::model::{Block, Command, Inline, Msg, Text, TimeStyle};

/// Command ids Discord returned at registration (§11.2), for clickable
/// command mentions. Unknown commands render as `/name` in code.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandIds(pub Vec<(Command, u64)>);

impl CommandIds {
    /// No ids known.
    pub const EMPTY: Self = Self(Vec::new());

    fn get(&self, command: Command) -> Option<u64> {
        self.0
            .iter()
            .find(|(c, _)| *c == command)
            .map(|(_, id)| *id)
    }
}

/// Where text is shown.
#[derive(Clone, Copy, Debug)]
pub enum Surface<'a> {
    /// A Discord message: mentions and Markdown.
    Discord(&'a CommandIds),
    /// Plain text: names and no Markdown.
    Plain,
}

/// What plain text says for someone or something Quack can't name; the ID
/// stays as a labelled detail for staff.
const FALLBACKS: [&str; 3] = [
    "a member (ID ",
    "an unnamed role (ID ",
    "an unknown channel (ID ",
];

/// Renders text for a surface.
pub fn text(t: &Text, surface: Surface<'_>) -> String {
    let mut out = String::new();
    for inline in &t.0 {
        inline_to(&mut out, inline, surface);
    }
    if matches!(surface, Surface::Plain) {
        // A fallback that starts a sentence starts with a capital.
        for f in FALLBACKS {
            let upper = fmt::upper_first(f);
            if out.starts_with(f) {
                out.replace_range(..f.len(), &upper);
            }
            for sep in [". ", "\n", ": "] {
                out = out.replace(&format!("{sep}{f}"), &format!("{sep}{upper}"));
            }
        }
    }
    out
}

/// Renders text as plain text.
pub fn plain(t: &Text) -> String {
    text(t, Surface::Plain)
}

/// Renders text as Discord Markdown.
pub fn markdown(t: &Text, commands: &CommandIds) -> String {
    text(t, Surface::Discord(commands))
}

fn inline_to(out: &mut String, inline: &Inline, surface: Surface<'_>) {
    let discord = matches!(surface, Surface::Discord(_));
    match inline {
        Inline::Fixed(s) => out.push_str(s),
        Inline::User(s) if discord => out.push_str(&fmt::escape_markdown(s)),
        Inline::ServerMarkdown(s) if discord => out.push_str(&fmt::defuse_mentions(s)),
        Inline::User(s) | Inline::ServerMarkdown(s) => out.push_str(&fmt::clean(s)),
        Inline::Bold(inner) => {
            if discord {
                out.push_str("**");
            }
            for i in inner {
                inline_to(out, i, surface);
            }
            if discord {
                out.push_str("**");
            }
        }
        Inline::Code(s) => {
            if discord {
                let _ = write!(out, "`{}`", fmt::clean(s).replace('`', "'"));
            } else {
                out.push_str(&fmt::clean(s));
            }
        }
        Inline::Link { text, url } => {
            let safe = fmt::safe_url(url).filter(|_| discord);
            if safe.is_some() {
                out.push('[');
            }
            for i in text {
                inline_to(out, i, surface);
            }
            if let Some(url) = safe {
                let _ = write!(out, "](<{url}>)");
            }
        }
        Inline::Person(p) if discord => {
            let _ = write!(out, "<@{}>", p.id);
        }
        Inline::Person(p) => match &p.name {
            Some(name) => out.push_str(&fmt::one_line(name)),
            None => {
                let _ = write!(out, "{}{})", FALLBACKS[0], p.id);
            }
        },
        Inline::Channel(c) if discord => {
            let _ = write!(out, "<#{}>", c.id);
        }
        Inline::Channel(c) => match &c.name {
            Some(name) => {
                out.push('#');
                out.push_str(&fmt::one_line(name));
            }
            None => {
                let _ = write!(out, "{}{})", FALLBACKS[2], c.id);
            }
        },
        Inline::Role(r) if discord => {
            let _ = write!(out, "<@&{}>", r.id);
        }
        Inline::Role(r) => match &r.name {
            Some(name) => {
                out.push('@');
                out.push_str(&fmt::one_line(name));
            }
            None => {
                let _ = write!(out, "{}{})", FALLBACKS[1], r.id);
            }
        },
        Inline::Command(c) => match surface {
            Surface::Discord(ids) => match ids.get(*c) {
                Some(id) => {
                    let _ = write!(out, "</{}:{id}>", c.path());
                }
                None => {
                    let _ = write!(out, "`/{}`", c.path());
                }
            },
            Surface::Plain => {
                let _ = write!(out, "/{}", c.path());
            }
        },
        Inline::Time(at, style) => out.push_str(&time(*at, *style, discord)),
    }
}

fn time(at: time::OffsetDateTime, style: TimeStyle, discord: bool) -> String {
    let unix = at.unix_timestamp();
    match (style, discord) {
        (TimeStyle::Full, true) => format!("<t:{unix}:f>"),
        (TimeStyle::Relative, true) => format!("<t:{unix}:R>"),
        (TimeStyle::FullRelative, true) => format!("<t:{unix}:f> (<t:{unix}:R>)"),
        (TimeStyle::Date, true) => format!("<t:{unix}:d>"),
        (TimeStyle::Full | TimeStyle::FullRelative, false) => fmt::datetime(at),
        (TimeStyle::Relative, false) => format!("on {}", fmt::datetime(at)),
        (TimeStyle::Date, false) => fmt::date(at),
    }
}

/// Renders one block as the Markdown of its Text Display (§11.11), or
/// `None` for blocks that are not text (dividers, files).
pub fn block_markdown(block: &Block, commands: &CommandIds) -> Option<String> {
    block_to(block, Surface::Discord(commands))
}

fn block_to(block: &Block, surface: Surface<'_>) -> Option<String> {
    let discord = matches!(surface, Surface::Discord(_));
    Some(match block {
        // A heading is one line, even when a name in it has line breaks.
        Block::Heading(t) if discord => format!("### {}", fmt::one_line(&text(t, surface))),
        Block::Heading(t) => fmt::one_line(&text(t, surface)),
        Block::Subtext(t) if discord => format!("-# {}", text(t, surface)),
        Block::Text(t) | Block::Subtext(t) => text(t, surface),
        Block::Quote(t) => quote(&text(t, surface)),
        Block::Field { label, value } => field(label, &text(value, surface), discord),
        Block::Divider | Block::File(_) | Block::Gallery(_) => return None,
    })
}

/// `**Label:** value`. Labels can come from a rule's fields, so they are
/// escaped like any typed text; a question label ("What happened?") gets
/// no colon and its answer on the next line.
fn field(label: &str, value: &str, discord: bool) -> String {
    let label = fmt::one_line(label);
    let question = label.ends_with('?');
    match (discord, question) {
        (true, true) => format!("**{}**\n{value}", fmt::escape_markdown(&label)),
        (true, false) => format!("**{}:** {value}", fmt::escape_markdown(&label)),
        (false, true) => format!("{label} {value}"),
        (false, false) => format!("{label}: {value}"),
    }
}

fn quote(s: &str) -> String {
    s.lines()
        .map(|line| format!("> {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Renders a whole message as plain text: one line per text block, a blank
/// line for each divider, and the file name for each file.
pub fn plain_msg(msg: &Msg) -> String {
    msg.blocks
        .iter()
        .map(|b| match b {
            Block::Divider => String::new(),
            Block::File(name) => format!("[file: {}]", fmt::clean(name)),
            Block::Gallery(items) => items
                .iter()
                .map(|i| match &i.description {
                    Some(d) => format!("[media: {}]", fmt::clean(d)),
                    None => "[media]".to_owned(),
                })
                .collect::<Vec<_>>()
                .join(" "),
            other => block_to(other, Surface::Plain).unwrap_or_default(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The Markdown of every text block, joined as Discord shows them; used for
/// the text budget and in tests.
pub fn msg_markdown(msg: &Msg, commands: &CommandIds) -> String {
    msg.blocks
        .iter()
        .filter_map(|b| block_markdown(b, commands))
        .collect::<Vec<_>>()
        .join("\n")
}
