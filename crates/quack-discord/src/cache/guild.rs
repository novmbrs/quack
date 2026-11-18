//! The guild cache (§11.10): servers, roles, channels (with overwrites and
//! parents), Quack's own account and its member in each server.
//!
//! Backed by `twilight-cache-inmemory` with only the `GUILD`, `CHANNEL`,
//! `ROLE`, and `USER_CURRENT` resources. It is trusted for a server only
//! while the gateway is connected and the server has sent `GUILD_CREATE`
//! since the last `READY` (§2); otherwise every lookup answers `None` and
//! callers use REST.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use quack_core::ids::{AccountId, ChannelId, GuildId};
use quack_core::permissions::{ChannelFacts, GuildFacts};
use quack_telemetry::labels::CacheName;
use quack_telemetry::metrics;
use time::OffsetDateTime;
use twilight_cache_inmemory::{InMemoryCache, ResourceType};
use twilight_model::gateway::event::Event;
use twilight_model::gateway::payload::incoming::GuildCreate;
use twilight_model::id::Id;

use crate::convert;
use crate::model::{Channel, Member};
use crate::permissions::{channel_facts, member_facts};

/// The metric name of this cache.
pub(crate) const NAME: CacheName = CacheName::new("guilds");

#[derive(Debug, Default)]
struct State {
    connected: bool,
    loaded: HashSet<GuildId>,
    quack_id: Option<AccountId>,
    quack_members: HashMap<GuildId, Member>,
}

/// The guild cache.
#[derive(Debug)]
pub struct GuildCache {
    cache: InMemoryCache,
    state: RwLock<State>,
}

impl Default for GuildCache {
    fn default() -> Self {
        Self::new()
    }
}

fn tw<M>(id: u64) -> Option<Id<M>> {
    Id::new_checked(id)
}

impl GuildCache {
    /// An empty, untrusted cache.
    pub fn new() -> Self {
        Self {
            cache: InMemoryCache::builder()
                .resource_types(
                    ResourceType::GUILD
                        | ResourceType::CHANNEL
                        | ResourceType::ROLE
                        | ResourceType::USER_CURRENT,
                )
                .build(),
            state: RwLock::new(State::default()),
        }
    }

