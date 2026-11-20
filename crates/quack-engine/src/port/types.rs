//! The plain types the `Discord` port takes and returns that `quack-discord`
//! shapes differently; the shared ones are in `quack_core::discord`.

use quack_core::ids::{AccountId, AttachmentId, ChannelId, MessageId};
use quack_core::permissions::Overwrite;
use time::OffsetDateTime;

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

/// A message as Discord returned it (evidence capture, file routes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchedMessage {
    /// The message.
    pub id: MessageId,
    /// Its channel or thread. A fetched message names no server (Discord
    /// leaves `guild_id` out of REST reads); the channel's server is the
    /// authority.
    pub channel_id: ChannelId,
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

/// A channel to create (setup, §13.1.4; ops channels, §21.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelSpec {
    /// Its name.
    pub name: String,
    /// Its topic.
    pub topic: Option<String>,
    /// Its permission overwrites.
    pub overwrites: Vec<Overwrite>,
    /// The category to put it in.
    pub parent: Option<ChannelId>,
    /// Whether it is a category rather than a text channel.
    pub category: bool,
}
