//! The interaction dedupe set (§11.3, §11.10): a repeated delivery of the
//! same interaction id within 15 minutes is dropped before the ack.
//! Writes are protected again by operation keys (§4.3), so this set only
//! saves a second, failing ack.

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use quack_telemetry::labels::CacheName;
use quack_telemetry::metrics;

/// The set's metric name.
const NAME: CacheName = CacheName::new("interactions");
/// Most ids remembered.
pub(crate) const CAPACITY: usize = 100_000;
/// How long an id is remembered (an interaction token's lifetime).
pub(crate) const TTL: Duration = Duration::from_mins(15);

#[derive(Debug, Default)]
struct Inner {
    seen: HashSet<u64>,
    order: VecDeque<(Instant, u64)>,
}

/// Interaction ids seen in the last 15 minutes, bounded.
#[derive(Debug, Default)]
pub(crate) struct Dedupe {
    inner: Mutex<Inner>,
}

impl Dedupe {
    /// Reports the set's size on `quack.cache.size`.
    pub(crate) fn observe(self: &Arc<Self>) {
        let me = Arc::downgrade(self);
        metrics()
            .caches
            .observe_size(NAME, move || me.upgrade().map_or(0, |d| d.len() as u64));
    }

    fn len(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .seen
            .len()
    }

    /// Records `id`; `false` when it was already seen (a duplicate).
    pub(crate) fn first(&self, id: u64, now: Instant) -> bool {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let mut evicted = 0_u64;
        while let Some(&(at, old)) = inner.order.front() {
            let expired = now.saturating_duration_since(at) >= TTL;
            if !expired && inner.order.len() < CAPACITY {
                break;
            }
            inner.order.pop_front();
            inner.seen.remove(&old);
            if !expired {
                evicted += 1;
            }
        }
        if evicted > 0 {
            metrics().caches.evicted(NAME, evicted);
        }
        let fresh = inner.seen.insert(id);
        metrics().caches.lookup(NAME, !fresh);
        if fresh {
            inner.order.push_back((now, id));
        }
        fresh
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_repeats_until_they_expire() {
        let d = Dedupe::default();
        let t0 = Instant::now();
        assert!(d.first(1, t0));
        assert!(!d.first(1, t0 + Duration::from_secs(60)));
        assert!(d.first(2, t0));
        assert!(d.first(1, t0 + TTL + Duration::from_secs(1)));
    }

    #[test]
    fn stays_bounded() {
        let d = Dedupe::default();
        let t0 = Instant::now();
        for id in 0..(CAPACITY as u64 + 10) {
            assert!(d.first(id, t0));
        }
        assert_eq!(d.len(), CAPACITY);
        assert!(d.first(0, t0), "the oldest id was evicted");
    }
}
