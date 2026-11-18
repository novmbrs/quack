//! The plain Discord call types shared by `quack-discord` and the engine's
//! `Discord` port: the typed outcome of one call and the payloads both
//! sides pass through unchanged (§17.3).
//!
//! Getting the outcome right is what makes retries safe: `Refused` means
//! Discord definitely changed nothing, `NotSent` means the request never
//! left Quack, and `Unknown` means it may have taken effect (§5.2, §6.4,
//! §10.4).

use std::fmt;
use std::time::Duration;

use crate::action::{CallOutcome, DiscordRefusal};
use crate::ids::{AttachmentId, ChannelId, MessageId};
use crate::permissions::MemberFacts;

/// The typed result of one Discord call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome<T> {
    /// Discord confirmed it.
    Done(T),
    /// Discord definitely refused; nothing changed.
    Refused(Refusal),
    /// Rate limited before anything was sent; try again after the delay.
    RetryAfter(Duration),
    /// The request was never sent: nothing changed.
    NotSent(NotSentCause),
    /// The request may or may not have taken effect (deadline, 5xx,
    /// connection reset after sending).
    Unknown(UnknownCause),
}

impl<T> Outcome<T> {
    /// Maps the confirmed value.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Outcome<U> {
        self.and_then(|v| Outcome::Done(f(v)))
    }

    /// Chains a second step that runs only when this one is `Done`.
    pub fn and_then<U>(self, f: impl FnOnce(T) -> Outcome<U>) -> Outcome<U> {
        match self.into_result() {
            Ok(v) => f(v),
            Err(failure) => failure,
        }
    }

    /// The confirmed value, or the same failure for another value type, so
    /// a multi-step call can stop at its first failure.
    pub fn into_result<U>(self) -> Result<T, Outcome<U>> {
        match self {
            Self::Done(v) => Ok(v),
            Self::Refused(r) => Err(Outcome::Refused(r)),
            Self::RetryAfter(d) => Err(Outcome::RetryAfter(d)),
            Self::NotSent(c) => Err(Outcome::NotSent(c)),
            Self::Unknown(c) => Err(Outcome::Unknown(c)),
        }
    }

    /// The confirmed value, if any.
    pub fn done(self) -> Option<T> {
        self.into_result::<()>().ok()
    }

    /// Whether Discord confirmed it.
    pub const fn is_done(&self) -> bool {
        matches!(self, Self::Done(_))
    }

    /// The refusal kind, when Discord refused.
    pub const fn refusal(&self) -> Option<RefusalKind> {
        match self {
            Self::Refused(r) => Some(r.kind),
            _ => None,
        }
    }

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

    /// A short machine code for logs and `last_error`: never Discord's
    /// error text.
    pub fn code(&self) -> String {
        match self {
            Self::Done(_) => "done".to_owned(),
            Self::Refused(r) => crate::messages::sync_error_code(r.status, r.code),
            Self::RetryAfter(_) => "rate_limited".to_owned(),
            Self::NotSent(_) => "not_sent".to_owned(),
            Self::Unknown(UnknownCause::ServerError(status)) => format!("http_{status}"),
            Self::Unknown(_) => "unknown".to_owned(),
        }
    }
}

/// A definite refusal: the HTTP status, Discord's JSON error code, and what
/// it means for Quack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Refusal {
    /// The HTTP status (4xx).
    pub status: u16,
    /// Discord's JSON error code, when the body carried one.
    pub code: Option<u32>,
    /// What the refusal means.
    pub kind: RefusalKind,
}

impl Refusal {
    /// A refusal from a status and optional JSON error code.
    pub const fn new(status: u16, code: Option<u32>) -> Self {
        Self {
            status,
            code,
            kind: RefusalKind::from_answer(status, code),
        }
    }

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

    /// The machine code stored in `last_error` columns, such as
    /// `http_403_50001` (§10.1).
    pub fn machine_code(&self) -> String {
        match self.code {
            Some(code) => format!("http_{}_{code}", self.status),
            None => format!("http_{}", self.status),
        }
    }

    /// The refusal as the domain's action classifier takes it.
    pub const fn discord(self) -> DiscordRefusal {
        DiscordRefusal {
            status: self.status,
            code: self.code,
        }
    }
}

