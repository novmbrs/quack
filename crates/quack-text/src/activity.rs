//! Server activity log posts (§12.13): message edits and deletions, members
//! joining, leaving, and being banned, and server changes. Message text has
//! already been redacted (§13.3.2) and is escaped like any typed text.

use quack_core::ids::{AccountId, MessageId};
use time::OffsetDateTime;

use crate::fmt;
use crate::model::{Actor, Block, ChannelRef, Msg, Person, Text, TimeStyle, Tone};

/// Excerpts a bulk-delete post shows at most.
pub const BULK_ROWS: usize = 20;
/// Characters of each bulk-delete excerpt, so 20 rows always fit the text
/// budget even when every character needs escaping.
pub const BULK_EXCERPT: usize = 70;
/// Message ids a post lists at most.
pub const MAX_IDS: usize = 50;

/// What is known about a message's text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content {
    /// The text.
    Text(String),
    /// The message has no text.
    Empty,
    /// Quack hadn't seen it.
    Unknown,
    /// The server's activity log hides message text.
    Hidden,
}

/// One file of a message: its name and original URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileLink {
    /// The file name.
    pub name: String,
    /// Its original URL.
    pub url: String,
}

/// A message an activity post is about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageFacts {
    /// Who wrote it.
    pub author: Person,
    /// Where.
    pub channel: ChannelRef,
    /// Its id.
    pub id: MessageId,
    /// When the event happened.
    pub at: OffsetDateTime,
    /// Its files (empty when the server hides them).
    pub files: Vec<FileLink>,
    /// Its link preview types or titles (empty when hidden).
    pub embeds: Vec<String>,
}

/// One activity event to post.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActivityPost {
    /// A message's text or files changed.
    MessageEdited {
        /// The message after the edit.
        message: MessageFacts,
        /// The text before.
        before: Content,
        /// The text after.
        after: Content,
    },
    /// A message Quack had seen was deleted.
    MessageDeleted {
        /// The message.
        message: MessageFacts,
        /// Its text.
        content: Content,
    },
    /// A message Quack hadn't seen was deleted.
    MessageDeletedUnknown {
        /// Where.
        channel: ChannelRef,
        /// Its id.
        id: MessageId,
        /// When.
        at: OffsetDateTime,
    },
    /// Several messages Quack hadn't seen were deleted.
    MessagesDeletedUnknown {
        /// Where.
        channel: ChannelRef,
        /// Their ids.
        ids: Vec<MessageId>,
        /// When.
        at: OffsetDateTime,
    },
    /// Messages were deleted at once.
    MessagesBulkDeleted {
        /// Where.
        channel: ChannelRef,
        /// Their ids.
        ids: Vec<MessageId>,
        /// The ones Quack had seen: author and text.
        known: Vec<(Person, Content)>,
        /// When.
        at: OffsetDateTime,
    },
    /// A member joined.
    MemberJoined {
        /// Who.
        member: Person,
        /// When their account was created.
        account_created: OffsetDateTime,
    },
    /// A member left.
    MemberLeft {
        /// Their username.
        username: String,
        /// Their account.
        id: AccountId,
    },
    /// Someone other than Quack banned a member.
    MemberBanned {
        /// Who was banned.
        member: Person,
        /// Who banned them.
        by: Actor,
        /// The reason they gave.
        reason: Option<String>,
    },
    /// Someone other than Quack unbanned a member.
    MemberUnbanned {
        /// Who was unbanned.
        member: Person,
        /// Who unbanned them.
        by: Actor,
        /// The reason they gave.
        reason: Option<String>,
    },
    /// The server's settings changed.
    ServerChanged,
    /// A channel was created.
    ChannelCreated(ChannelRef),
    /// A channel's settings changed.
    ChannelChanged(ChannelRef),
    /// A channel was deleted; its name.
    ChannelDeleted(String),
}

