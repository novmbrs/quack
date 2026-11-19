//! The decoded gateway events the server activity log takes. `quack-bot`
//! builds them from Twilight's types, so this crate never sees Twilight.

use quack_core::ids::{AccountId, ChannelId, GuildId, MessageId};
use quack_text::model::{ChannelRef, Person};
use time::OffsetDateTime;

/// The Discord dispatch events the server activity log wants. Intents:
/// `GUILDS`, `GUILD_MEMBERS`, `GUILD_MODERATION` (audit log entries, with
/// View Audit Log), `GUILD_MESSAGES`, `MESSAGE_CONTENT`.
pub const GATEWAY_EVENTS: &[&str] = &[
    "MESSAGE_CREATE",
    "MESSAGE_UPDATE",
    "MESSAGE_DELETE",
    "MESSAGE_DELETE_BULK",
    "GUILD_MEMBER_ADD",
    "GUILD_MEMBER_REMOVE",
    "GUILD_AUDIT_LOG_ENTRY_CREATE",
    "GUILD_UPDATE",
    "CHANNEL_CREATE",
    "CHANNEL_UPDATE",
    "CHANNEL_DELETE",
    "GUILD_DELETE",
];

/// Who wrote a message, as far as filtering cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthorKind {
    /// A person.
    Member,
    /// Another bot (logged like a person).
    Bot,
    /// Quack itself (never posted).
    Quack,
    /// A webhook (never posted).
    Webhook,
}

/// One file of a message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileFacts {
    /// The file name.
    pub name: String,
    /// Its original URL.
    pub url: String,
    /// Its size in bytes.
    pub size: u64,
}

/// `MESSAGE_CREATE` in a text channel or thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageCreated {
    /// The server.
    pub guild: GuildId,
    /// The channel or thread.
    pub channel: ChannelId,
    /// The message.
    pub id: MessageId,
    /// Its author.
    pub author: Person,
    /// What kind of author.
    pub author_kind: AuthorKind,
    /// Its text.
    pub text: String,
    /// Its files.
    pub files: Vec<FileFacts>,
    /// Its link preview types.
    pub embeds: Vec<String>,
}

/// `MESSAGE_UPDATE`: a field Discord left out is `None` (unknown, not
/// empty).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageUpdated {
    /// The server.
    pub guild: GuildId,
    /// The channel or thread.
    pub channel: ChannelId,
    /// The message.
    pub id: MessageId,
    /// Its author, when the payload has one.
    pub author: Option<(Person, AuthorKind)>,
    /// Discord set an edited timestamp (embed unfurls don't).
    pub edited: bool,
    /// The new text.
    pub text: Option<String>,
    /// The new file list.
    pub files: Option<Vec<FileFacts>>,
    /// The new link preview types.
    pub embeds: Option<Vec<String>>,
}

/// A ban or unban from the audit log (entries 22 and 23).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BanChange {
    /// Audit log entry 22.
    Banned,
    /// Audit log entry 23.
    Unbanned,
}

/// `GUILD_AUDIT_LOG_ENTRY_CREATE` for a ban or unban.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BanEntry {
    /// The server.
    pub guild: GuildId,
    /// Banned or unbanned.
    pub change: BanChange,
    /// The member.
    pub member: Person,
    /// Who did it.
    pub executor: Person,
    /// Quack did it (never posted: the moderation log has it).
    pub by_quack: bool,
    /// The reason they gave.
    pub reason: Option<String>,
}

/// A channel event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChannelChange {
    /// `CHANNEL_CREATE`.
    Created(ChannelRef),
    /// `CHANNEL_UPDATE`.
    Updated(ChannelRef),
    /// `CHANNEL_DELETE`: the channel and its last name.
    Deleted(ChannelId, String),
}

/// `GUILD_MEMBER_ADD`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberJoined {
    /// The server.
    pub guild: GuildId,
    /// The member.
    pub member: Person,
    /// When their account was created.
    pub account_created: OffsetDateTime,
}

/// `GUILD_MEMBER_REMOVE`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberLeft {
    /// The server.
    pub guild: GuildId,
    /// Their username.
    pub username: String,
    /// Their account.
    pub id: AccountId,
}
