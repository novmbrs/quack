//! `DiscordPort`: the engine's `Discord` port over `quack-discord` (§17.2,
//! §17.3). Each method is one call: fact reads go to [`Facts`] (the caches
//! under §2's freshness rules, else REST), everything else to the
//! [`DiscordClient`] method of the same name; messages are rendered with
//! the command ids from the last sync. Outcomes and most payloads are
//! shared types; `convert` maps the rest.

mod convert;

#[cfg(test)]
mod tests;

use quack_bot::CommandMentions;
use quack_core::ids::{AccountId, ChannelId, GuildId, MemberId, MessageId};
use quack_core::permissions::{AccountFacts, BanFacts, ChannelFacts, GuildFacts, MemberFacts};
use quack_discord::DiscordClient;
use quack_discord::facts::Facts;
use quack_discord::render::render;
use quack_discord::rest::MessageBody;
use quack_engine::port::{
    ChannelSpec, FetchedMessage, FileEdit, Fresh, InteractionToken, Membership, Nonce, Outcome,
    OutgoingFile, SentMessage,
};
use quack_engine::{BoxFuture, Discord};
use quack_text::Msg;
use time::OffsetDateTime;

/// The engine's `Discord` port over Quack's Discord client and caches.
#[derive(Clone, Debug)]
pub struct DiscordPort {
    facts: Facts,
    commands: CommandMentions,
}

impl DiscordPort {
    /// A port over `facts` (which holds the REST client), rendering
    /// command mentions from `commands`.
    pub const fn new(facts: Facts, commands: CommandMentions) -> Self {
        Self { facts, commands }
    }

    fn client(&self) -> &DiscordClient {
        self.facts.client()
    }

    fn body(&self, msg: &Msg) -> MessageBody {
        MessageBody::from_msg(msg, &self.commands.get())
    }
}

