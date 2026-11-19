//! `quack serve`: the startup and shutdown sequences of §17.5 and §16.7.
//!
//! Startup: database (migrate or verify), the singleton fence, HTTP, the
//! Discord REST client and its checks, command sync, the notification
//! listener and caches, the gateway, then the job runner and gauges once
//! the servers have loaded; `/readyz` turns ready when every check passes.
//!
//! Shutdown (SIGTERM/SIGINT, the fence lost, or the gateway closed for
//! good): one deadline 30 s after
//! the signal. Mark not ready; stop claiming jobs, stop the gateway, stop
//! accepting HTTP connections; drain interactions, requests, and jobs
//! until 5 s before the deadline; then refuse new Discord calls, close the
//! pool, and release the fence. The caller flushes telemetry.

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
    /// The test seam (`quack serve --discord-offline <url>`, and the
    /// tests): no gateway and no startup calls; REST goes to `api_base`
    /// (a local fake). Readiness treats the gateway as connected.
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
    /// The gateway ended on its own (a terminal close): this process can't
    /// receive Discord events any more.
    #[error("the Discord gateway closed for good; stopping so a supervisor can start Quack again")]
    GatewayStopped,
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
    // A signal during startup (waiting for the fence, say) stops it too.
    let mut signal = std::pin::pin!(signal);
    let mut starting = std::pin::pin!(start::start(
        config,
        options,
        Arc::clone(&ready),
        stop.clone()
    ));
    let (started, signalled) = tokio::select! {
        started = &mut starting => (started?, false),
        () = &mut signal => {
            stop.cancel();
            match starting.await {
                Ok(started) => (started, true),
                Err(error) => {
                    tracing::info!(%error, "stopped during startup");
                    return Ok(());
                }
            }
        }
    };
    // A fence or gateway lost during startup has already cancelled `stop`
    // (and, for the fence, closed the Discord client); it ends the wait at
    // once.
    let cause = if signalled {
        Ok(())
    } else {
        tokio::select! {
            biased;
            () = started.fence_lost.cancelled() => Err(ServeError::FenceLost),
            () = started.gateway_lost.cancelled() => Err(ServeError::GatewayStopped),
            () = signal => Ok(()),
        }
    };
    tracing::info!(ok = cause.is_ok(), "shutting down");
    shut_down(started, &ready, &stop).await;
    cause
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
    let modules = s.modules.shutdown(drain);
    let mut tasks = s.tasks;
    let joined = async {
        for task in &mut tasks {
            let _ = task.await;
        }
    };
    if tokio::time::timeout(drain, async { tokio::join!(runner, modules, joined) })
        .await
        .is_err()
    {
        tracing::warn!("shutdown drain hit its deadline; dropping what is left");
    }
    // What is still running is stopped, not left detached.
    for task in &tasks {
        task.abort();
    }
    s.client.close();
    // The pool and the fence get what is left of the deadline.
    let close = async {
        s.db.close().await;
        if let Some(fence) = s.fence.lock().await.take()
            && let Err(error) = fence.release().await
        {
            tracing::warn!(%error, "couldn't release the fence cleanly");
        }
    };
    let left = deadline.saturating_duration_since(Instant::now());
    if tokio::time::timeout(left, close).await.is_err() {
        tracing::warn!("closing the pool and the fence hit the shutdown deadline");
    }
}