/// What a definite refusal means (§17.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RefusalKind {
    /// 50013: Quack lacks a permission.
    MissingPermissions,
    /// 50001: Quack can't see the channel or resource.
    MissingAccess,
    /// 50007: the member's DMs are closed or they blocked Quack.
    CannotDm,
    /// 50278: Quack shares no server with the member.
    NoMutualGuilds,
    /// 10004: Quack is not in the server.
    UnknownGuild,
    /// 10007: not a member of the server.
    UnknownMember,
    /// 10013: no such account.
    UnknownUser,
    /// 10026: no such ban.
    UnknownBan,
    /// 10008: no such message.
    UnknownMessage,
    /// 10003: no such channel.
    UnknownChannel,
    /// 413, or 40005: the request or a file is too large.
    TooLarge,
    /// Any other definite refusal (4xx except 429).
    Other,
}

impl RefusalKind {
    /// Maps Discord's answer. The JSON code decides when it is one Quack
    /// knows (an `Unknown …` code only with its documented 404, since Quack
    /// reads it as "absent"); otherwise a bare 413 is `TooLarge` and
    /// anything else `Other`.
    pub const fn from_answer(status: u16, code: Option<u32>) -> Self {
        match code {
            Some(50013) => Self::MissingPermissions,
            Some(50001) => Self::MissingAccess,
            Some(50007) => Self::CannotDm,
            Some(50278) => Self::NoMutualGuilds,
            Some(10004) if status == 404 => Self::UnknownGuild,
            Some(10007) if status == 404 => Self::UnknownMember,
            Some(10013) if status == 404 => Self::UnknownUser,
            Some(10026) if status == 404 => Self::UnknownBan,
            Some(10008) if status == 404 => Self::UnknownMessage,
            Some(10003) if status == 404 => Self::UnknownChannel,
            Some(40005) => Self::TooLarge,
            _ if status == 413 => Self::TooLarge,
            _ => Self::Other,
        }
    }

    /// Whether the thing asked about does not exist (any `Unknown…` code).
    pub const fn is_unknown_resource(self) -> bool {
        matches!(
            self,
            Self::UnknownGuild
                | Self::UnknownMember
                | Self::UnknownUser
                | Self::UnknownBan
                | Self::UnknownMessage
                | Self::UnknownChannel
        )
    }
}

/// Why a request may have taken effect without Quack learning the answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnknownCause {
    /// The deadline passed after the request was sent.
    Timeout,
    /// Discord answered 5xx.
    ServerError(u16),
    /// The connection broke after the request was sent.
    Connection,
    /// Discord answered success with a body Quack could not read.
    Malformed,
}

/// Why a request never left Quack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotSentCause {
    /// Connecting failed (DNS, TCP, or TLS) before anything was sent.
    Connection,
    /// The client is shutting down and accepts no new calls.
    ShuttingDown,
    /// Quack refused to build the request (a disallowed download URL, an
    /// unencodable header). Retrying will not help.
    Invalid,
}

/// Where a member read may come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fresh {
    /// The member cache under §2's rules, else REST.
    Cached,
    /// Always REST (action reads, §5.1).
    Rest,
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

/// Where a created or edited message is, and the attachment ids Discord
/// assigned (by file name).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SentMessage {
    /// The channel.
    pub channel_id: ChannelId,
    /// The message.
    pub message_id: MessageId,
    /// `(file name, attachment id)` for each file on the message.
    pub attachments: Vec<(String, AttachmentId)>,
}

/// A file to upload. Its `Debug` shows the size, not the bytes.
#[derive(Clone, PartialEq, Eq)]
pub struct OutgoingFile {
    /// The file name (also the `attachment://` name the message refers to).
    pub name: String,
    /// Its MIME type, when known (`application/octet-stream` otherwise).
    pub content_type: Option<String>,
    /// Its contents.
    pub bytes: Vec<u8>,
}

impl fmt::Debug for OutgoingFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OutgoingFile")
            .field("name", &self.name)
            .field("content_type", &self.content_type)
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

/// Files on an edit: none (Discord keeps what is there), keep one existing
/// attachment, or replace with a new upload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileEdit {
    /// Leave the message's attachments as they are.
    None,
    /// Keep exactly this attachment.
    Keep(AttachmentId),
    /// Replace the attachments with this file.
    Replace(OutgoingFile),
}

/// An interaction token. Secret: never logged; `Debug` prints
/// `[redacted]`.
#[derive(Clone, PartialEq, Eq)]
pub struct InteractionToken(String);

impl InteractionToken {
    /// Wraps a token from an interaction payload.
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }

    /// The secret itself, for building the request URL.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for InteractionToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("InteractionToken([redacted])")
    }
}
