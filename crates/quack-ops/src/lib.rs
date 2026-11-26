//! Ops in Discord (design §21): Quack posts its own operations into
//! private channels of one ops server and answers `/ops` there.
//!
//! - [`Ops::start`] finds or creates the channels (§21.2), then runs the
//!   heartbeat, the alerts (§21.5), the error feed (§21.7), the gateway
//!   summary (§21.3), and the status boards (§21.6) until stopped.
//! - [`Ops::record_start`] and [`Ops::record_stop`] keep `ops_state` about
//!   the current run and post `ops.started`.
//! - [`Ops::server_joined`] and [`Ops::server_left`] post installs (§21.4).
//! - [`Ops::jobs`] registers `post_ops`, which delivers every durable post.
//! - [`Ops::status`], [`Ops::server`], [`Ops::dead_jobs`], [`Ops::requeue`],
//!   and [`Ops::dismiss`] answer `/ops` and the ops buttons (§21.8); the
//!   bot checks [`Ops::is_operator`] first.
//! - [`Ops::metrics_overview`], [`Ops::metrics_servers`], and
//!   [`Ops::metrics_jobs`] answer `/ops metrics` (§21.8).
//!
//! Only server metadata and counts cross servers here (§21.1).

mod answers;
mod body;
mod card;
mod channels;
mod errors;
mod metrics;
mod post;
mod status;
mod watch;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use quack_core::ids::{AccountId, ChannelId, GuildId};
use quack_db::DbError;
use quack_engine::Engine;
use quack_engine::jobs::ModuleJobs;
use time::OffsetDateTime;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub use answers::Answer;
pub use card::Rendered;
pub use channels::Channels;
pub use quack_db::ops::StopReason;

/// The ops settings (§17.7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpsConfig {
    /// The ops server.
    pub guild: GuildId,
    /// The operators (§21.1).
    pub operators: Vec<AccountId>,
    /// A trace link with `{trace_id}` for the error feed.
    pub trace_url: Option<String>,
    /// Where the public status board goes, if anywhere (§21.6).
    pub public_status: Option<ChannelId>,
    /// This build's version.
    pub version: String,
    /// This build's commit, when the build set one.
    pub commit: Option<String>,
}

/// What the gateway looks like right now, read from the binary's gateway.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GatewaySnapshot {
    /// Whether the session is connected.
    pub connected: bool,
    /// The last heartbeat round trip.
    pub latency: Option<Duration>,
    /// New sessions so far.
    pub identifies: u64,
    /// Resumed sessions so far.
    pub resumes: u64,
    /// Dropped connections so far.
    pub drops: u64,
    /// Failed reconnects so far.
    pub connect_failures: u64,
    /// Invalidated sessions so far.
    pub invalid_sessions: u64,
}

/// Reads the gateway (the binary passes one over its `GatewayStatus`).
pub type GatewayProbe = Arc<dyn Fn() -> GatewaySnapshot + Send + Sync>;

/// Ops for one process. Cheap to clone.
#[derive(Clone)]
pub struct Ops {
    inner: Arc<Inner>,
}

struct Inner {
    engine: Engine,
    config: OpsConfig,
    /// When this process started (the board's uptime).
    started_at: OffsetDateTime,
    /// The channels, once found or created.
    channels: Mutex<Option<Channels>>,
    gateway: Mutex<Option<GatewayProbe>>,
}

impl std::fmt::Debug for Ops {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ops")
            .field("guild", &self.inner.config.guild)
            .field("operators", &self.inner.config.operators.len())
            .finish_non_exhaustive()
    }
}

impl Ops {
    /// Ops over `engine`; nothing runs until [`Ops::start`].
    pub fn new(engine: Engine, config: OpsConfig) -> Self {
        Self {
            inner: Arc::new(Inner {
                engine,
                config,
                started_at: OffsetDateTime::now_utc(),
                channels: Mutex::new(None),
                gateway: Mutex::new(None),
            }),
        }
    }

    /// The ops server.
    pub fn guild(&self) -> GuildId {
        self.inner.config.guild
    }

    /// Whether `account` may use ops here: an operator, in the ops server.
    pub fn is_operator(&self, guild: Option<GuildId>, account: AccountId) -> bool {
        guild == Some(self.inner.config.guild) && self.inner.config.operators.contains(&account)
    }

    /// Lets the status board and the gateway summary read the gateway.
    pub fn set_gateway(&self, probe: GatewayProbe) {
        *self
            .inner
            .gateway
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(probe);
    }

    fn gateway(&self) -> Option<GatewaySnapshot> {
        self.inner
            .gateway
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|probe| probe())
    }

    fn engine(&self) -> &Engine {
        &self.inner.engine
    }

    fn config(&self) -> &OpsConfig {
        &self.inner.config
    }

    /// The channels, when they are ready.
    pub fn channels(&self) -> Option<Channels> {
        *self
            .inner
            .channels
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn set_channels(&self, channels: Channels) {
        *self
            .inner
            .channels
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(channels);
    }

    /// The `post_ops` job kind and its handler.
    pub fn jobs(&self) -> ModuleJobs {
        post::jobs(self.clone())
    }

    /// Records this run in `ops_state` and posts `ops.started` (§21.3).
    /// `ready_in` is the time from process start to ready.
    pub async fn record_start(
        &self,
        migrations_applied: usize,
        ready_in: Duration,
    ) -> Result<(), DbError> {
        post::started(self, migrations_applied, ready_in).await
    }

    /// Records a clean stop (never after a lost fence, §21.3).
    pub async fn record_stop(&self, reason: StopReason) {
        let result = async {
            let mut conn = self.engine().db().conn().await?;
            quack_db::ops::record_stop(&mut conn, reason).await
        }
        .await;
        if let Err(error) = result {
            tracing::warn!(%error, "couldn't record how this run stopped");
        }
    }

    /// Posts that Quack joined (or rejoined) a server (§21.4).
    pub async fn server_joined(&self, guild: GuildId, rejoined: bool) {
        post::install(self, guild, Some(rejoined)).await;
    }

    /// Posts that Quack left a server (§21.4).
    pub async fn server_left(&self, guild: GuildId) {
        post::install(self, guild, None).await;
    }

    /// Starts the channel setup and the periodic work, until `stop`.
    pub fn start(&self, stop: &CancellationToken) -> Vec<JoinHandle<()>> {
        watch::spawn(self, stop)
    }

    // One step each of the periodic work, which [`Ops::start`] runs on its
    // own; tests call them directly.

    /// Finds or creates the channels once; returns why they aren't ready.
    pub async fn prepare_channels(&self) -> Result<Channels, String> {
        let channels = channels::ensure(self).await.map_err(|e| e.to_string())?;
        self.set_channels(channels);
        Ok(channels)
    }

    /// Saves posts for new dead jobs and failed actions (§21.5).
    pub async fn check_alerts(&self) -> Result<(), DbError> {
        watch::alerts(self).await
    }

    /// Posts the minute's errors (§21.7).
    pub async fn flush_errors(&self) {
        errors::flush(self).await;
    }

    /// Edits or posts the status boards: the ops board and, when
    /// configured, the public one (§21.6).
    pub async fn update_board(&self) -> Result<(), DbError> {
        status::update(self).await
    }
}
