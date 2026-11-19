//! Display names for the dashboard's sentences (§12.0). Discord renders
//! people as mentions; plain text needs their names, read through the
//! `Discord` port's `accounts` for the people the name cache doesn't
//! already know.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use quack_core::ids::AccountId;
use quack_telemetry::labels::CacheName;
use quack_telemetry::metrics;
use quack_text::{Actor, Person};

use crate::Engine;

const CACHE: CacheName = CacheName::new("names");

/// How long a name read from Discord is reused.
const NAME_TTL: Duration = Duration::from_mins(5);

/// The most names the cache holds.
const CAPACITY: usize = 10_000;

/// Display names by account. An account without one renders with the
/// unknown-name wording.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Names(HashMap<AccountId, String>);

impl Names {
    /// Reads the display names (global name, else username) of `ids`: the
    /// cached ones from the engine's name cache, the rest with one
    /// `accounts` call. If Discord can't answer, those people stay
    /// unnamed: a name never fails a read.
    pub async fn read(engine: &Engine, mut ids: Vec<AccountId>) -> Self {
        ids.sort_unstable();
        ids.dedup();
        let cache = engine.name_cache();
        let (mut names, missing) = cache.get(&ids, Instant::now());
        if missing.is_empty() {
            return Self(names);
        }
        let found: HashMap<AccountId, String> = engine
            .discord()
            .accounts(&missing)
            .await
            .done()
            .into_iter()
            .flatten()
            .map(|a| (a.id, a.global_name.unwrap_or(a.username)))
            .collect();
        cache.put(&found, Instant::now());
        names.extend(found);
        Self(names)
    }

    /// The account as a person, named when known.
    pub fn person(&self, id: AccountId) -> Person {
        self.0
            .get(&id)
            .map_or_else(|| Person::unnamed(id), |name| Person::new(id, name.clone()))
    }

    /// Who acted: a person, or Quack for `None`.
    pub fn actor(&self, id: Option<AccountId>) -> Actor {
        id.map_or(Actor::Quack, |id| Actor::Person(self.person(id)))
    }
}

/// Display names Discord gave recently, for five minutes each and at most
/// 10,000 (§11.10). Names are per account, not per server. Only names
/// Discord returned are kept, so an account it couldn't read is asked
/// again next time.
#[derive(Debug, Default)]
pub(crate) struct NameCache {
    entries: Arc<Mutex<HashMap<AccountId, (String, Instant)>>>,
}

impl NameCache {
    /// An empty cache, with its size gauge installed.
    pub(crate) fn new() -> Self {
        let cache = Self::default();
        let entries = Arc::downgrade(&cache.entries);
        metrics().caches.observe_size(CACHE, move || {
            entries
                .upgrade()
                .map_or(0, |e| u64::try_from(lock(&e).len()).unwrap_or(u64::MAX))
        });
        cache
    }

    /// The names of `ids` still fresh at `now`, and the ids to read.
    fn get(&self, ids: &[AccountId], now: Instant) -> (HashMap<AccountId, String>, Vec<AccountId>) {
        let entries = lock(&self.entries);
        let mut names = HashMap::with_capacity(ids.len());
        let mut missing = Vec::new();
        for id in ids {
            match entries.get(id) {
                Some((name, until)) if *until > now => {
                    names.insert(*id, name.clone());
                }
                _ => missing.push(*id),
            }
        }
        drop(entries);
        let caches = &metrics().caches;
        for _ in 0..names.len() {
            caches.lookup(CACHE, true);
        }
        for _ in 0..missing.len() {
            caches.lookup(CACHE, false);
        }
        (names, missing)
    }

    /// Keeps names read at `now`. A full cache first drops what expired;
    /// if that isn't enough it starts over.
    fn put(&self, found: &HashMap<AccountId, String>, now: Instant) {
        if found.is_empty() {
            return;
        }
        let mut entries = lock(&self.entries);
        if entries.len() + found.len() > CAPACITY {
            entries.retain(|_, (_, until)| *until > now);
            if entries.len() + found.len() > CAPACITY {
                let dropped = u64::try_from(entries.len()).unwrap_or(u64::MAX);
                entries.clear();
                metrics().caches.evicted(CACHE, dropped);
            }
        }
        let until = now + NAME_TTL;
        for (id, name) in found.iter().take(CAPACITY) {
            entries.insert(*id, (name.clone(), until));
        }
    }
}

/// The lock, even after a panic elsewhere left it poisoned (the map is
/// always consistent between statements).
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests")]

    use super::*;

    fn a(id: u64) -> AccountId {
        AccountId::new(id).unwrap()
    }

    fn names(pairs: &[(u64, &str)]) -> HashMap<AccountId, String> {
        pairs
            .iter()
            .map(|(id, n)| (a(*id), (*n).to_owned()))
            .collect()
    }

    #[test]
    fn a_name_is_reused_for_five_minutes() {
        let cache = NameCache::default();
        let t = Instant::now();
        cache.put(&names(&[(1, "Ana")]), t);
        let (hit, missing) = cache.get(&[a(1), a(2)], t + Duration::from_mins(4));
        assert_eq!(hit, names(&[(1, "Ana")]));
        assert_eq!(missing, [a(2)]);
        let (hit, missing) = cache.get(&[a(1)], t + NAME_TTL);
        assert!(hit.is_empty());
        assert_eq!(missing, [a(1)]);
    }

    #[test]
    fn a_full_cache_drops_expired_names_first_then_starts_over() {
        let cache = NameCache::default();
        let t = Instant::now();
        let old: HashMap<_, _> = (1..=CAPACITY as u64)
            .map(|i| (a(i), format!("n{i}")))
            .collect();
        cache.put(&old, t);
        // Expired names make room.
        let later = t + NAME_TTL;
        cache.put(&names(&[(20_001, "new")]), later);
        assert_eq!(lock(&cache.entries).len(), 1);
        // Fresh names that don't fit are dropped all at once.
        let fresh: HashMap<_, _> = (1..CAPACITY as u64)
            .map(|i| (a(i), format!("n{i}")))
            .collect();
        cache.put(&fresh, later);
        cache.put(&names(&[(30_001, "x"), (30_002, "y")]), later);
        assert_eq!(lock(&cache.entries).len(), 2);
    }
}
