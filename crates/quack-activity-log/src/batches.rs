//! Batches of deletes of messages Quack hadn't seen (§13.3.2): per channel
//! for 60 seconds or until 50 IDs, at most 1,000 open batches.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use quack_core::ids::{ChannelId, GuildId, MessageId};

/// How long a batch stays open.
pub(crate) const WINDOW: Duration = Duration::from_secs(60);
/// IDs a batch holds at most.
pub(crate) const MAX_IDS: usize = 50;
/// Batches open at once at most; past it deletes post at once.
pub(crate) const MAX_OPEN: usize = 1_000;

/// One channel's batch.
type Where = (GuildId, ChannelId);

/// What adding a delete did.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Added {
    /// It waits in its channel's batch.
    Batched,
    /// The batch is full: post these IDs now.
    Full(Vec<MessageId>),
    /// Too many batches are open: post this one now, unbatched.
    Unbatched,
}

struct Batch {
    ids: Vec<MessageId>,
    opened: Instant,
}

/// The open batches.
#[derive(Default)]
pub(crate) struct Batches {
    open: HashMap<Where, Batch>,
}

impl Batches {
    /// Adds an uncached delete.
    pub(crate) fn add(&mut self, at: Where, id: MessageId, now: Instant) -> Added {
        if !self.open.contains_key(&at) && self.open.len() >= MAX_OPEN {
            return Added::Unbatched;
        }
        let batch = self.open.entry(at).or_insert_with(|| Batch {
            ids: Vec::new(),
            opened: now,
        });
        if !batch.ids.contains(&id) {
            batch.ids.push(id);
        }
        if batch.ids.len() >= MAX_IDS {
            let full = self.open.remove(&at).map_or_default(|b| b.ids);
            return Added::Full(full);
        }
        Added::Batched
    }

    /// Takes the batches whose window ended (every batch when `all`).
    pub(crate) fn due(&mut self, now: Instant, all: bool) -> Vec<(Where, Vec<MessageId>)> {
        let due: Vec<Where> = self
            .open
            .iter()
            .filter(|(_, b)| all || now.duration_since(b.opened) >= WINDOW)
            .map(|(k, _)| *k)
            .collect();
        due.into_iter()
            .filter_map(|k| self.open.remove(&k).map(|b| (k, b.ids)))
            .collect()
    }

    /// Drops a server's batches.
    pub(crate) fn forget_guild(&mut self, guild: GuildId) {
        self.open.retain(|(g, _), _| *g != guild);
    }

    /// Batches open now.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.open.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(c: u64) -> Where {
        (GuildId::new(1).unwrap(), ChannelId::new(c).unwrap())
    }

    fn id(n: u64) -> MessageId {
        MessageId::new(n).unwrap()
    }

    #[test]
    fn batches_close_after_the_window_or_at_50() {
        let mut b = Batches::default();
        let now = Instant::now();
        for n in 1..MAX_IDS as u64 {
            assert_eq!(b.add(at(1), id(n), now), Added::Batched);
        }
        let Added::Full(ids) = b.add(at(1), id(50), now) else {
            panic!("expected a full batch");
        };
        assert_eq!(ids.len(), MAX_IDS);
        b.add(at(2), id(1), now);
        assert_eq!(b.due(now, false).len(), 0);
        assert_eq!(b.due(now + WINDOW, false).len(), 1);
        assert_eq!(b.len(), 0);
    }

    #[test]
    fn past_the_open_limit_deletes_post_at_once() {
        let mut b = Batches::default();
        let now = Instant::now();
        for c in 1..=MAX_OPEN as u64 {
            b.add(at(c), id(1), now);
        }
        assert_eq!(b.add(at(5_000), id(1), now), Added::Unbatched);
        assert_eq!(
            b.add(at(1), id(2), now),
            Added::Batched,
            "an open one still takes more"
        );
        b.forget_guild(GuildId::new(1).unwrap());
        assert_eq!(b.len(), 0);
    }
}
