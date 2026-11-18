//! The neutral message model every message function returns.
//!
//! It holds only the shapes Quack's messages use (§11.11): text blocks with
//! a tiny inline markup, buttons, selects, modals, and file references.
//! `quack-discord` turns a [`Msg`] into a Components V2 message; the API and
//! audit-log reasons use [`Text::plain`] and [`Msg::plain`].
//!
//! Text a person typed or Discord returned is always an [`Inline::User`]
//! span, which the Discord renderer escapes, so it can never become
//! Markdown, a mention, or a link.

mod msg;

use quack_core::ids::{AccountId, ChannelId, RoleId};
use time::OffsetDateTime;

pub use msg::{
    Block, Button, ButtonStyle, ButtonTarget, Input, InputKind, InputStyle, Modal, Msg, Row,
    Select, SelectOption, SelectOptions, Tone,
};

/// Someone shown by name: a member, a moderator, or a message author.
///
/// Discord renders a mention (which never pings); plain text renders the
/// display name (server nickname, else global name, else username), or the
/// id when no name is known.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Person {
    /// Their Discord account.
    pub id: AccountId,
    /// Their display name, when known.
    pub name: Option<String>,
}

impl Person {
    /// Someone with a known display name.
    pub fn new(id: AccountId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: Some(name.into()),
        }
    }

    /// Someone whose display name is not known.
    pub const fn unnamed(id: AccountId) -> Self {
        Self { id, name: None }
    }
}

/// Who did something: a staff member or Quack itself (`{actor}` when NULL).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Actor {
    /// Quack acted on its own.
    Quack,
    /// A person acted.
    Person(Person),
    /// A moderator imported history didn't name ("a moderator").
    UnknownModerator,
}

/// A channel or thread, shown as a mention or `#name`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelRef {
    /// The channel.
    pub id: ChannelId,
    /// Its name without `#`, when known.
    pub name: Option<String>,
}

impl ChannelRef {
    /// A channel with a known name.
    pub fn new(id: ChannelId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: Some(name.into()),
        }
    }
}

/// A role, shown as a mention or `@name`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoleRef {
    /// The role.
    pub id: RoleId,
    /// Its name, when known.
    pub name: Option<String>,
}

/// A slash command that text can point at. Discord renders a clickable
/// command mention when the command's id is known (§11.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Command {
    /// `/case add`.
    CaseAdd,
    /// `/case view`.
    CaseView,
    /// `/case list`.
    CaseList,
    /// `/case void`.
    CaseVoid,
    /// `/case history`.
    CaseHistory,
    /// `/appeals`.
    Appeals,
    /// `/rules`.
    Rules,
    /// `/setup`.
    Setup,
    /// `/help`.
    Help,
}

impl Command {
    /// The top-level command name Discord registers.
    pub const fn top(self) -> &'static str {
        match self {
            Self::CaseAdd
            | Self::CaseView
            | Self::CaseList
            | Self::CaseVoid
            | Self::CaseHistory => "case",
            Self::Appeals => "appeals",
            Self::Rules => "rules",
            Self::Setup => "setup",
            Self::Help => "help",
        }
    }

    /// The full name as typed, without the slash (`case view`).
    pub const fn path(self) -> &'static str {
        match self {
            Self::CaseAdd => "case add",
            Self::CaseView => "case view",
            Self::CaseList => "case list",
            Self::CaseVoid => "case void",
            Self::CaseHistory => "case history",
            Self::Appeals => "appeals",
            Self::Rules => "rules",
            Self::Setup => "setup",
            Self::Help => "help",
        }
    }
}

/// How a timestamp is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeStyle {
    /// Date and time (`<t:unix:f>`).
    Full,
    /// Relative ("3 hours ago", `<t:unix:R>`); plain text says "on <date> at
    /// <time> UTC" so the sentence still reads.
    Relative,
    /// Date and time followed by the relative time (`{ends_at}`).
    FullRelative,
    /// The date only (`<t:unix:d>`).
    Date,
}

