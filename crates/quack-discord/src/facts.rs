//! Facts under the freshness rules of §2: the caches while they are exact,
//! Discord REST otherwise.
//!
//! The guild cache answers for a server only while the gateway is connected
//! and the server has loaded since the last `READY`; the member cache
//! answers with event-fed entries under the same condition and with
//! REST-fed entries for 5 minutes. "Absent" always comes from Discord and is
//! never cached. These are the fact reads of the engine's `Discord` port.

use std::sync::{Arc, OnceLock};

use quack_core::ids::{AccountId, ChannelId, GuildId, MemberId};
use quack_core::permissions::{ChannelFacts, GuildFacts, MemberFacts};
use time::OffsetDateTime;

use crate::cache::{GuildCache, MemberCache};
use crate::model::Channel;
use crate::permissions::{channel_facts, member_facts};
use crate::rest::{DiscordClient, Outcome, RefusalKind};

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

/// Reads server, member, and channel facts from the caches or REST.
/// Cheap to clone.
#[derive(Clone, Debug)]
pub struct Facts {
    client: DiscordClient,
    guilds: Arc<GuildCache>,
    members: Arc<MemberCache>,
    quack: Arc<OnceLock<AccountId>>,
}

impl Facts {
    /// Facts over this client and these caches (the ones the gateway
    /// feeds).
    pub fn new(client: DiscordClient, guilds: Arc<GuildCache>, members: Arc<MemberCache>) -> Self {
        Self {
            client,
            guilds,
            members,
            quack: Arc::new(OnceLock::new()),
        }
    }

    /// The REST client.
    pub const fn client(&self) -> &DiscordClient {
        &self.client
    }

    /// Quack's own account: from `READY`, else `GET /users/@me` (once).
    pub async fn quack_id(&self) -> Outcome<AccountId> {
        if let Some(id) = self.guilds.quack_id().or_else(|| self.quack.get().copied()) {
            return Outcome::Done(id);
        }
        self.client.current_user().await.map(|me| {
            let _ = self.quack.set(me.id);
            me.id
        })
    }

    /// A server's owner, MFA level, roles, and Quack's own member.
    pub async fn guild_facts(&self, guild: GuildId) -> Outcome<GuildFacts> {
        let now = OffsetDateTime::now_utc();
        if let Some(facts) = self.guilds.guild_facts(guild, now) {
            return Outcome::Done(facts);
        }
        let quack = match self.quack_id().await.into_result() {
            Ok(quack) => quack,
            Err(failure) => return failure,
        };
        self.client.guild_facts(guild, quack, now).await
    }

    /// One member of a server with permissions computed from the server's
    /// facts. `Absent` only for Discord's 404 Unknown Member; a REST answer
    /// refreshes the member cache.
    pub async fn member(
        &self,
        guild: GuildId,
        member: MemberId,
        fresh: Fresh,
    ) -> Outcome<Membership> {
        let facts = match self.guild_facts(guild).await.into_result() {
            Ok(facts) => facts,
            Err(failure) => return failure,
        };
        let now = OffsetDateTime::now_utc();
        if fresh == Fresh::Cached
            && let Some(cached) = self
                .members
                .get(guild, member, self.guilds.is_trusted(guild))
        {
            return Outcome::Done(Membership::Present(member_facts(&facts, &cached, now)));
        }
        self.client.member(guild, member).await.map(|found| {
            let Some(found) = found else {
                self.members.remove(guild, member);
                return Membership::Absent;
            };
            let present = member_facts(&facts, &found, now);
            self.members.put_rest(guild, found);
            Membership::Present(present)
        })
    }

    /// Members whose username or nickname starts with `query` (at most
    /// `limit`), with permissions computed from the server's facts. Always
    /// REST; the answers refresh the member cache.
    pub async fn search_members(
        &self,
        guild: GuildId,
        query: &str,
        limit: u16,
    ) -> Outcome<Vec<MemberFacts>> {
        let facts = match self.guild_facts(guild).await.into_result() {
            Ok(facts) => facts,
            Err(failure) => return failure,
        };
        let now = OffsetDateTime::now_utc();
        self.client
            .search_members(guild, query, limit)
            .await
            .map(|found| {
                found
                    .into_iter()
                    .map(|m| {
                        let present = member_facts(&facts, &m, now);
                        self.members.put_rest(guild, m);
                        present
                    })
                    .collect()
            })
    }

    /// One channel or thread of a server with Quack's permissions in it;
    /// `None` when Discord says it doesn't exist (404 Unknown Channel) or it
    /// belongs to another server.
    pub async fn channel_facts(
        &self,
        guild: GuildId,
        channel: ChannelId,
    ) -> Outcome<Option<ChannelFacts>> {
        let now = OffsetDateTime::now_utc();
        if let Some(Some(found)) = self.guilds.channel_facts(guild, channel, now) {
            return Outcome::Done(Some(found));
        }
        let facts = match self.guild_facts(guild).await.into_result() {
            Ok(facts) => facts,
            Err(failure) => return failure,
        };
        let found = match self.rest_channel(guild, channel).await.into_result() {
            Ok(Some(found)) => found,
            Ok(None) => return Outcome::Done(None),
            Err(failure) => return failure,
        };
        let parent = match found.parent_id.filter(|_| found.is_thread()) {
            Some(parent) => match self.rest_channel(guild, parent).await.into_result() {
                Ok(parent) => parent,
                Err(failure) => return failure,
            },
            None => None,
        };
        Outcome::Done(Some(channel_facts(&facts, &found, parent.as_ref(), now)))
    }

    /// Every channel of a server (not threads) with Quack's permissions.
    pub async fn guild_channels(&self, guild: GuildId) -> Outcome<Vec<ChannelFacts>> {
        let now = OffsetDateTime::now_utc();
        if let Some(channels) = self.guilds.guild_channels(guild, now) {
            return Outcome::Done(channels);
        }
        let facts = match self.guild_facts(guild).await.into_result() {
            Ok(facts) => facts,
            Err(failure) => return failure,
        };
        self.client.guild_channels(guild).await.map(|channels| {
            channels
                .iter()
                .filter(|c| !c.is_thread())
                .map(|c| channel_facts(&facts, c, None, now))
                .collect()
        })
    }

    /// `GET /channels/{id}`, with Unknown Channel and another server's
    /// channel as `None`.
    async fn rest_channel(&self, guild: GuildId, channel: ChannelId) -> Outcome<Option<Channel>> {
        match self.client.channel(channel).await {
            Outcome::Refused(r) if r.kind == RefusalKind::UnknownChannel => Outcome::Done(None),
            other => other.map(|c| (c.guild_id == Some(guild)).then_some(c)),
        }
    }
}

#[cfg(test)]
mod tests;
