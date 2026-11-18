//! Plain Discord data the client returns and takes, with quack-core ids.
//!
//! These are what the binary's `DiscordPort` maps into the engine's types;
//! no Twilight type crosses this crate's public surface except inside the
//! command definitions.

pub use quack_core::discord::{FileEdit, InteractionToken, OutgoingFile, SentMessage};
use quack_core::ids::{AccountId, AttachmentId, ChannelId, GuildId, MemberId, MessageId, RoleId};
use quack_core::permissions::{AccountFacts, Overwrite, RoleFacts};
use time::OffsetDateTime;

/// A member of a server as Discord returns it (before permissions are
/// computed; see [`crate::permissions::member_facts`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    /// The member's account.
    pub id: MemberId,
    /// Whether the account is a bot.
    pub bot: bool,
    /// Their username.
    pub username: String,
    /// Their global display name.
    pub global_name: Option<String>,
    /// Their account avatar hash.
    pub avatar_hash: Option<String>,
    /// Their nickname in this server.
    pub nick: Option<String>,
    /// Their role ids (without @everyone).
    pub role_ids: Vec<RoleId>,
    /// When this membership began.
    pub joined_at: Option<OffsetDateTime>,
    /// When their current timeout ends.
    pub timeout_until: Option<OffsetDateTime>,
}

impl Member {
    /// The member's account facts.
    pub fn account(&self) -> AccountFacts {
        AccountFacts {
            id: self.id.account(),
            username: self.username.clone(),
            global_name: self.global_name.clone(),
            avatar_hash: self.avatar_hash.clone(),
            bot: self.bot,
        }
    }
}

/// A server's own facts from `GET /guilds/{id}` (Quack's member is read
/// separately).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Guild {
    /// The server.
    pub id: GuildId,
    /// Its name.
    pub name: String,
    /// Its icon hash.
    pub icon_hash: Option<String>,
    /// The owner.
    pub owner_id: AccountId,
    /// Whether moderators must use 2FA (MFA level elevated).
    pub mfa_required: bool,
    /// Every role, @everyone included.
    pub roles: Vec<RoleFacts>,
}

/// Thread-only state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThreadState {
    /// Whether the thread is archived.
    pub archived: bool,
    /// Whether only moderators can unarchive it.
    pub locked: bool,
    /// When its archive state last changed (the paging key for archived
    /// threads).
    pub archived_at: Option<OffsetDateTime>,
}

/// A channel or thread as Discord returns it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Channel {
    /// The channel.
    pub id: ChannelId,
    /// Its server (absent for DM channels).
    pub guild_id: Option<GuildId>,
    /// Discord's channel type number (0 text, 4 category, 11/12 threads).
    pub kind: u8,
    /// Its name (empty for DM channels).
    pub name: String,
    /// The category, or a thread's parent channel.
    pub parent_id: Option<ChannelId>,
    /// Its permission overwrites (threads have none of their own).
    pub overwrites: Vec<Overwrite>,
    /// Thread state, for threads.
    pub thread: Option<ThreadState>,
    /// Who created it, for threads (`owner_id`).
    pub owner_id: Option<AccountId>,
}

impl Channel {
    /// Whether it is a thread (types 10, 11, 12).
    pub const fn is_thread(&self) -> bool {
        matches!(self.kind, 10..=12)
    }
}

/// A short summary of one embed (§7.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbedSummary {
    /// The embed type (`rich`, `image`, `link`, …).
    pub kind: String,
    /// Its title.
    pub title: Option<String>,
    /// Its description.
    pub description: Option<String>,
    /// Its URL.
    pub url: Option<String>,
}

/// A file attached to a message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attachment {
    /// The attachment.
    pub id: AttachmentId,
    /// Its file name.
    pub filename: String,
    /// Its MIME type, when Discord knows it.
    pub content_type: Option<String>,
    /// Its size in bytes.
    pub size: u64,
    /// Its CDN URL.
    pub url: String,
}

/// A message as Discord returns it (REST or gateway).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    /// The message.
    pub id: MessageId,
    /// Its channel.
    pub channel_id: ChannelId,
    /// Its server, when Discord says (gateway events do; REST reads don't).
    pub guild_id: Option<GuildId>,
    /// The author.
    pub author: AccountFacts,
    /// Whether a webhook posted it.
    pub webhook: bool,
    /// The author's role ids, when the payload had their member.
    pub member_role_ids: Option<Vec<RoleId>>,
    /// The text.
    pub content: String,
    /// When it was sent.
    pub created_at: OffsetDateTime,
    /// When it was last edited.
    pub edited_at: Option<OffsetDateTime>,
    /// Its embeds.
    pub embeds: Vec<EmbedSummary>,
    /// Its files.
    pub attachments: Vec<Attachment>,
}

/// A message nonce: 1–25 characters, fixed per record (§10.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Nonce(String);

impl Nonce {
    /// Most characters Discord accepts in a nonce.
    pub const MAX_CHARS: usize = 25;

    /// A nonce, or `None` when it is empty or longer than 25 characters.
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        (!value.is_empty() && value.chars().count() <= Self::MAX_CHARS).then_some(Self(value))
    }

    /// The nonce text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What an interaction callback needs: its id and token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InteractionRef {
    /// The interaction id.
    pub id: u64,
    /// Its token (valid 15 minutes).
    pub token: InteractionToken,
}

/// A channel to create (§13.1.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelSpec {
    /// Its name.
    pub name: String,
    /// Its topic.
    pub topic: Option<String>,
    /// The category to put it in.
    pub parent_id: Option<ChannelId>,
    /// Its permission overwrites.
    pub overwrites: Vec<Overwrite>,
}

/// What `GET /gateway/bot` says about sharding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GatewayBot {
    /// The number of shards Discord recommends.
    pub shards: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonces_are_bounded() {
        assert!(Nonce::new("").is_none());
        assert!(Nonce::new("a".repeat(26)).is_none());
        assert_eq!(
            Nonce::new("a".repeat(25)).map(|n| n.as_str().len()),
            Some(25)
        );
    }

    #[test]
    fn secrets_stay_out_of_debug() {
        let token = InteractionToken::new("secret");
        assert!(!format!("{token:?}").contains("secret"));
        assert_eq!(token.expose(), "secret");
        let file = OutgoingFile {
            name: "a.txt".into(),
            content_type: None,
            bytes: vec![1, 2, 3],
        };
        assert_eq!(
            format!("{file:?}"),
            r#"OutgoingFile { name: "a.txt", content_type: None, bytes: 3 }"#
        );
    }
}
