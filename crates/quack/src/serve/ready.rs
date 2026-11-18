//! Readiness (§14.9, §17.5 step 10): the facts `/readyz` reports, flipped
//! by the startup and shutdown sequences and refreshed every second from
//! the gateway and the job runner.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use quack_db::jobs::Runner;
use quack_discord::gateway::GatewayStatus;
use tokio_util::sync::CancellationToken;

/// How fresh every dispatcher's last pass must be.
pub const WORKERS_FRESH: Duration = Duration::from_secs(15);

/// Readiness flags. All start `false` (not ready, not shutting down).
#[derive(Debug, Default)]
pub struct Readiness {
    /// The applied migrations equal the binary's set (checked at start).
    pub migrations: AtomicBool,
    /// This process holds the singleton fence.
    pub fence: AtomicBool,
    /// The gateway session is `READY` or `RESUMED`.
    pub gateway: AtomicBool,
    /// Every job dispatcher completed a pass in the last 15 seconds.
    pub workers: AtomicBool,
    /// Shutdown started.
    pub shutdown: AtomicBool,
}

impl Readiness {
    /// Whether every check passes.
    pub fn is_ready(&self) -> bool {
        self.migrations.load(Ordering::Acquire)
            && self.fence.load(Ordering::Acquire)
            && self.gateway.load(Ordering::Acquire)
            && self.workers.load(Ordering::Acquire)
            && !self.shutdown.load(Ordering::Acquire)
    }

    /// Each check by name, for `/readyz`.
    pub fn checks(&self) -> [(&'static str, bool); 5] {
        [
            ("migrations", self.migrations.load(Ordering::Acquire)),
            ("fence", self.fence.load(Ordering::Acquire)),
            ("gateway", self.gateway.load(Ordering::Acquire)),
            ("workers", self.workers.load(Ordering::Acquire)),
            ("shutdown", !self.shutdown.load(Ordering::Acquire)),
        ]
    }

    /// Sets one flag.
    pub fn set(flag: &AtomicBool, value: bool) {
        flag.store(value, Ordering::Release);
    }
}

/// Where the gateway flag comes from.
#[derive(Clone, Debug)]
pub enum GatewaySource {
    /// The shard's connection state.
    Shard(Arc<GatewayStatus>),
    /// No gateway in this run (a test seam): counts as connected.
    Off,
}

/// Refreshes the gateway and workers flags every second until `stop`.
pub(super) async fn refresh(
    ready: Arc<Readiness>,
    gateway: GatewaySource,
    runner: Runner,
    stop: CancellationToken,
) {
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            () = stop.cancelled() => return,
            _ = tick.tick() => {}
        }
        let connected = match &gateway {
            GatewaySource::Shard(status) => status.is_connected(),
            GatewaySource::Off => true,
        };
        Readiness::set(&ready.gateway, connected);
        Readiness::set(&ready.workers, runner.heartbeats_fresh(WORKERS_FRESH));
    }
}