fn content(label: &str, c: &Content, after: bool) -> Block {
    match c {
        Content::Text(t) => Block::Quote(Text::new().user(t)),
        Content::Empty if after => Block::Field {
            label: label.to_owned(),
            value: Text::fixed("The message has no text."),
        },
        Content::Empty => Block::Field {
            label: label.to_owned(),
            value: Text::fixed("The message had no text."),
        },
        Content::Unknown if after => Block::Field {
            label: label.to_owned(),
            value: Text::fixed("Discord didn't include the new text."),
        },
        Content::Unknown => Block::Field {
            label: label.to_owned(),
            value: Text::fixed("Quack hadn't seen the earlier text."),
        },
        Content::Hidden => Block::Text(text_hidden()),
    }
}

/// `activity.text_hidden`.
pub fn text_hidden() -> Text {
    Text::fixed("This server's activity log hides message text.")
}

fn with_label(msg: Msg, label: &str, c: &Content, after: bool) -> Msg {
    match c {
        Content::Text(_) => msg
            .para(Text::new().bold(format!("{label}:")))
            .block(content(label, c, after)),
        Content::Empty | Content::Unknown | Content::Hidden => msg.block(content(label, c, after)),
    }
}

fn attachments(mut msg: Msg, m: &MessageFacts) -> Msg {
    if !m.files.is_empty() {
        let mut t = Text::fixed("Files: ");
        for (i, f) in m.files.iter().enumerate() {
            if i > 0 {
                t = t.s(", ");
            }
            t = t.push(crate::model::Inline::Link {
                text: vec![crate::model::Inline::User(fmt::one_line(&f.name))],
                url: f.url.clone(),
            });
        }
        msg = msg.para(t);
    }
    if !m.embeds.is_empty() {
        let names: Vec<String> = m.embeds.iter().map(|e| fmt::one_line(e)).collect();
        msg = msg.para(Text::fixed("Link previews: ").user(names.join(", ")));
    }
    msg
}

fn subtext(id: MessageId, at: OffsetDateTime) -> Block {
    Block::Subtext(Text::fixed(format!("Message ID {id} · ")).time(at, TimeStyle::Full))
}

fn ids(list: &[MessageId]) -> Block {
    let shown: Vec<String> = list.iter().take(MAX_IDS).map(ToString::to_string).collect();
    Block::Subtext(Text::fixed(format!(
        "Deleted message IDs: {}",
        shown.join(", ")
    )))
}

fn reason(msg: Msg, reason: Option<&String>) -> Msg {
    match reason {
        Some(r) => msg
            .para(Text::new().bold("Reason:"))
            .block(Block::Quote(Text::new().user(r))),
        None => msg,
    }
}

fn bulk_deleted(
    msg: Msg,
    channel: &ChannelRef,
    list: &[MessageId],
    known: &[(Person, Content)],
    at: OffsetDateTime,
) -> Msg {
    // The counts come first and say how many excerpts follow; excerpts are
    // short enough that the rows shown always fit (§11.11).
    let shown = known.len().min(BULK_ROWS);
    let lead = Text::fixed(format!(
        "{} messages were deleted in ",
        fmt::count(list.len() as u64)
    ))
    .channel(channel)
    .s(".");
    let lead = match known.len() {
        0 => {
            lead.s(" Quack hadn't seen these messages, so their authors and text are unavailable.")
        }
        n if n == shown => lead.s(format!(" Quack had seen {} of them:", fmt::count(n as u64))),
        n => lead.s(format!(
            " Quack had seen {} of them. Showing excerpts from {}:",
            fmt::count(n as u64),
            fmt::count(shown as u64)
        )),
    };
    let mut msg = msg.para(lead);
    for (author, c) in known.iter().take(BULK_ROWS) {
        let row = Text::new().person(author).s(": ");
        msg = msg.para(match c {
            Content::Text(t) => row.user(fmt::cut(&fmt::one_line(t), BULK_EXCERPT)),
            Content::Empty => row.s("(no text)"),
            Content::Unknown | Content::Hidden => row.s("(text not shown)"),
        });
    }
    msg.block(ids(list))
        .block(Block::Subtext(Text::new().time(at, TimeStyle::Full)))
}

