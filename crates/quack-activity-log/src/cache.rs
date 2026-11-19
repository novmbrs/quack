//! The server activity log's message cache (§11.10, §13.3.2): the only
//! message cache in Quack. Bounded by entries and estimated bytes, entries
//! expire after 24 hours, and the least recently used go first. Text is
//! stored already redacted and only when the server shows message text.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use quack_core::ids::{ChannelId, GuildId, MessageId};
use quack_telemetry::labels::CacheName;
use quack_telemetry::metrics;
use quack_text::model::Person;

use crate::events::{AuthorKind, FileFacts};

/// The cache's name in `quack.cache.*`.
pub(crate) const NAME: CacheName = CacheName::new("activity_messages");
/// Entries kept at most.
pub(crate) const MAX_ENTRIES: usize = 50_000;
/// Estimated bytes kept at most (64 MiB).
pub(crate) const MAX_BYTES: usize = 64 * 1024 * 1024;
/// How long an entry lives.
pub(crate) const TTL: Duration = Duration::from_hours(24);
/// The fixed per-entry estimate on top of its strings.
const ENTRY_OVERHEAD: usize = 160;

/// A message's key.
pub(crate) type Key = (GuildId, ChannelId, MessageId);

/// What the cache knows about one message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Cached {
    /// Its author.
    pub author: Person,
    /// What kind of author; Quack's and webhooks' are tombstones.
    pub kind: AuthorKind,
    /// Posted in a channel that is itself a route destination: never
    /// posted.
    pub in_route_channel: bool,
    /// The redacted text; `None` when the server hid text when it was
    /// cached (or Quack never saw it).
    pub text: Option<String>,
    /// Its files (empty when the server hides them).
    pub files: Vec<FileFacts>,
    /// Its link preview types (empty when hidden).
    pub embeds: Vec<String>,
}

impl Cached {
    /// Quack's own, a webhook's, or in a route channel: recognized, never
    /// posted.
    pub(crate) fn tombstone(&self) -> bool {
        self.in_route_channel || matches!(self.kind, AuthorKind::Quack | AuthorKind::Webhook)
    }

    fn bytes(&self) -> usize {
        ENTRY_OVERHEAD
            + self.author.name.as_ref().map_or(0, String::len)
            + self.text.as_ref().map_or(0, String::len)
            + self
                .files
                .iter()
                .map(|f| f.name.len() + f.url.len() + 16)
                .sum::<usize>()
            + self.embeds.iter().map(String::len).sum::<usize>()
    }
}

struct Slot {
    tick: u64,
    at: Instant,
    bytes: usize,
    value: Cached,
}

/// The bounded LRU.
pub(crate) struct MessageCache {
    entries: HashMap<Key, Slot>,
    order: BTreeMap<u64, Key>,
    next_tick: u64,
    bytes: usize,
    max_entries: usize,
    max_bytes: usize,
    ttl: Duration,
}

impl MessageCache {
    /// An empty cache with these bounds.
    pub(crate) fn new(max_entries: usize, max_bytes: usize, ttl: Duration) -> Self {
        Self {
            entries: HashMap::new(),
            order: BTreeMap::new(),
            next_tick: 0,
            bytes: 0,
            max_entries,
            max_bytes,
            ttl,
        }
    }

    /// Entries held.
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Estimated bytes held.
    #[cfg(test)]
    pub(crate) fn bytes(&self) -> usize {
        self.bytes
    }

    fn tick(&mut self) -> u64 {
        self.next_tick += 1;
        self.next_tick
    }

    /// Stores a message (replacing an older copy), then evicts down to the
    /// bounds.
    pub(crate) fn put(&mut self, key: Key, value: Cached, now: Instant) {
        self.remove(&key);
        let bytes = value.bytes();
        let tick = self.tick();
        self.order.insert(tick, key);
        self.bytes += bytes;
        self.entries.insert(
            key,
            Slot {
                tick,
                at: now,
                bytes,
                value,
            },
        );
        self.evict(now);
    }

    /// A copy of a message, counting the lookup; expired entries are
    /// misses.
    pub(crate) fn get(&mut self, key: &Key, now: Instant) -> Option<Cached> {
        let expired = self
            .entries
            .get(key)
            .is_some_and(|s| now.duration_since(s.at) >= self.ttl);
        if expired {
            self.remove(key);
            metrics().caches.evicted(NAME, 1);
        }
        let tick = self.tick();
        let found = self.entries.get_mut(key).map(|slot| {
            let old = std::mem::replace(&mut slot.tick, tick);
            (old, slot.value.clone())
        });
        metrics().caches.lookup(NAME, found.is_some());
        let (old, value) = found?;
        self.order.remove(&old);
        self.order.insert(tick, *key);
        Some(value)
    }