    fn read(&self) -> RwLockReadGuard<'_, State> {
        self.state.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self) -> RwLockWriteGuard<'_, State> {
        self.state.write().unwrap_or_else(PoisonError::into_inner)
    }

    /// Reports the cache's size on `quack.cache.size`.
    pub fn observe(self: &Arc<Self>) {
        let me = Arc::downgrade(self);
        metrics().caches.observe_size(NAME, move || {
            me.upgrade().map_or(0, |c| c.guild_count() as u64)
        });
    }

    /// Applies a gateway event. Runs in the shard loop before the event is
    /// handed off, so handlers always see the cache after their event.
    pub(crate) fn update(&self, event: &Event) {
        if let Event::Ready(_) = event {
            self.cache.clear();
        }
        self.cache.update(event);
        let mut state = self.write();
        match event {
            Event::Ready(ready) => {
                state.connected = true;
                state.quack_id = convert::id(ready.user.id);
                state.loaded.clear();
                state.quack_members.clear();
            }
            Event::Resumed => state.connected = true,
            Event::GatewayClose(_)
            | Event::GatewayReconnect
            | Event::GatewayInvalidateSession(_) => {
                state.connected = false;
            }
            Event::GuildCreate(create) => match &**create {
                GuildCreate::Available(guild) => {
                    let Some(id) = convert::id::<GuildId, _>(guild.id) else {
                        return;
                    };
                    let quack = state.quack_id;
                    if let Some(member) = guild
                        .members
                        .iter()
                        .find(|m| convert::id::<AccountId, _>(m.user.id) == quack)
                        .and_then(convert::member)
                    {
                        state.quack_members.insert(id, member);
                    }
                    state.loaded.insert(id);
                }
                GuildCreate::Unavailable(guild) => {
                    if let Some(id) = convert::id(guild.id) {
                        state.loaded.remove(&id);
                    }
                }
            },
            Event::UnavailableGuild(guild) => {
                if let Some(id) = convert::id(guild.id) {
                    state.loaded.remove(&id);
                }
            }
            Event::GuildDelete(delete) => {
                if let Some(id) = convert::id(delete.id) {
                    state.loaded.remove(&id);
                    state.quack_members.remove(&id);
                }
            }
            Event::MemberAdd(add) if convert::id(add.member.user.id) == state.quack_id => {
                if let (Some(guild), Some(member)) =
                    (convert::id(add.guild_id), convert::member(&add.member))
                {
                    state.quack_members.insert(guild, member);
                }
            }
            Event::MemberUpdate(update) if convert::id(update.user.id) == state.quack_id => {
                if let (Some(guild), Some(member)) =
                    (convert::id(update.guild_id), super::member_update(update))
                {
                    state.quack_members.insert(guild, member);
                }
            }
            _ => {}
        }
    }

    /// Whether the cache is exact for this server right now.
    pub fn is_trusted(&self, guild: GuildId) -> bool {
        let state = self.read();
        state.connected && state.loaded.contains(&guild)
    }

    /// Whether the gateway session is connected (READY or RESUMED).
    pub fn is_connected(&self) -> bool {
        self.read().connected
    }

    /// Quack's own account, once READY arrived.
    pub fn quack_id(&self) -> Option<AccountId> {
        self.read().quack_id
    }

    /// How many servers are cached.
    pub fn guild_count(&self) -> usize {
        self.cache.stats().guilds()
    }

    /// Server facts from the cache, or `None` when it isn't trusted for
    /// the server (use REST then).
    pub fn guild_facts(&self, guild: GuildId, now: OffsetDateTime) -> Option<GuildFacts> {
        let facts = self.guild_facts_inner(guild, now);
        metrics().caches.lookup(NAME, facts.is_some());
        facts
    }

    fn guild_facts_inner(&self, guild: GuildId, now: OffsetDateTime) -> Option<GuildFacts> {
        let quack = {
            let state = self.read();
            if !(state.connected && state.loaded.contains(&guild)) {
                return None;
            }
            state.quack_members.get(&guild)?.clone()
        };
        let id = tw(guild.get())?;
        let cached = self.cache.guild(id)?;
        let roles = self
            .cache
            .guild_roles(id)?
            .iter()
            .filter_map(|r| {
                self.cache
                    .role(*r)
                    .and_then(|role| convert::role(role.resource()))
            })
            .collect();
        let mut facts = GuildFacts {
            guild_id: guild,
            name: cached.name().to_owned(),
            icon_hash: cached.icon().map(ToString::to_string),
            owner_id: convert::id(cached.owner_id())?,
            mfa_required: cached.mfa_level() == twilight_model::guild::MfaLevel::Elevated,
            roles,
            quack: None,
        };
        facts.quack = Some(member_facts(&facts, &quack, now));
        Some(facts)
    }

    /// A channel or thread of this server, if cached.
    fn channel(&self, guild: GuildId, channel: ChannelId) -> Option<Channel> {
        let cached = self.cache.channel(tw(channel.get())?)?;
        convert::channel(&cached).filter(|c| c.guild_id == Some(guild))
    }

    /// A channel's facts from the cache: `None` when the cache isn't
    /// trusted, `Some(None)` when the server has no such channel.
    pub fn channel_facts(
        &self,
        guild: GuildId,
        channel: ChannelId,
        now: OffsetDateTime,
    ) -> Option<Option<ChannelFacts>> {
        let facts = self.guild_facts_inner(guild, now)?;
        let found = self.channel(guild, channel).map(|c| {
            let parent = c
                .is_thread()
                .then(|| c.parent_id.and_then(|p| self.channel(guild, p)))
                .flatten();
            channel_facts(&facts, &c, parent.as_ref(), now)
        });
        metrics().caches.lookup(NAME, true);
        Some(found)
    }

    /// Every channel of the server (not threads), or `None` when the cache
    /// isn't trusted.
    pub fn guild_channels(&self, guild: GuildId, now: OffsetDateTime) -> Option<Vec<ChannelFacts>> {
        let facts = self.guild_facts_inner(guild, now)?;
        let ids: Vec<ChannelId> = self
            .cache
            .guild_channels(tw(guild.get())?)?
            .iter()
            .filter_map(|c| convert::id(*c))
            .collect();
        metrics().caches.lookup(NAME, true);
        Some(
            ids.into_iter()
                .filter_map(|id| self.channel(guild, id))
                .filter(|c| !c.is_thread())
                .map(|c| channel_facts(&facts, &c, None, now))
                .collect(),
        )
    }
}
