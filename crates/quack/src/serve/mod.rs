//! `quack serve`: the startup and shutdown sequences of §17.5 and §16.7.
//!
//! Startup: database (migrate or verify), the singleton fence, HTTP, the
//! Discord REST client and its checks, command sync, the notification
//! listener and caches, the gateway, then the job runner and gauges once
//! the servers have loaded; `/readyz` turns ready when every check passes.
//!
//! Shutdown (SIGTERM/SIGINT, or the fence lost): one deadline 30 s after
//! the signal. Mark not ready; stop claiming jobs, stop the gateway, stop
//! accepting HTTP connections; drain interactions, requests, and jobs
//! until 5 s before the deadline; then refuse new Discord calls, close the
//! pool, and release the fence. The caller flushes telemetry.

pub mod http;
mod listen;
mod ready;
mod start;

pub use start::sync_commands;

use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

use crate::config::Config;

pub use ready::{GatewaySource, Readiness, WORKERS_FRESH};

/// The whole shutdown, from the signal.
pub const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(30);
/// Draining stops this long before the deadline.
pub const DRAIN_MARGIN: Duration = Duration::from_secs(5);
/// How long startup waits for the servers in `READY` to load.
const LOAD_WAIT: Duration = Duration::from_secs(30);

/// How `serve` reaches Discord.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiscordMode {
    /// Validate the token, check sharding, sync commands, connect the
    /// gateway.
    Connect,
    /// An internal test seam: no gateway and no startup calls; REST goes
    /// to `api_base` (a local fake). Readiness treats the gateway as
    /// connected.
    Off {
        /// The REST base URL.
        api_base: String,
    },
}

/// What `serve` is run with besides its configuration.
#[derive(Clone, Debug)]
pub struct Options {
    /// Apply pending migrations first (`--migrate`).
    pub migrate: bool,
    /// How Discord is reached.
    pub discord: DiscordMode,
}

/// Why `serve` stopped with an error (exit code 1).
#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    /// A startup step failed; the message says which.
    #[error("{0}")]
    Startup(String),
    /// The singleton fence connection was lost: another instance may act.
    #[error(
        "lost the database lock that keeps one Quack running; stopping so two never act at once"
    )]
    FenceLost,
}

/// Runs `serve` until `signal` completes (or the fence is lost), then
/// shuts down within [`SHUTDOWN_DEADLINE`]. `ready` is shared so callers
/// (and tests) can watch readiness.
pub async fn run(
    config: &Config,
    options: &Options,
    ready: Arc<Readiness>,
    signal: impl Future<Output = ()> + Send,
) -> Result<(), ServeError> {
    let stop = CancellationToken::new();
    let started = start::start(config, options, Arc::clone(&ready), stop.clone()).await?;
    let fence_lost = tokio::select! {
        () = signal => false,
        () = started.fence_lost.cancelled() => true,
    };
    tracing::info!(fence_lost, "shutting down");
    shut_down(started, &ready, &stop).await;
    if fence_lost {
        Err(ServeError::FenceLost)
    } else {
        Ok(())
    }
}

/// Steps 1–4 of the shutdown.
async fn shut_down(s: start::Started, ready: &Readiness, stop: &CancellationToken) {
    let deadline = Instant::now() + SHUTDOWN_DEADLINE;
    let drain = deadline
        .saturating_duration_since(Instant::now())
        .saturating_sub(DRAIN_MARGIN);
    Readiness::set(&ready.shutdown, true);
    // Stop admission everywhere at once; each part drains on its own.
    stop.cancel();
    let runner = async {
        if let Some(runner) = &s.runner {
            runner.shutdown(drain).await;
        }
    };
    let tasks = async {
        for task in s.tasks {
            let _ = task.await;
        }
    };
    if tokio::time::timeout(drain, async { tokio::join!(runner, tasks) })
        .await
        .is_err()
    {
        tracing::warn!("shutdown drain hit its deadline; dropping what is left");
    }
    s.client.close();
    s.db.close().await;
    if let Some(fence) = s.fence.lock().await.take()
        && let Err(error) = fence.release().await
    {
        tracing::warn!(%error, "couldn't release the fence cleanly");
    }
}
