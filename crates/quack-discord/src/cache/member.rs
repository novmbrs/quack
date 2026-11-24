//! The member cache (§11.10): (server, account) → member, at most 200,000
//! entries, least recently used evicted.
//!
//! Event-fed entries are exact while the gateway is connected and the server
//! loaded, so callers pass whether the server is trusted; REST-fed entries
//! expire after 5 minutes. "Not a member" is never cached.
//!
//! It owns the account cache too, so every member it stores also refreshes
//! that member's account.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use quack_core::ids::{GuildId, MemberId};
use quack_telemetry::labels::CacheName;
use quack_telemetry::metrics;

use super::account::{self, AccountCache};
use super::lru::Lru;
use crate::model::Member;

/// The metric name of this cache.
pub(crate) const NAME: CacheName = CacheName::new("members");

#[derive(Clone, Copy, Debug)]
enum Fed {
    Event,
    Rest(Instant),
}

#[derive(Debug)]
struct Entry {
    member: Member,
    fed: Fed,
}

/// When a REST request started: the server's revision and the time, read
/// together by [`MemberCache::revision`].
#[derive(Clone, Copy, Debug)]
pub struct Since {
    revision: u64,
    at: Instant,
}

/// The bounded member cache.
#[derive(Debug)]
pub struct MemberCache {
    entries: Mutex<Inner>,
    rest_ttl: Duration,
    accounts: AccountCache,
}

/// The entries, and per server a revision every gateway change bumps, so a
/// REST answer that started before a change never overwrites it.
#[derive(Debug)]
struct Inner {
    lru: Lru<(GuildId, MemberId), Entry>,
    revisions: std::collections::HashMap<GuildId, u64>,
}

impl Inner {
    fn bump(&mut self, guild: GuildId) {
        *self.revisions.entry(guild).or_default() += 1;
    }

    fn revision(&self, guild: GuildId) -> u64 {
        self.revisions.get(&guild).copied().unwrap_or_default()
    }
}

impl Default for MemberCache {
    fn default() -> Self {
        Self::new(Self::CAPACITY, Self::REST_TTL)
    }
}

impl MemberCache {
    /// The design's bound.
    pub const CAPACITY: usize = 200_000;
    /// How long a REST-fed entry may be used.
    pub const REST_TTL: Duration = Duration::from_secs(300);

    /// An empty cache.
    pub fn new(capacity: usize, rest_ttl: Duration) -> Self {
        Self {
            entries: Mutex::new(Inner {
                lru: Lru::new(capacity),
                revisions: std::collections::HashMap::new(),
            }),
            rest_ttl,
            accounts: AccountCache::default(),
        }
    }

