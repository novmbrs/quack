//! The account cache (§11.10): account → username, global name, avatar,
//! bot flag, at most 100,000 entries, least recently used evicted.
//!
//! Accounts aren't per server, and every sighting refreshes one: gateway
//! members and their updates, interaction authors, message authors, and
//! every member or user Quack reads over REST. An entry is used for 24
//! hours after it was last seen: a member's rename arrives as a member
//! update, so the limit only ages out people who share no server with
//! Quack. "Unknown user" is never cached.

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use quack_core::ids::AccountId;
use quack_core::permissions::AccountFacts;
use quack_telemetry::labels::CacheName;
use quack_telemetry::metrics;

use super::lru::Lru;
use crate::model::Member;

/// The metric name of this cache.
pub(crate) const NAME: CacheName = CacheName::new("accounts");

/// The bounded account cache.
#[derive(Debug)]
pub struct AccountCache {
    entries: Mutex<Lru<AccountId, (AccountFacts, Instant)>>,
    ttl: Duration,
}

impl Default for AccountCache {
    fn default() -> Self {
        Self::new(Self::CAPACITY, Self::TTL)
    }
}

impl AccountCache {
    /// The design's bound.
    pub const CAPACITY: usize = 100_000;
    /// How long after its last sighting an account may be used.
    pub const TTL: Duration = Duration::from_hours(24);

    /// An empty cache.
    pub fn new(capacity: usize, ttl: Duration) -> Self {
        Self {
            entries: Mutex::new(Lru::new(capacity)),
            ttl,
        }
    }

    fn lock(&self) -> MutexGuard<'_, Lru<AccountId, (AccountFacts, Instant)>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A cached account seen within the time limit.
    pub fn get(&self, id: AccountId) -> Option<AccountFacts> {
        let found = self
            .lock()
            .get(&id)
            .filter(|(_, seen)| seen.elapsed() < self.ttl)
            .map(|(account, _)| account.clone());
        metrics().caches.lookup(NAME, found.is_some());
        found
    }

    /// Stores an account as Discord described it at `seen` (for a REST
    /// read, when the request started), unless the cache already holds a
    /// later sighting: a slow read never overwrites a newer gateway update.
    pub fn put(&self, account: AccountFacts, seen: Instant) {
        let mut entries = self.lock();
        if entries
            .get(&account.id)
            .is_some_and(|(_, held)| *held > seen)
        {
            return;
        }
        let evicted = entries.insert(account.id, (account, seen));
        drop(entries);
        if evicted > 0 {
            metrics().caches.evicted(NAME, evicted as u64);
        }
    }

    /// Stores a member's account as seen at `seen` (see [`Self::put`]).
    pub fn put_member(&self, member: &Member, seen: Instant) {
        self.put(
            AccountFacts {
                id: member.id.account(),
                username: member.username.clone(),
                global_name: member.global_name.clone(),
                avatar_hash: member.avatar_hash.clone(),
                bot: member.bot,
            },
            seen,
        );
    }

    /// How many accounts are cached.
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

    fn account(id: u64, name: &str) -> AccountFacts {
        AccountFacts {
            id: AccountId::new(id).unwrap(),
            username: name.to_owned(),
            global_name: None,
            avatar_hash: None,
            bot: false,
        }
    }

    #[test]
    fn a_sighting_is_used_until_it_expires_and_a_newer_one_replaces_it() {
        let cache = AccountCache::new(10, Duration::from_millis(30));
        let id = AccountId::new(1).unwrap();
        assert_eq!(cache.get(id), None);
        cache.put(account(1, "ana"), Instant::now());
        cache.put(account(1, "ana2"), Instant::now());
        assert_eq!(cache.get(id), Some(account(1, "ana2")));
        std::thread::sleep(Duration::from_millis(40));
        assert_eq!(cache.get(id), None);
    }

    #[test]
    fn a_read_that_started_before_a_newer_sighting_does_not_replace_it() {
        let cache = AccountCache::new(10, AccountCache::TTL);
        let id = AccountId::new(1).unwrap();
        let started = Instant::now();
        cache.put(account(1, "renamed"), Instant::now());
        cache.put(account(1, "old"), started);
        assert_eq!(cache.get(id), Some(account(1, "renamed")));
        cache.put(account(1, "newest"), Instant::now());
        assert_eq!(cache.get(id), Some(account(1, "newest")));
    }

    #[test]
    fn bounded() {
        let cache = AccountCache::new(2, AccountCache::TTL);
        for id in 1..=3 {
            cache.put(account(id, "x"), Instant::now());
        }
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.get(AccountId::new(1).unwrap()), None);
    }
}
