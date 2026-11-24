//! A small least-recently-used map: a hash map for lookups and an ordered
//! map of use ticks for eviction. O(log n) per operation, no unsafe code.

use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;

#[derive(Debug)]
pub(crate) struct Lru<K, V> {
    entries: HashMap<K, (V, u64)>,
    order: BTreeMap<u64, K>,
    tick: u64,
    capacity: usize,
}

impl<K: Hash + Eq + Clone, V> Lru<K, V> {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: BTreeMap::new(),
            tick: 0,
            capacity: capacity.max(1),
        }
    }

    fn next_tick(&mut self) -> u64 {
        self.tick += 1;
        self.tick
    }

    /// Looks up an entry and marks it used.
    pub(crate) fn get(&mut self, key: &K) -> Option<&V> {
        let tick = self.next_tick();
        let (_, used) = self.entries.get_mut(key)?;
        self.order.remove(used);
        *used = tick;
        self.order.insert(tick, key.clone());
        self.entries.get(key).map(|(v, _)| v)
    }

    /// Looks up an entry without marking it used.
    pub(crate) fn peek(&self, key: &K) -> Option<&V> {
        self.entries.get(key).map(|(v, _)| v)
    }

    /// Inserts or replaces an entry; returns how many entries were evicted.
    pub(crate) fn insert(&mut self, key: K, value: V) -> usize {
        let tick = self.next_tick();
        if let Some((_, used)) = self.entries.insert(key.clone(), (value, tick)) {
            self.order.remove(&used);
        }
        self.order.insert(tick, key);
        let mut evicted = 0;
        while self.entries.len() > self.capacity {
            let Some((_, oldest)) = self.order.pop_first() else {
                break;
            };
            self.entries.remove(&oldest);
            evicted += 1;
        }
        evicted
    }

    pub(crate) fn remove(&mut self, key: &K) -> Option<V> {
        let (value, used) = self.entries.remove(key)?;
        self.order.remove(&used);
        Some(value)
    }

    pub(crate) fn retain(&mut self, mut keep: impl FnMut(&K) -> bool) {
        let order = &mut self.order;
        self.entries.retain(|k, (_, used)| {
            let kept = keep(k);
            if !kept {
                order.remove(used);
            }
            kept
        });
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_the_least_recently_used() {
        let mut lru = Lru::new(2);
        assert_eq!(lru.insert(1, "a"), 0);
        assert_eq!(lru.insert(2, "b"), 0);
        assert_eq!(lru.get(&1), Some(&"a"));
        assert_eq!(lru.insert(3, "c"), 1);
        assert_eq!(lru.get(&2), None);
        assert_eq!(lru.get(&1), Some(&"a"));
        assert_eq!(lru.insert(1, "A"), 0);
        assert_eq!(lru.len(), 2);
        assert_eq!(lru.remove(&1), Some("A"));
        lru.retain(|k| *k != 3);
        assert_eq!(lru.len(), 0);
        lru.insert(4, "d");
        lru.clear();
        assert_eq!(lru.len(), 0);
        assert_eq!(lru.order.len(), 0);
    }
}