    /// The account cache this cache feeds.
    pub const fn accounts(&self) -> &AccountCache {
        &self.accounts
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Reports its size and the account cache's on `quack.cache.size`.
    pub fn observe(self: &Arc<Self>) {
        let me = Arc::downgrade(self);
        metrics()
            .caches
            .observe_size(NAME, move || me.upgrade().map_or(0, |c| c.len() as u64));
        let me = Arc::downgrade(self);
        metrics().caches.observe_size(account::NAME, move || {
            me.upgrade().map_or(0, |c| c.accounts.len() as u64)
        });
    }

    /// A cached member. Event-fed entries count only when `trusted` (the
    /// gateway is connected and the server loaded); REST-fed ones only
    /// while fresh.
    pub fn get(&self, guild: GuildId, member: MemberId, trusted: bool) -> Option<Member> {
        let found = {
            let mut entries = self.lock();
            entries.lru.get(&(guild, member)).and_then(|e| {
                let usable = match e.fed {
                    Fed::Event => trusted,
                    Fed::Rest(at) => at.elapsed() < self.rest_ttl,
                };
                usable.then(|| e.member.clone())
            })
        };
        metrics().caches.lookup(NAME, found.is_some());
        found
    }

    fn insert(&self, entries: &mut Inner, guild: GuildId, member: Member, fed: Fed, seen: Instant) {
        self.accounts.put_member(&member, seen);
        let evicted = entries
            .lru
            .insert((guild, member.id), Entry { member, fed });
        if evicted > 0 {
            metrics().caches.evicted(NAME, evicted as u64);
        }
    }

    /// The server's revision now: read it before a REST request and pass
    /// it to [`MemberCache::put_rest`] or [`MemberCache::remove_rest`].
    pub fn revision(&self, guild: GuildId) -> Since {
        Since {
            revision: self.lock().revision(guild),
            at: Instant::now(),
        }
    }

    /// Whether a gateway event already put this member in the cache (a
    /// join for them is then a repeat). Not a lookup: not counted.
    pub(crate) fn holds_from_event(&self, guild: GuildId, member: MemberId) -> bool {
        self.lock()
            .lru
            .peek(&(guild, member))
            .is_some_and(|e| matches!(e.fed, Fed::Event))
    }

    /// Stores a member from a gateway event.
    pub fn put_event(&self, guild: GuildId, member: Member) {
        let mut entries = self.lock();
        entries.bump(guild);
        self.insert(&mut entries, guild, member, Fed::Event, Instant::now());
    }

    /// Stores a member Quack read over REST, unless a gateway change for
    /// the server came after `since` (the answer may be older than it). Its
    /// account counts as seen when the request started.
    pub fn put_rest(&self, guild: GuildId, member: Member, since: Since) {
        let mut entries = self.lock();
        if entries.revision(guild) == since.revision {
            let fed = Fed::Rest(Instant::now());
            self.insert(&mut entries, guild, member, fed, since.at);
        }
    }

    /// Forgets a member (`GUILD_MEMBER_REMOVE`).
    pub fn remove(&self, guild: GuildId, member: MemberId) {
        let mut entries = self.lock();
        entries.bump(guild);
        entries.lru.remove(&(guild, member));
    }

    /// Forgets a member REST said is absent, unless a gateway change for
    /// the server (a rejoin, say) came after `since`.
    pub fn remove_rest(&self, guild: GuildId, member: MemberId, since: Since) {
        let mut entries = self.lock();
        if entries.revision(guild) == since.revision {
            entries.lru.remove(&(guild, member));
        }
    }

    /// Forgets a server's members (Quack left, or the server went away).
    pub fn clear_guild(&self, guild: GuildId) {
        let mut entries = self.lock();
        entries.bump(guild);
        entries.lru.retain(|(g, _)| *g != guild);
    }

    /// Forgets everything (`READY`: a new session starts from scratch).
    pub fn clear(&self) {
        let mut entries = self.lock();
        for revision in entries.revisions.values_mut() {
            *revision += 1;
        }
        entries.lru.clear();
    }

    /// How many members are cached.
    pub fn len(&self) -> usize {
        self.lock().lru.len()
    }

    /// Whether nothing is cached.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(id: u64) -> Member {
        Member {
            id: MemberId::new(id).unwrap(),
            bot: false,
            username: format!("user{id}"),
            global_name: None,
            avatar_hash: None,
            nick: None,
            role_ids: vec![],
            joined_at: None,
            timeout_until: None,
        }
    }

    #[test]
    fn event_entries_need_a_trusted_server_and_rest_entries_expire() {
        let g = GuildId::new(1).unwrap();
        let cache = MemberCache::new(10, Duration::from_millis(30));
        cache.put_event(g, member(5));
        assert_eq!(
            cache.get(g, MemberId::new(5).unwrap(), true),
            Some(member(5))
        );
        assert_eq!(cache.get(g, MemberId::new(5).unwrap(), false), None);
        cache.put_rest(g, member(6), cache.revision(g));
        assert!(cache.get(g, MemberId::new(6).unwrap(), false).is_some());
        std::thread::sleep(Duration::from_millis(40));
        assert!(cache.get(g, MemberId::new(6).unwrap(), true).is_none());
        cache.remove(g, MemberId::new(5).unwrap());
        assert!(cache.get(g, MemberId::new(5).unwrap(), true).is_none());
    }

    #[test]
    fn a_rest_answer_older_than_a_gateway_change_is_not_kept() {
        let g = GuildId::new(1).unwrap();
        let id = MemberId::new(5).unwrap();
        let cache = MemberCache::new(10, MemberCache::REST_TTL);
        // A GET starts, the member leaves, then the old 200 arrives.
        let since = cache.revision(g);
        cache.remove(g, id);
        cache.put_rest(g, member(5), since);
        assert_eq!(cache.get(g, id, true), None, "not resurrected");
        // A GET starts, the member rejoins, then the old 404 arrives.
        let since = cache.revision(g);
        cache.put_event(g, member(5));
        cache.remove_rest(g, id, since);
        assert!(cache.get(g, id, true).is_some(), "the rejoin stands");
        // Nothing in between: kept.
        let since = cache.revision(g);
        cache.put_rest(g, member(7), since);
        assert!(cache.get(g, MemberId::new(7).unwrap(), true).is_some());
    }

    #[test]
    fn bounded_and_cleared_per_server() {
        let (a, b) = (GuildId::new(1).unwrap(), GuildId::new(2).unwrap());
        let cache = MemberCache::new(3, MemberCache::REST_TTL);
        for id in 1..=4 {
            cache.put_event(a, member(id));
        }
        assert_eq!(cache.len(), 3);
        assert!(cache.get(a, MemberId::new(1).unwrap(), true).is_none());
        cache.put_event(b, member(9));
        cache.clear_guild(a);
        assert_eq!(cache.len(), 1);
        cache.clear();
        assert!(cache.is_empty());
    }
}
