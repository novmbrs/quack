//! `DiscordPort`: the engine's `Discord` port over `quack-discord` (§17.2,
//! §17.3). Each method is one call: fact reads go to [`Facts`] (the caches
//! under §2's freshness rules, else REST), everything else to the
//! [`DiscordClient`] method of the same name; messages are rendered with
//! the command ids from the last sync. Types and outcomes map variant for
//! variant (the `convert` module).

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

use std::convert::identity;

use convert::outcome;

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
        Box::pin(async move { outcome(self.facts.guild_facts(guild).await, identity) })
    }

    fn member(
        &self,
        guild: GuildId,
        member: MemberId,
        fresh: Fresh,
    ) -> BoxFuture<'_, Outcome<Membership>> {
        Box::pin(async move {
            outcome(
                self.facts
                    .member(guild, member, convert::fresh(fresh))
                    .await,
                convert::membership,
            )
        })
    }

    fn accounts<'a>(&'a self, ids: &'a [AccountId]) -> BoxFuture<'a, Outcome<Vec<AccountFacts>>> {
        Box::pin(async move { outcome(self.client().accounts(ids).await, identity) })
    }

    fn channel_facts(
        &self,
        guild: GuildId,
        channel: ChannelId,
    ) -> BoxFuture<'_, Outcome<Option<ChannelFacts>>> {
        Box::pin(async move { outcome(self.facts.channel_facts(guild, channel).await, identity) })
    }

    fn guild_channels(&self, guild: GuildId) -> BoxFuture<'_, Outcome<Vec<ChannelFacts>>> {
        Box::pin(async move { outcome(self.facts.guild_channels(guild).await, identity) })
    }

    fn ban(&self, guild: GuildId, member: MemberId) -> BoxFuture<'_, Outcome<Option<BanFacts>>> {
        Box::pin(async move { outcome(self.client().ban(guild, member).await, identity) })
    }

    fn is_thread_member(
        &self,
        thread: ChannelId,
        account: AccountId,
    ) -> BoxFuture<'_, Outcome<bool>> {
        Box::pin(async move {
            outcome(
                self.client().is_thread_member(thread, account).await,
                identity,
            )
        })
    }

    fn search_members<'a>(
        &'a self,
        guild: GuildId,
        query: &'a str,
        limit: u8,
    ) -> BoxFuture<'a, Outcome<Vec<MemberFacts>>> {
        Box::pin(async move {
            outcome(
                self.facts
                    .search_members(guild, query, u16::from(limit))
                    .await,
                |m| m,
            )
        })
    }

    fn timeout<'a>(
        &'a self,
        guild: GuildId,
        member: MemberId,
        until: OffsetDateTime,
        reason: &'a str,
    ) -> BoxFuture<'a, Outcome<()>> {
        Box::pin(async move {
            outcome(
                self.client().timeout(guild, member, until, reason).await,
                identity,
            )
        })
    }

    fn remove_timeout<'a>(
        &'a self,
        guild: GuildId,
        member: MemberId,
        reason: &'a str,
    ) -> BoxFuture<'a, Outcome<()>> {
        Box::pin(async move {
            outcome(
                self.client().remove_timeout(guild, member, reason).await,
                identity,
            )
        })
    }

    fn kick<'a>(
        &'a self,
        guild: GuildId,
        member: MemberId,
        reason: &'a str,
    ) -> BoxFuture<'a, Outcome<()>> {
        Box::pin(async move { outcome(self.client().kick(guild, member, reason).await, identity) })
    }

    fn ban_member<'a>(
        &'a self,
        guild: GuildId,
        member: MemberId,
        delete_message_seconds: u32,
        reason: &'a str,
    ) -> BoxFuture<'a, Outcome<()>> {
        Box::pin(async move {
            outcome(
                self.client()
                    .ban_member(guild, member, delete_message_seconds, reason)
                    .await,
                identity,
            )
        })
    }

    fn unban<'a>(
        &'a self,
        guild: GuildId,
        member: MemberId,
        reason: &'a str,
    ) -> BoxFuture<'a, Outcome<()>> {
        Box::pin(async move { outcome(self.client().unban(guild, member, reason).await, identity) })
    }

    fn open_dm(&self, account: AccountId) -> BoxFuture<'_, Outcome<ChannelId>> {
        Box::pin(async move { outcome(self.client().open_dm(account).await, identity) })
    }

    fn send<'a>(
        &'a self,
        channel: ChannelId,
        msg: &'a Msg,
        files: &'a [OutgoingFile],
        nonce: &'a Nonce,
    ) -> BoxFuture<'a, Outcome<SentMessage>> {
        Box::pin(async move {
            let files: Vec<_> = files.iter().map(convert::file).collect();
            let nonce = convert::nonce(nonce);
            let body = self.body(msg);
            outcome(
                self.client()
                    .send(channel, &body, &files, nonce.as_ref())
                    .await,
                convert::sent,
            )
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
            let files = convert::file_edit(files);
            outcome(
                self.client().edit(channel, message, &body, &files).await,
                convert::sent,
            )
        })
    }

    fn edit_interaction_original<'a>(
        &'a self,
        token: &'a InteractionToken,
        msg: &'a Msg,
    ) -> BoxFuture<'a, Outcome<()>> {
        Box::pin(async move {
            let rendered = render(msg, &self.commands.get());
            outcome(
                self.client()
                    .edit_interaction_original(&convert::token(token), &rendered)
                    .await,
                identity,
            )
        })
    }

    fn delete_message(&self, channel: ChannelId, message: MessageId) -> BoxFuture<'_, Outcome<()>> {
        Box::pin(async move {
            outcome(
                self.client().delete_message(channel, message).await,
                identity,
            )
        })
    }

    fn fetch_message(
        &self,
        channel: ChannelId,
        message: MessageId,
    ) -> BoxFuture<'_, Outcome<FetchedMessage>> {
        Box::pin(async move {
            outcome(
                self.client().fetch_message(channel, message).await,
                convert::fetched,
            )
        })
    }

    fn fetch_history(
        &self,
        channel: ChannelId,
        before: Option<MessageId>,
        limit: u8,
    ) -> BoxFuture<'_, Outcome<Vec<FetchedMessage>>> {
        Box::pin(async move {
            outcome(
                self.client().fetch_history(channel, before, limit).await,
                |list| list.into_iter().map(convert::fetched).collect(),
            )
        })
    }

    fn download_attachment<'a>(
        &'a self,
        url: &'a str,
        max_bytes: u64,
    ) -> BoxFuture<'a, Outcome<Vec<u8>>> {
        Box::pin(async move {
            outcome(
                self.client().download_attachment(url, max_bytes).await,
                |b| b,
            )
        })
    }

    fn create_channel<'a>(
        &'a self,
        guild: GuildId,
        spec: &'a ChannelSpec,
    ) -> BoxFuture<'a, Outcome<ChannelId>> {
        Box::pin(async move {
            outcome(
                self.client()
                    .create_channel(guild, &convert::channel_spec(spec))
                    .await,
                |c| c,
            )
        })
    }

    fn create_private_thread<'a>(
        &'a self,
        parent: ChannelId,
        name: &'a str,
    ) -> BoxFuture<'a, Outcome<ChannelId>> {
        Box::pin(async move {
            outcome(
                self.client().create_private_thread(parent, name).await,
                |c| c,
            )
        })
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
                Err(failure) => return outcome(failure, identity),
            };
            outcome(
                self.client()
                    .find_threads(guild, parent, name, quack, since)
                    .await,
                identity,
            )
        })
    }

    fn add_thread_member(&self, thread: ChannelId, member: MemberId) -> BoxFuture<'_, Outcome<()>> {
        Box::pin(async move {
            outcome(
                self.client().add_thread_member(thread, member).await,
                identity,
            )
        })
    }

    fn remove_thread_member(
        &self,
        thread: ChannelId,
        member: MemberId,
    ) -> BoxFuture<'_, Outcome<()>> {
        Box::pin(async move {
            outcome(
                self.client().remove_thread_member(thread, member).await,
                identity,
            )
        })
    }

    fn thread_members(&self, thread: ChannelId) -> BoxFuture<'_, Outcome<Vec<MemberId>>> {
        Box::pin(async move { outcome(self.client().thread_members(thread).await, identity) })
    }

    fn set_thread_state(
        &self,
        thread: ChannelId,
        archived: bool,
        locked: bool,
    ) -> BoxFuture<'_, Outcome<()>> {
        Box::pin(async move {
            outcome(
                self.client()
                    .set_thread_state(thread, archived, locked)
                    .await,
                identity,
            )
        })
    }

    fn delete_channel(&self, channel: ChannelId) -> BoxFuture<'_, Outcome<()>> {
        Box::pin(async move { outcome(self.client().delete_channel(channel).await, identity) })
    }
}
