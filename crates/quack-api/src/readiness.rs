//! Readiness facts the binary owns (§14.9): migrations, the singleton
//! fence, the gateway, the job workers, and shutdown. The API checks the
//! database itself when `/readyz` is asked.

use std::sync::atomic::{AtomicBool, Ordering};

/// Readiness flags the binary sets as things start and stop, and `/readyz`
/// reads. All start `false` (not ready, not shutting down). Writes release
/// and reads acquire.
#[derive(Debug, Default)]
pub struct Readiness {
    /// The applied migrations equal the binary's set (checked at start).
    pub migrations: AtomicBool,
    /// This process holds the singleton lock (§17.5).
    pub fence: AtomicBool,
    /// The shard's session is `READY` or `RESUMED`.
    pub gateway: AtomicBool,
    /// Every job dispatcher completed a pass within the last 15 s
    /// (`Runner::heartbeats_fresh`).
    pub workers: AtomicBool,
    /// Shutdown started.
    pub shutdown: AtomicBool,
}

impl Readiness {
    /// Flags with every check passing (tests and API-only runs).
    pub fn all_ready() -> Self {
        Self {
            migrations: AtomicBool::new(true),
            fence: AtomicBool::new(true),
            gateway: AtomicBool::new(true),
            workers: AtomicBool::new(true),
            shutdown: AtomicBool::new(false),
        }
    }

    /// Reads one flag.
    pub fn get(flag: &AtomicBool) -> bool {
        flag.load(Ordering::Acquire)
    }

    /// Sets one flag.
    pub fn set(flag: &AtomicBool, value: bool) {
        flag.store(value, Ordering::Release);
    }

    /// Whether every check passes.
    pub fn is_ready(&self) -> bool {
        [&self.migrations, &self.fence, &self.gateway, &self.workers]
            .into_iter()
            .all(Self::get)
            && !Self::get(&self.shutdown)
    }
}