impl Discord for DiscordPort {
    fn guild_facts(&self, guild: GuildId) -> BoxFuture<'_, Outcome<GuildFacts>> {
        Box::pin(async move { self.facts.guild_facts(guild).await })
    }

    fn member(
        &self,
        guild: GuildId,
        member: MemberId,
        fresh: Fresh,
    ) -> BoxFuture<'_, Outcome<Membership>> {
        Box::pin(async move { self.facts.member(guild, member, fresh).await })
    }

    fn accounts<'a>(&'a self, ids: &'a [AccountId]) -> BoxFuture<'a, Outcome<Vec<AccountFacts>>> {
        Box::pin(async move { self.client().accounts(ids).await })
    }

    fn channel_facts(
        &self,
        guild: GuildId,
        channel: ChannelId,
    ) -> BoxFuture<'_, Outcome<Option<ChannelFacts>>> {
        Box::pin(async move { self.facts.channel_facts(guild, channel).await })
    }

    fn guild_channels(&self, guild: GuildId) -> BoxFuture<'_, Outcome<Vec<ChannelFacts>>> {
        Box::pin(async move { self.facts.guild_channels(guild).await })
    }

    fn ban(&self, guild: GuildId, member: MemberId) -> BoxFuture<'_, Outcome<Option<BanFacts>>> {
        Box::pin(async move { self.client().ban(guild, member).await })
    }

    fn is_thread_member(
        &self,
        thread: ChannelId,
        account: AccountId,
    ) -> BoxFuture<'_, Outcome<bool>> {
        Box::pin(async move { self.client().is_thread_member(thread, account).await })
    }

    fn search_members<'a>(
        &'a self,
        guild: GuildId,
        query: &'a str,
        limit: u8,
    ) -> BoxFuture<'a, Outcome<Vec<MemberFacts>>> {
        Box::pin(async move {
            self.facts
                .search_members(guild, query, u16::from(limit))
                .await
        })
    }

    fn timeout<'a>(
        &'a self,
        guild: GuildId,
        member: MemberId,
        until: OffsetDateTime,
        reason: &'a str,
    ) -> BoxFuture<'a, Outcome<()>> {
        Box::pin(async move { self.client().timeout(guild, member, until, reason).await })
    }

    fn remove_timeout<'a>(
        &'a self,
        guild: GuildId,
        member: MemberId,
        reason: &'a str,
    ) -> BoxFuture<'a, Outcome<()>> {
        Box::pin(async move { self.client().remove_timeout(guild, member, reason).await })
    }

    fn kick<'a>(
        &'a self,
        guild: GuildId,
        member: MemberId,
        reason: &'a str,
    ) -> BoxFuture<'a, Outcome<()>> {
        Box::pin(async move { self.client().kick(guild, member, reason).await })
    }

    fn ban_member<'a>(
        &'a self,
        guild: GuildId,
        member: MemberId,
        delete_message_seconds: u32,
        reason: &'a str,
    ) -> BoxFuture<'a, Outcome<()>> {
        Box::pin(async move {
            self.client()
                .ban_member(guild, member, delete_message_seconds, reason)
                .await
        })
    }

    fn unban<'a>(
        &'a self,
        guild: GuildId,
        member: MemberId,
        reason: &'a str,
    ) -> BoxFuture<'a, Outcome<()>> {
        Box::pin(async move { self.client().unban(guild, member, reason).await })
    }

    fn open_dm(&self, account: AccountId) -> BoxFuture<'_, Outcome<ChannelId>> {
        Box::pin(async move { self.client().open_dm(account).await })
    }

    fn send<'a>(
        &'a self,
        channel: ChannelId,
        msg: &'a Msg,
        files: &'a [OutgoingFile],
        nonce: &'a Nonce,
    ) -> BoxFuture<'a, Outcome<SentMessage>> {
        Box::pin(async move {
            let nonce = convert::nonce(nonce);
            let body = self.body(msg);
            self.client()
                .send(channel, &body, files, nonce.as_ref())
                .await
        })
    }

    fn edit<'a>(
        &'a self,
        channel: ChannelId,
        message: MessageId,
        msg: &'a Msg,
        files: &'a FileEdit,
    ) -> BoxFuture<'a, Outcome<SentMessage>> {
        Box::pin(async move {
            let body = self.body(msg);
            self.client().edit(channel, message, &body, files).await
        })
    }

    fn edit_interaction_original<'a>(
        &'a self,
        token: &'a InteractionToken,
        msg: &'a Msg,
    ) -> BoxFuture<'a, Outcome<()>> {
        Box::pin(async move {
            let rendered = render(msg, &self.commands.get());
            self.client()
                .edit_interaction_original(token, &rendered)
                .await
        })
    }

    fn delete_message(&self, channel: ChannelId, message: MessageId) -> BoxFuture<'_, Outcome<()>> {
        Box::pin(async move { self.client().delete_message(channel, message).await })
    }

    fn fetch_message(
        &self,
        channel: ChannelId,
        message: MessageId,
    ) -> BoxFuture<'_, Outcome<FetchedMessage>> {
        Box::pin(async move {
            self.client()
                .fetch_message(channel, message)
                .await
                .map(convert::fetched)
        })
    }

    fn fetch_history(
        &self,
        channel: ChannelId,
        before: Option<MessageId>,
        limit: u8,
    ) -> BoxFuture<'_, Outcome<Vec<FetchedMessage>>> {
        Box::pin(async move {
            self.client()
                .fetch_history(channel, before, limit)
                .await
                .map(|list| list.into_iter().map(convert::fetched).collect())
        })
    }

    fn fetch_history_after(
        &self,
        channel: ChannelId,
        after: MessageId,
        limit: u8,
    ) -> BoxFuture<'_, Outcome<Vec<FetchedMessage>>> {
        Box::pin(async move {
            self.client()
                .fetch_history_after(channel, after, limit)
                .await
                .map(|list| list.into_iter().map(convert::fetched).collect())
        })
    }

    fn download_attachment<'a>(
        &'a self,
        url: &'a str,
        max_bytes: u64,
    ) -> BoxFuture<'a, Outcome<Vec<u8>>> {
        Box::pin(async move { self.client().download_attachment(url, max_bytes).await })
    }

    fn create_channel<'a>(
        &'a self,
        guild: GuildId,
        spec: &'a ChannelSpec,
    ) -> BoxFuture<'a, Outcome<ChannelId>> {
        Box::pin(async move {
            self.client()
                .create_channel(guild, &convert::channel_spec(spec))
                .await
        })
    }

    fn create_private_thread<'a>(
        &'a self,
        parent: ChannelId,
        name: &'a str,
    ) -> BoxFuture<'a, Outcome<ChannelId>> {
        Box::pin(async move { self.client().create_private_thread(parent, name).await })
    }

    fn find_threads<'a>(
        &'a self,
        guild: GuildId,
        parent: ChannelId,
        name: &'a str,
        since: time::OffsetDateTime,
    ) -> BoxFuture<'a, Outcome<Vec<ChannelId>>> {
        Box::pin(async move {
            // Only threads Quack itself created are ever returned.
            let quack = match self.facts.quack_id().await.into_result() {
                Ok(quack) => quack,
                Err(failure) => return failure,
            };
            self.client()
                .find_threads(guild, parent, name, quack, since)
                .await
        })
    }

    fn add_thread_member(&self, thread: ChannelId, member: MemberId) -> BoxFuture<'_, Outcome<()>> {
        Box::pin(async move { self.client().add_thread_member(thread, member).await })
    }

    fn remove_thread_member(
        &self,
        thread: ChannelId,
        member: MemberId,
    ) -> BoxFuture<'_, Outcome<()>> {
        Box::pin(async move { self.client().remove_thread_member(thread, member).await })
    }

    fn thread_members(&self, thread: ChannelId) -> BoxFuture<'_, Outcome<Vec<MemberId>>> {
        Box::pin(async move { self.client().thread_members(thread).await })
    }

    fn set_thread_state(
        &self,
        thread: ChannelId,
        archived: bool,
        locked: bool,
    ) -> BoxFuture<'_, Outcome<()>> {
        Box::pin(async move {
            self.client()
                .set_thread_state(thread, archived, locked)
                .await
        })
    }

    fn delete_channel(&self, channel: ChannelId) -> BoxFuture<'_, Outcome<()>> {
        Box::pin(async move { self.client().delete_channel(channel).await })
    }
}
