//! The `Discord` port (§17.3): everything the engine and the modules need
//! from Discord, and nothing else. It is the only boundary tests replace:
//! the binary adapts the real client to it, and `quack-testkit` fakes it.
//!
//! Every call returns an [`Outcome`], so a caller always knows whether
//! Discord confirmed the call, definitely refused it, rate limited it
//! before sending, never received it, or may or may not have applied it.

mod types;

use std::future::Future;
use std::pin::Pin;

use quack_core::ids::{AccountId, ChannelId, GuildId, MemberId, MessageId};
use quack_core::permissions::{AccountFacts, BanFacts, ChannelFacts, GuildFacts, MemberFacts};
use quack_text::Msg;
use time::OffsetDateTime;

pub use quack_core::discord::{
    FileEdit, Fresh, InteractionToken, Membership, NotSentCause, Outcome, OutgoingFile, Refusal,
    RefusalKind, SentMessage, UnknownCause,
};
pub use types::{ChannelSpec, FetchedAttachment, FetchedEmbed, FetchedMessage, Nonce};

/// A boxed, sendable future borrowing for `'a`: what every port method
/// returns.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Everything the engine and modules need from Discord (§17.3).
///
/// Implemented by the binary's adapter over `quack-discord` and by
/// `quack-testkit`'s fake. Fact reads may answer from the gateway caches
/// only under the freshness rules of §2; "absent" is never cached. Every
/// mutating call carries the `X-Audit-Log-Reason` it is given.
pub trait Discord: Send + Sync + 'static {
    // Facts.

    /// A server's owner, MFA level, roles, and Quack's own member.
    fn guild_facts(&self, guild: GuildId) -> BoxFuture<'_, Outcome<GuildFacts>>;
    /// One member of a server. `Fresh::Rest` always asks Discord (action
    /// reads, §5.1). `Absent` only for 404 Unknown Member.
    fn member(
        &self,
        guild: GuildId,
        member: MemberId,
        fresh: Fresh,
    ) -> BoxFuture<'_, Outcome<Membership>>;
    /// Accounts by id, whether or not they share a server with Quack.
    /// Unknown ids are left out.
    fn accounts<'a>(&'a self, ids: &'a [AccountId]) -> BoxFuture<'a, Outcome<Vec<AccountFacts>>>;
    /// One channel or thread of a server; `None` for 404 Unknown Channel.
    fn channel_facts(
        &self,
        guild: GuildId,
        channel: ChannelId,
    ) -> BoxFuture<'_, Outcome<Option<ChannelFacts>>>;
    /// Every channel of a server (setup, reconciliation, the directory).
    fn guild_channels(&self, guild: GuildId) -> BoxFuture<'_, Outcome<Vec<ChannelFacts>>>;
    /// The member's ban, read from Discord; `None` for 404 Unknown Ban.
    fn ban(&self, guild: GuildId, member: MemberId) -> BoxFuture<'_, Outcome<Option<BanFacts>>>;
    /// Whether `account` is a member of a private thread.
    fn is_thread_member(
        &self,
        thread: ChannelId,
        account: AccountId,
    ) -> BoxFuture<'_, Outcome<bool>>;
    /// Members whose names start with `query` (the dashboard directory).
    fn search_members<'a>(
        &'a self,
        guild: GuildId,
        query: &'a str,
        limit: u8,
    ) -> BoxFuture<'a, Outcome<Vec<MemberFacts>>>;

    // Actions (§5.1). `reason` is the X-Audit-Log-Reason value.

    /// Times the member out until `until`.
    fn timeout<'a>(
        &'a self,
        guild: GuildId,
        member: MemberId,
        until: OffsetDateTime,
        reason: &'a str,
    ) -> BoxFuture<'a, Outcome<()>>;
    /// Removes the member's timeout.
    fn remove_timeout<'a>(
        &'a self,
        guild: GuildId,
        member: MemberId,
        reason: &'a str,
    ) -> BoxFuture<'a, Outcome<()>>;
    /// Removes the member from the server.
    fn kick<'a>(
        &'a self,
        guild: GuildId,
        member: MemberId,
        reason: &'a str,
    ) -> BoxFuture<'a, Outcome<()>>;
    /// Bans the member, deleting `delete_message_seconds` of their messages.
    fn ban_member<'a>(
        &'a self,
        guild: GuildId,
        member: MemberId,
        delete_message_seconds: u32,
        reason: &'a str,
    ) -> BoxFuture<'a, Outcome<()>>;
    /// Removes the member's ban.
    fn unban<'a>(
        &'a self,
        guild: GuildId,
        member: MemberId,
        reason: &'a str,
    ) -> BoxFuture<'a, Outcome<()>>;

    // Messages. Creates follow §10.4: the nonce is sent with
    // `enforce_nonce`, so a repeat inside the window returns the first
    // message instead of creating another.

    /// Opens (or finds) the DM channel with an account; idempotent.
    fn open_dm(&self, account: AccountId) -> BoxFuture<'_, Outcome<ChannelId>>;
    /// Creates a message. An empty `msg` (no blocks, no rows) with files is
    /// a plain message that carries only the files (evidence copies).
    fn send<'a>(
        &'a self,
        channel: ChannelId,
        msg: &'a Msg,
        files: &'a [OutgoingFile],
        nonce: &'a Nonce,
    ) -> BoxFuture<'a, Outcome<SentMessage>>;
    /// Edits a message Quack posted, with bot credentials.
    fn edit<'a>(
        &'a self,
        channel: ChannelId,
        message: MessageId,
        msg: &'a Msg,
        files: &'a FileEdit,
    ) -> BoxFuture<'a, Outcome<SentMessage>>;
    /// Edits an interaction's original (ephemeral) response through its
    /// token (private receipts, §10.3).
    fn edit_interaction_original<'a>(
        &'a self,
        token: &'a InteractionToken,
        msg: &'a Msg,
    ) -> BoxFuture<'a, Outcome<()>>;
    /// Deletes a message.
    fn delete_message(&self, channel: ChannelId, message: MessageId) -> BoxFuture<'_, Outcome<()>>;
    /// Reads one message.
    fn fetch_message(
        &self,
        channel: ChannelId,
        message: MessageId,
    ) -> BoxFuture<'_, Outcome<FetchedMessage>>;
    /// Reads up to `limit` messages before `before` (newest first).
    fn fetch_history(
        &self,
        channel: ChannelId,
        before: Option<MessageId>,
        limit: u8,
    ) -> BoxFuture<'_, Outcome<Vec<FetchedMessage>>>;
    /// Reads up to `limit` of the messages right after `after` (the
    /// surrounding messages of evidence, §7.2); in whatever order Discord
    /// returns them, so callers sort by id.
    fn fetch_history_after(
        &self,
        channel: ChannelId,
        after: MessageId,
        limit: u8,
    ) -> BoxFuture<'_, Outcome<Vec<FetchedMessage>>>;

    // Files.

    /// Downloads an attachment from Discord's CDN, stopping at `max_bytes`.
    fn download_attachment<'a>(
        &'a self,
        url: &'a str,
        max_bytes: u64,
    ) -> BoxFuture<'a, Outcome<Vec<u8>>>;

    // Channels and threads.

    /// Creates a channel.
    fn create_channel<'a>(
        &'a self,
        guild: GuildId,
        spec: &'a ChannelSpec,
    ) -> BoxFuture<'a, Outcome<ChannelId>>;
    /// Creates a private thread under `parent`.
    fn create_private_thread<'a>(
        &'a self,
        parent: ChannelId,
        name: &'a str,
    ) -> BoxFuture<'a, Outcome<ChannelId>>;
    /// Private threads Quack created under `parent` with exactly this
    /// name, oldest first (active and archived ones). Only a complete
    /// lookup is `Done`: every archived thread made since `since` was seen.
    /// Nothing another account made is ever returned, so the caller may
    /// adopt or delete what it gets.
    fn find_threads<'a>(
        &'a self,
        guild: GuildId,
        parent: ChannelId,
        name: &'a str,
        since: time::OffsetDateTime,
    ) -> BoxFuture<'a, Outcome<Vec<ChannelId>>>;
    /// Adds a member to a thread; idempotent.
    fn add_thread_member(&self, thread: ChannelId, member: MemberId) -> BoxFuture<'_, Outcome<()>>;
    /// Removes a member from a thread.
    fn remove_thread_member(
        &self,
        thread: ChannelId,
        member: MemberId,
    ) -> BoxFuture<'_, Outcome<()>>;
    /// A thread's members.
    fn thread_members(&self, thread: ChannelId) -> BoxFuture<'_, Outcome<Vec<MemberId>>>;
    /// Archives and locks (or reopens) a thread.
    fn set_thread_state(
        &self,
        thread: ChannelId,
        archived: bool,
        locked: bool,
    ) -> BoxFuture<'_, Outcome<()>>;
    /// Deletes a channel or thread.
    fn delete_channel(&self, channel: ChannelId) -> BoxFuture<'_, Outcome<()>>;
}