fn edited(msg: Msg, message: &MessageFacts, before: &Content, after: &Content) -> Msg {
    let msg = msg.para(
        Text::fixed("A message from ")
            .person(&message.author)
            .s(" was edited in ")
            .channel(&message.channel)
            .s("."),
    );
    let msg = match (before, after) {
        (Content::Hidden, Content::Hidden) => msg.para(text_hidden()),
        _ => with_label(
            with_label(msg, "Before", before, false),
            "After",
            after,
            true,
        ),
    };
    attachments(msg, message).block(subtext(message.id, message.at))
}

fn deleted(msg: Msg, message: &MessageFacts, c: &Content) -> Msg {
    let msg = msg.para(
        Text::fixed("A message from ")
            .person(&message.author)
            .s(" was deleted in ")
            .channel(&message.channel)
            .s("."),
    );
    let msg = match c {
        Content::Text(_) | Content::Hidden => msg.block(content("Text", c, false)),
        Content::Empty | Content::Unknown => msg,
    };
    attachments(msg, message).block(subtext(message.id, message.at))
}

/// `activity.*`: the post for one event.
pub fn post(p: &ActivityPost) -> Msg {
    // There is no dashboard history of activity posts (§13.3), so a
    // shortened post says so instead of pointing anywhere.
    let msg = Msg::new(Tone::Neutral).when_trimmed(Text::fixed(
        "This post shows excerpts. Quack shortened some message text to fit in Discord.",
    ));
    match p {
        ActivityPost::MessageEdited {
            message,
            before,
            after,
        } => edited(msg, message, before, after),
        ActivityPost::MessageDeleted {
            message,
            content: c,
        } => deleted(msg, message, c),
        ActivityPost::MessageDeletedUnknown { channel, id, at } => msg
            .para(
                Text::fixed("A message was deleted in ")
                    .channel(channel)
                    .s(". Quack hadn't seen it, so its author and text are unavailable."),
            )
            .block(subtext(*id, *at)),
        ActivityPost::MessagesDeletedUnknown {
            channel,
            ids: list,
            at,
        } => msg
            .para(
                Text::fixed(format!(
                    "{} messages were deleted in ",
                    fmt::count(list.len() as u64)
                ))
                .channel(channel)
                .s(". Quack hadn't seen these messages, so their authors and text are unavailable."),
            )
            .block(ids(list))
            .block(Block::Subtext(Text::new().time(*at, TimeStyle::Full))),
        ActivityPost::MessagesBulkDeleted {
            channel,
            ids: list,
            known,
            at,
        } => bulk_deleted(msg, channel, list, known, *at),
        ActivityPost::MemberJoined {
            member,
            account_created,
        } => msg.para(
            Text::new()
                .person(member)
                .s(" joined the server. Their account was created ")
                .time(*account_created, TimeStyle::Relative)
                .s("."),
        ),
        ActivityPost::MemberLeft { username, id } => msg.para(
            Text::new()
                .bold_user(fmt::one_line(username))
                .s(format!(" left the server. Discord member ID: {id}.")),
        ),
        ActivityPost::MemberBanned {
            member,
            by,
            reason: r,
        } => reason(
            msg.para(
                Text::new()
                    .person(member)
                    .s(" was banned by ")
                    .actor(by)
                    .s("."),
            ),
            r.as_ref(),
        ),
        ActivityPost::MemberUnbanned {
            member,
            by,
            reason: r,
        } => reason(
            msg.para(
                Text::new()
                    .person(member)
                    .s(" was unbanned by ")
                    .actor(by)
                    .s("."),
            ),
            r.as_ref(),
        ),
        ActivityPost::ServerChanged => msg.para(Text::fixed("The server settings changed.")),
        ActivityPost::ChannelCreated(c) => {
            msg.para(Text::fixed("The channel ").channel(c).s(" was created."))
        }
        ActivityPost::ChannelChanged(c) => {
            msg.para(Text::fixed("Settings changed for ").channel(c).s("."))
        }
        ActivityPost::ChannelDeleted(name) => msg.para(
            Text::fixed("The channel #")
                .user(fmt::one_line(name))
                .s(" was deleted."),
        ),
    }
}
