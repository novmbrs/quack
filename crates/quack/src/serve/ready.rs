//! Readiness (§14.9, §17.5 step 10): the facts `/readyz` reports, flipped
//! by the startup and shutdown sequences and refreshed every second from
//! the gateway and the job runner.

use std::sync::Arc;
use std::time::Duration;

pub use quack_api::Readiness;
use quack_db::jobs::Runner;
use quack_discord::gateway::GatewayStatus;
use tokio_util::sync::CancellationToken;

/// How fresh every dispatcher's last pass must be.
pub const WORKERS_FRESH: Duration = Duration::from_secs(15);

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
