//! The plain types the `Discord` port takes and returns.

use std::fmt;
use std::time::Duration;

use quack_core::action::{CallOutcome, DiscordRefusal};
use quack_core::ids::{AccountId, AttachmentId, ChannelId, GuildId, MessageId};
use quack_core::permissions::{MemberFacts, Overwrite};
use time::OffsetDateTime;

/// The typed result of one Discord call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome<T> {
    /// Discord confirmed it.
    Done(T),
    /// Discord definitely refused; nothing changed.
    Refused(Refusal),
    /// Rate limited before anything was sent; try again after the delay.
    RetryAfter(Duration),
    /// The request was never sent (connection refused, shutting down):
    /// nothing changed.
    NotSent(NotSentCause),
    /// The request may or may not have taken effect (deadline, 5xx,
    /// connection reset after sending).
    Unknown(UnknownCause),
}

impl<T> Outcome<T> {
    /// The outcome without its value, as the action classifier takes it.
    pub fn call(&self) -> CallOutcome {
        match self {
            Self::Done(_) => CallOutcome::Done,
            Self::Refused(r) => CallOutcome::Refused(r.discord()),
            Self::RetryAfter(d) => {
                CallOutcome::RetryAfter(time::Duration::try_from(*d).unwrap_or(time::Duration::MAX))
            }
            Self::NotSent(_) => CallOutcome::NotSent,
            Self::Unknown(_) => CallOutcome::Unknown,
        }
    }

    /// The value when Discord confirmed the call.
    pub fn done(self) -> Option<T> {
        match self {
            Self::Done(value) => Some(value),
            _ => None,
        }
    }

    /// Maps the confirmed value.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Outcome<U> {
        match self {
            Self::Done(value) => Outcome::Done(f(value)),
            Self::Refused(r) => Outcome::Refused(r),
            Self::RetryAfter(d) => Outcome::RetryAfter(d),
            Self::NotSent(c) => Outcome::NotSent(c),
            Self::Unknown(c) => Outcome::Unknown(c),
        }
    }

    /// A short machine code for logs and `last_error`: never Discord's
    /// error text.
    pub fn code(&self) -> String {
        match self {
            Self::Done(_) => "done".to_owned(),
            Self::Refused(r) => quack_core::messages::sync_error_code(r.status, r.code),
            Self::RetryAfter(_) => "rate_limited".to_owned(),
            Self::NotSent(_) => "not_sent".to_owned(),
            Self::Unknown(UnknownCause::ServerError(status)) => format!("http_{status}"),
            Self::Unknown(_) => "unknown".to_owned(),
        }
    }
}

/// A definite refusal: Discord's HTTP status and JSON error code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Refusal {
    /// HTTP status (a 4xx other than 429).
    pub status: u16,
    /// Discord's JSON error code, when the body had one.
    pub code: Option<u32>,
    /// What it means.
    pub kind: RefusalKind,
}

impl Refusal {
    /// A refusal of this kind with its usual status and code.
    pub const fn of(kind: RefusalKind) -> Self {
        let (status, code) = match kind {
            RefusalKind::MissingPermissions => (403, Some(50_013)),
            RefusalKind::MissingAccess => (403, Some(50_001)),
            RefusalKind::CannotDm => (403, Some(50_007)),
            RefusalKind::NoMutualGuilds => (403, Some(50_278)),
            RefusalKind::UnknownGuild => (404, Some(10_004)),
            RefusalKind::UnknownMember => (404, Some(10_007)),
            RefusalKind::UnknownUser => (404, Some(10_013)),
            RefusalKind::UnknownBan => (404, Some(10_026)),
            RefusalKind::UnknownMessage => (404, Some(10_008)),
            RefusalKind::UnknownChannel => (404, Some(10_003)),
            RefusalKind::TooLarge => (413, Some(40_005)),
            RefusalKind::Other => (400, None),
        };
        Self { status, code, kind }
    }

    /// The refusal as the domain's action classifier takes it.
    pub const fn discord(self) -> DiscordRefusal {
        DiscordRefusal {
            status: self.status,
            code: self.code,
        }
    }
}

/// What a refusal means (§17.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefusalKind {
    /// 403, 50013.
    MissingPermissions,
    /// 403, 50001.
    MissingAccess,
    /// 403, 50007.
    CannotDm,
    /// 50278.
    NoMutualGuilds,
    /// 404, 10004.
    UnknownGuild,
    /// 404, 10007.
    UnknownMember,
    /// 404, 10013.
    UnknownUser,
    /// 404, 10026.
    UnknownBan,
    /// 404, 10008.
    UnknownMessage,
    /// 404, 10003.
    UnknownChannel,
    /// 413, 40005.
    TooLarge,
    /// Any other 4xx except 429.
    Other,
}

