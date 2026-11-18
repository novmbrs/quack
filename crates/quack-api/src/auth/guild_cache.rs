//! The account-servers cache (§11.10): a session's Discord server list
//! for 60 s, at most 1,000 sessions, so the server picker doesn't call
//! Discord on every load.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::oauth::UserGuild;

const TTL: Duration = Duration::from_secs(60);
const CAPACITY: usize = 1_000;

/// One session's list and when it was fetched.
type Entry = (Instant, Arc<Vec<UserGuild>>);

/// Cached server lists, keyed by session ID hash.
#[derive(Debug, Default)]
pub(crate) struct AccountGuildsCache {
    entries: Mutex<HashMap<[u8; 32], Entry>>,
}

impl AccountGuildsCache {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// The session's list, if fetched less than 60 s ago.
    pub(crate) fn get(&self, session: &[u8; 32]) -> Option<Arc<Vec<UserGuild>>> {
        let entries = self.entries.lock().ok()?;
        entries
            .get(session)
            .filter(|(at, _)| at.elapsed() < TTL)
            .map(|(_, list)| list.clone())
    }

    /// Stores the session's list, evicting expired entries (then the
    /// oldest) when full.
    pub(crate) fn put(&self, session: [u8; 32], list: Arc<Vec<UserGuild>>) {
        let Ok(mut entries) = self.entries.lock() else {
            return;
        };
        if entries.len() >= CAPACITY {
            entries.retain(|_, (at, _)| at.elapsed() < TTL);
        }
        if entries.len() >= CAPACITY
            && let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, (at, _))| *at)
                .map(|(k, _)| *k)
        {
            entries.remove(&oldest);
        }
        entries.insert(session, (Instant::now(), list));
    }

    /// Forgets a session's list (logout).
    pub(crate) fn forget(&self, session: &[u8; 32]) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.remove(session);
        }
    }
}
