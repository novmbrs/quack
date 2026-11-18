//! The member cache (§11.10): (server, account) → member, at most 200,000
//! entries, least recently used evicted.
//!
//! Event-fed entries are exact while the gateway is connected and the server
//! loaded, so callers pass whether the server is trusted; REST-fed entries
//! expire after 5 minutes. "Not a member" is never cached.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use quack_core::ids::{GuildId, MemberId};
use quack_telemetry::labels::CacheName;
use quack_telemetry::metrics;

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

/// The bounded member cache.
#[derive(Debug)]
pub struct MemberCache {
    entries: Mutex<Lru<(GuildId, MemberId), Entry>>,
    rest_ttl: Duration,
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
            entries: Mutex::new(Lru::new(capacity)),
            rest_ttl,
        }
    }

    fn lock(&self) -> MutexGuard<'_, Lru<(GuildId, MemberId), Entry>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Reports the cache's size on `quack.cache.size`.
    pub fn observe(self: &Arc<Self>) {
        let me = Arc::downgrade(self);
        metrics().caches.observe_size(NAME, move || {
            me.upgrade().map_or(0, |c| c.len() as u64)
        });
    }

    /// A cached member. Event-fed entries count only when `trusted` (the
    /// gateway is connected and the server loaded); REST-fed ones only
    /// while fresh.
    pub fn get(&self, guild: GuildId, member: MemberId, trusted: bool) -> Option<Member> {
        let found = {
            let mut entries = self.lock();
            entries.get(&(guild, member)).and_then(|e| {
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

    fn insert(&self, guild: GuildId, member: Member, fed: Fed) {
        let evicted = self.lock().insert((guild, member.id), Entry { member, fed });
        if evicted > 0 {
            metrics().caches.evicted(NAME, evicted as u64);
        }
    }

    /// Stores a member from a gateway event.
    pub fn put_event(&self, guild: GuildId, member: Member) {
        self.insert(guild, member, Fed::Event);
    }

    /// Stores a member Quack read over REST.
    pub fn put_rest(&self, guild: GuildId, member: Member) {
        self.insert(guild, member, Fed::Rest(Instant::now()));
    }

    /// Forgets a member (`GUILD_MEMBER_REMOVE`).
    pub fn remove(&self, guild: GuildId, member: MemberId) {
        self.lock().remove(&(guild, member));
    }

    /// Forgets a server's members (Quack left, or the server went away).
    pub fn clear_guild(&self, guild: GuildId) {
        self.lock().retain(|(g, _)| *g != guild);
    }

    /// Forgets everything (`READY`: a new session starts from scratch).
    pub fn clear(&self) {
        self.lock().clear();
    }

    /// How many members are cached.
    pub fn len(&self) -> usize {
        self.lock().len()
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
        assert_eq!(cache.get(g, MemberId::new(5).unwrap(), true), Some(member(5)));
        assert_eq!(cache.get(g, MemberId::new(5).unwrap(), false), None);
        cache.put_rest(g, member(6));
        assert!(cache.get(g, MemberId::new(6).unwrap(), false).is_some());
        std::thread::sleep(Duration::from_millis(40));
        assert!(cache.get(g, MemberId::new(6).unwrap(), true).is_none());
        cache.remove(g, MemberId::new(5).unwrap());
        assert!(cache.get(g, MemberId::new(5).unwrap(), true).is_none());
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