    /// Takes a message out (a delete).
    pub(crate) fn take(&mut self, key: &Key, now: Instant) -> Option<Cached> {
        let value = self.get(key, now);
        self.remove(key);
        value
    }

    /// Forgets every message of a server (module off, Quack left).
    pub(crate) fn forget_guild(&mut self, guild: GuildId) -> usize {
        let keys: Vec<Key> = self
            .entries
            .keys()
            .filter(|(g, _, _)| *g == guild)
            .copied()
            .collect();
        for key in &keys {
            self.remove(key);
        }
        keys.len()
    }

    /// Whether any message of this server is held.
    pub(crate) fn has_guild(&self, guild: GuildId) -> bool {
        self.entries.keys().any(|(g, _, _)| *g == guild)
    }

    fn remove(&mut self, key: &Key) -> Option<Slot> {
        let slot = self.entries.remove(key)?;
        self.order.remove(&slot.tick);
        self.bytes -= slot.bytes;
        Some(slot)
    }

    /// Drops expired entries from the old end, then the least recently
    /// used until both bounds hold.
    fn evict(&mut self, now: Instant) {
        let mut evicted = 0_u64;
        while let Some((_, key)) = self.order.first_key_value() {
            let key = *key;
            let over = self.entries.len() > self.max_entries || self.bytes > self.max_bytes;
            let expired = self
                .entries
                .get(&key)
                .is_some_and(|s| now.duration_since(s.at) >= self.ttl);
            if !over && !expired {
                break;
            }
            self.remove(&key);
            evicted += 1;
        }
        if evicted > 0 {
            metrics().caches.evicted(NAME, evicted);
        }
    }
}

#[cfg(test)]
mod tests {
    use quack_core::ids::AccountId;

    use super::*;

    fn key(n: u64) -> Key {
        (
            GuildId::new(1).unwrap(),
            ChannelId::new(2).unwrap(),
            MessageId::new(n).unwrap(),
        )
    }

    fn message(text: &str) -> Cached {
        Cached {
            author: Person {
                id: AccountId::new(9).unwrap(),
                name: Some("duck".to_owned()),
            },
            kind: AuthorKind::Member,
            in_route_channel: false,
            text: Some(text.to_owned()),
            files: Vec::new(),
            embeds: Vec::new(),
        }
    }

    #[test]
    fn evicts_the_least_recently_used_past_the_entry_bound() {
        let mut cache = MessageCache::new(2, MAX_BYTES, TTL);
        let now = Instant::now();
        cache.put(key(1), message("a"), now);
        cache.put(key(2), message("b"), now);
        assert!(cache.get(&key(1), now).is_some(), "1 is now the newest");
        cache.put(key(3), message("c"), now);
        assert_eq!(cache.len(), 2);
        assert!(cache.get(&key(2), now).is_none(), "2 was evicted");
        assert!(cache.get(&key(1), now).is_some());
        assert!(cache.get(&key(3), now).is_some());
    }

    #[test]
    fn evicts_past_the_byte_bound() {
        let mut cache = MessageCache::new(100, 3 * (ENTRY_OVERHEAD + 1000), TTL);
        let now = Instant::now();
        for n in 1..=5 {
            cache.put(key(n), message(&"x".repeat(1000)), now);
        }
        assert!(cache.bytes() <= 3 * (ENTRY_OVERHEAD + 1000));
        assert_eq!(cache.len(), 2, "the author name counts too");
        assert!(cache.get(&key(5), now).is_some());
        assert!(cache.get(&key(1), now).is_none());
    }

    #[test]
    fn entries_expire() {
        let mut cache = MessageCache::new(10, MAX_BYTES, Duration::from_secs(60));
        let then = Instant::now();
        cache.put(key(1), message("a"), then);
        let later = then + Duration::from_secs(61);
        assert!(cache.get(&key(1), later).is_none());
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.bytes(), 0);
    }

    #[test]
    fn take_removes_and_servers_are_forgotten() {
        let mut cache = MessageCache::new(10, MAX_BYTES, TTL);
        let now = Instant::now();
        cache.put(key(1), message("a"), now);
        cache.put(key(2), message("b"), now);
        assert_eq!(cache.take(&key(1), now), Some(message("a")));
        assert!(cache.take(&key(1), now).is_none());
        assert!(cache.has_guild(GuildId::new(1).unwrap()));
        assert_eq!(cache.forget_guild(GuildId::new(1).unwrap()), 1);
        assert_eq!((cache.len(), cache.bytes()), (0, 0));
    }
}