/// One span of inline text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inline {
    /// Quack's own words from this crate, rendered as written. Never holds
    /// anything a person typed.
    Fixed(String),
    /// Text a person typed or Discord returned (names, reasons, answers).
    /// Escaped on Discord so it cannot become Markdown, mentions, or links.
    User(String),
    /// Markdown a server's staff wrote for members (DM introduction and
    /// footer, honeypot warning): formatting is kept, mentions are not.
    ServerMarkdown(String),
    /// Bold text.
    Bold(Vec<Inline>),
    /// Inline code.
    Code(String),
    /// A link with Quack-built text and a URL from Quack or Discord.
    Link {
        /// The link text.
        text: Vec<Inline>,
        /// An `https://` URL.
        url: String,
    },
    /// A person.
    Person(Person),
    /// A channel or thread.
    Channel(ChannelRef),
    /// A role.
    Role(RoleRef),
    /// A slash command.
    Command(Command),
    /// A point in time.
    Time(OffsetDateTime, TimeStyle),
}

/// A run of inline spans: one sentence, line, or paragraph. Line breaks are
/// `\n` inside [`Inline::Fixed`] spans.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Text(pub Vec<Inline>);

impl Text {
    /// Empty text.
    pub const fn new() -> Self {
        Self(Vec::new())
    }

    /// Quack's own words.
    pub fn fixed(s: impl Into<String>) -> Self {
        Self::new().s(s)
    }

    /// Whether there is nothing to show.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Appends Quack's own words.
    #[must_use]
    pub fn s(mut self, s: impl Into<String>) -> Self {
        let s = s.into();
        if let Some(Inline::Fixed(last)) = self.0.last_mut() {
            last.push_str(&s);
        } else if !s.is_empty() {
            self.0.push(Inline::Fixed(s));
        }
        self
    }

    /// Appends text a person typed.
    #[must_use]
    pub fn user(mut self, s: impl Into<String>) -> Self {
        self.0.push(Inline::User(s.into()));
        self
    }

    /// Appends text a person typed, in bold (`{rule}`, `{server}`).
    #[must_use]
    pub fn bold_user(mut self, s: impl Into<String>) -> Self {
        self.0.push(Inline::Bold(vec![Inline::User(s.into())]));
        self
    }

    /// Appends Quack's own words in bold.
    #[must_use]
    pub fn bold(mut self, s: impl Into<String>) -> Self {
        self.0.push(Inline::Bold(vec![Inline::Fixed(s.into())]));
        self
    }

    /// Appends any span.
    #[must_use]
    pub fn push(mut self, inline: Inline) -> Self {
        self.0.push(inline);
        self
    }

    /// Appends another text.
    #[must_use]
    pub fn then(mut self, other: Self) -> Self {
        for inline in other.0 {
            self = match inline {
                Inline::Fixed(s) => self.s(s),
                other => self.push(other),
            };
        }
        self
    }

    /// Appends another text after a space, when it is not empty.
    #[must_use]
    pub fn sentence(self, other: Self) -> Self {
        if other.is_empty() {
            self
        } else if self.is_empty() {
            other
        } else {
            self.s(" ").then(other)
        }
    }

    /// Appends a person.
    #[must_use]
    pub fn person(self, p: &Person) -> Self {
        self.push(Inline::Person(p.clone()))
    }

    /// Appends an actor: a person, or "Quack".
    #[must_use]
    pub fn actor(self, a: &Actor) -> Self {
        match a {
            Actor::Quack => self.s("Quack"),
            Actor::Person(p) => self.person(p),
            Actor::UnknownModerator => self.s("a moderator"),
        }
    }

    /// Appends a channel.
    #[must_use]
    pub fn channel(self, c: &ChannelRef) -> Self {
        self.push(Inline::Channel(c.clone()))
    }

    /// Appends a role.
    #[must_use]
    pub fn role(self, r: &RoleRef) -> Self {
        self.push(Inline::Role(r.clone()))
    }

    /// Appends a command.
    #[must_use]
    pub fn command(self, c: Command) -> Self {
        self.push(Inline::Command(c))
    }

    /// Appends a timestamp.
    #[must_use]
    pub fn time(self, at: OffsetDateTime, style: TimeStyle) -> Self {
        self.push(Inline::Time(at, style))
    }

    /// Appends a link whose text is Quack's own words.
    #[must_use]
    pub fn link(self, text: impl Into<String>, url: impl Into<String>) -> Self {
        self.push(Inline::Link {
            text: vec![Inline::Fixed(text.into())],
            url: url.into(),
        })
    }

    /// The text as one line of plain text (see [`crate::render`]).
    pub fn plain(&self) -> String {
        crate::render::plain(self)
    }
}