/// Why a request was never sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotSentCause {
    /// The connection could not be made.
    Connection,
    /// The client is shutting down.
    ShuttingDown,
}

/// Why a request's result is unknown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnknownCause {
    /// The deadline passed.
    Timeout,
    /// Discord answered 5xx.
    ServerError(u16),
    /// The connection broke after sending.
    Connection,
}

/// A member lookup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Membership {
    /// In the server.
    Present(MemberFacts),
    /// Discord answered 404 Unknown Member.
    Absent,
}

impl Membership {
    /// The member, when present.
    pub const fn present(&self) -> Option<&MemberFacts> {
        match self {
            Self::Present(m) => Some(m),
            Self::Absent => None,
        }
    }
}

/// Where a fact read may come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fresh {
    /// The member cache under §2's rules, else REST.
    Cached,
    /// Always REST (action reads, §5.1).
    Rest,
}

/// What a create or edit returned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SentMessage {
    /// Where the message is.
    pub channel_id: ChannelId,
    /// The message.
    pub message_id: MessageId,
    /// The attachment ids Discord assigned, by file name.
    pub attachments: Vec<(String, AttachmentId)>,
}

/// A file to attach.
#[derive(Clone, PartialEq, Eq)]
pub struct OutgoingFile {
    /// The file name Discord shows.
    pub name: String,
    /// Its content type, when known.
    pub content_type: Option<String>,
    /// The bytes.
    pub bytes: Vec<u8>,
}

impl fmt::Debug for OutgoingFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OutgoingFile")
            .field("name", &self.name)
            .field("bytes", &self.bytes.len())
            .finish_non_exhaustive()
    }
}

/// Files on an edit: none, keep the existing attachment, or replace it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileEdit {
    /// The message has no file.
    None,
    /// Keep the attachment the message already has.
    Keep(AttachmentId),
    /// Attach this file instead.
    Replace(OutgoingFile),
}

/// A message create's nonce: at most 25 characters, derived from the
/// record's id so it never changes across retries (§10.4).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Nonce(String);

impl Nonce {
    /// Wraps a nonce, cut to Discord's 25 characters.
    pub fn new(nonce: impl Into<String>) -> Self {
        let nonce: String = nonce.into();
        Self(nonce.chars().take(25).collect())
    }

    /// The nonce text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// An interaction token. Secret: never logged; `Debug` prints
/// `[redacted]`.
#[derive(Clone, PartialEq, Eq)]
pub struct InteractionToken(String);

impl InteractionToken {
    /// Wraps a token.
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }

    /// The token, for the adapter's request only.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for InteractionToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("InteractionToken([redacted])")
    }
}

/// A message as Discord returned it (evidence capture, file routes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchedMessage {
    /// The message.
    pub id: MessageId,
    /// Its channel or thread.
    pub channel_id: ChannelId,
    /// Its server; `None` in DMs.
    pub guild_id: Option<GuildId>,
    /// Its author.
    pub author_id: AccountId,
    /// The author's display name.
    pub author_name: String,
    /// Whether the author is a bot.
    pub author_bot: bool,
    /// Its text.
    pub content: String,
    /// When it was sent.
    pub created_at: OffsetDateTime,
    /// When it was last edited.
    pub edited_at: Option<OffsetDateTime>,
    /// Its embeds.
    pub embeds: Vec<FetchedEmbed>,
    /// Its files.
    pub attachments: Vec<FetchedAttachment>,
}

/// One embed of a fetched message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchedEmbed {
    /// The embed type (`rich`, `image`, …).
    pub kind: String,
    /// Its title.
    pub title: Option<String>,
    /// Its description.
    pub description: Option<String>,
    /// Its URL.
    pub url: Option<String>,
}

/// One file of a fetched message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchedAttachment {
    /// Discord's attachment id.
    pub id: AttachmentId,
    /// The file name.
    pub filename: String,
    /// Its content type, when Discord gave one.
    pub content_type: Option<String>,
    /// Its size.
    pub size_bytes: u64,
    /// Its CDN URL.
    pub url: String,
}

/// A channel to create (setup, §13.1.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelSpec {
    /// Its name.
    pub name: String,
    /// Its topic.
    pub topic: Option<String>,
    /// Its permission overwrites.
    pub overwrites: Vec<Overwrite>,
}
