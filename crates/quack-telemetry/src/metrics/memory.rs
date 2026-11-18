//! Metrics for bounded in-memory caches and queues.
//!
//! Every bounded structure reports its size and what it drops, so a full
//! cache or queue is visible before it hurts.

use opentelemetry::metrics::{Counter, Meter, ObservableGauge};

use super::observe::{Sources, counter, u64_gauge};
use crate::catalog;
use crate::labels::{Attribute, CacheName, CacheResult, QueueName};

/// Cache metrics: size, lookups, and evictions.
pub struct CacheMetrics {
    lookups: Counter<u64>,
    evictions: Counter<u64>,
    sizes: Sources<CacheName, u64>,
    _size_gauge: ObservableGauge<u64>,
}

impl CacheMetrics {
    pub(crate) fn new(meter: &Meter) -> Self {
        let sizes: Sources<CacheName, u64> = Sources::new();
        let source = sizes.clone();
        let size_gauge = u64_gauge(meter, &catalog::CACHE_SIZE, move |observer| {
            source.for_each(|cache, size| observer.observe(size, &[cache.kv()]));
        });
        Self {
            lookups: counter(meter, &catalog::CACHE_LOOKUPS),
            evictions: counter(meter, &catalog::CACHE_EVICTIONS),
            sizes,
            _size_gauge: size_gauge,
        }
    }

    /// Counts one lookup.
    pub fn lookup(&self, cache: CacheName, hit: bool) {
        let result = if hit {
            CacheResult::Hit
        } else {
            CacheResult::Miss
        };
        self.lookups.add(1, &[cache.kv(), result.kv()]);
    }

    /// Counts entries dropped to stay within the cache's bound.
    pub fn evicted(&self, cache: CacheName, entries: u64) {
        self.evictions.add(entries, &[cache.kv()]);
    }

    /// Installs the size supplier for one cache, replacing an earlier one.
    pub fn observe_size(
        &self,
        cache: CacheName,
        supplier: impl Fn() -> u64 + Send + Sync + 'static,
    ) {
        self.sizes.set(cache, supplier);
    }
}

/// Queue metrics: depth and drops.
pub struct QueueMetrics {
    dropped: Counter<u64>,
    depths: Sources<QueueName, u64>,
    _depth_gauge: ObservableGauge<u64>,
}

impl QueueMetrics {
    pub(crate) fn new(meter: &Meter) -> Self {
        let depths: Sources<QueueName, u64> = Sources::new();
        let source = depths.clone();
        let depth_gauge = u64_gauge(meter, &catalog::QUEUE_DEPTH, move |observer| {
            source.for_each(|queue, depth| observer.observe(depth, &[queue.kv()]));
        });
        Self {
            dropped: counter(meter, &catalog::QUEUE_DROPPED),
            depths,
            _depth_gauge: depth_gauge,
        }
    }

    /// Counts items dropped because the queue was full.
    pub fn dropped(&self, queue: QueueName, items: u64) {
        self.dropped.add(items, &[queue.kv()]);
    }

    /// Installs the depth supplier for one queue, replacing an earlier one.
    pub fn observe_depth(
        &self,
        queue: QueueName,
        supplier: impl Fn() -> u64 + Send + Sync + 'static,
    ) {
        self.depths.set(queue, supplier);
    }
}
