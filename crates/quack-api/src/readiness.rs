//! Readiness facts the binary owns (§14.9): migrations, the singleton
//! fence, the gateway, the job workers, and shutdown. The API checks the
//! database itself when `/readyz` is asked.

use std::sync::atomic::{AtomicBool, Ordering};

/// What `/readyz` asks the rest of the process. Implemented by the binary
/// over its gateway, fence, and runner; [`Readiness`] is a ready-made
/// implementation of flags the binary flips.
pub trait ReadinessFacts: Send + Sync + 'static {
    /// The applied migrations equal the binary's set (verified at start).
    fn migrations_current(&self) -> bool;
    /// This process holds the singleton lock (§17.5).
    fn fence_held(&self) -> bool;
    /// The shard's session is `READY` or `RESUMED`.
    fn gateway_ready(&self) -> bool;
    /// Every job dispatcher completed a pass within the last 15 s
    /// (`Runner::heartbeats_fresh`).
    fn workers_fresh(&self) -> bool;
    /// The process is shutting down.
    fn shutting_down(&self) -> bool;
}

/// Readiness flags the binary sets as things start and stop. All start
/// `false` (not ready, not shutting down).
#[derive(Debug, Default)]
pub struct Readiness {
    /// See [`ReadinessFacts::migrations_current`].
    pub migrations: AtomicBool,
    /// See [`ReadinessFacts::fence_held`].
    pub fence: AtomicBool,
    /// See [`ReadinessFacts::gateway_ready`].
    pub gateway: AtomicBool,
    /// See [`ReadinessFacts::workers_fresh`].
    pub workers: AtomicBool,
    /// See [`ReadinessFacts::shutting_down`].
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
}

impl ReadinessFacts for Readiness {
    fn migrations_current(&self) -> bool {
        self.migrations.load(Ordering::Relaxed)
    }
    fn fence_held(&self) -> bool {
        self.fence.load(Ordering::Relaxed)
    }
    fn gateway_ready(&self) -> bool {
        self.gateway.load(Ordering::Relaxed)
    }
    fn workers_fresh(&self) -> bool {
        self.workers.load(Ordering::Relaxed)
    }
    fn shutting_down(&self) -> bool {
        self.shutdown.load(Ordering::Relaxed)
    }
}
