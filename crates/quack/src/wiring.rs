//! Where the optional modules, the HTTP API, and the MySQL importer are
//! connected to `serve` and the command line.
//!
//! - [`Modules`]: the modules' process-wide state (the ticket journal, the
//!   server activity log), their bot adapters, job handlers, API hooks,
//!   the dropped-event hook, and their startup and shutdown steps.
//! - [`api_router`]: the HTTP API (with `/livez` and `/readyz`) over the
//!   engine, configured from [`Config`].
//! - [`import_mysql`]: `quack import-mysql`.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use quack_activity_log::ActivityLog;
use quack_api::{
    ApiConfig, ApiState, DiscordOauthConfig, ModuleServices, PublicUrl, SessionSecret,
};
use quack_bot::Module;
use quack_bot::modules::{ThreadLookup, TicketGaps};
use quack_discord::cache::GuildCache;
use quack_discord::gateway::Event;
use quack_engine::Engine;
use quack_engine::jobs::ModuleJobs;
use quack_tickets::{Journal, OpenGauge};
use time::OffsetDateTime;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::config::Config;
use crate::serve::Readiness;

/// The optional modules' state for one process.
pub struct Modules {
    journal: Arc<Journal>,
    activity: ActivityLog,
    gaps: TicketGaps,
    /// The activity log's sender tasks, aborted after its shutdown.
    senders: Mutex<Vec<JoinHandle<()>>>,
}

impl std::fmt::Debug for Modules {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Modules")
            .field("activity", &self.activity)
            .field("gaps", &self.gaps)
            .finish_non_exhaustive()
    }
}

impl Modules {
    /// The modules' state over `engine`; nothing runs until [`Self::start`].
    pub fn new(engine: &Engine) -> Self {
        let journal = Arc::new(Journal::new());
        Self {
            gaps: TicketGaps::new(Arc::clone(&journal)),
            journal,
            activity: ActivityLog::new(engine.clone()),
            senders: Mutex::new(Vec::new()),
        }
    }

    /// The bot's module adapters (§13.1.6). `guilds` tells the ticket
    /// journal a thread from a channel without waiting.
    pub fn adapters(&self, guilds: Arc<GuildCache>) -> Vec<Arc<dyn Module>> {
        let threads: ThreadLookup = Arc::new(move |guild, channel| {
            match guilds.channel_facts(guild, channel, OffsetDateTime::now_utc()) {
                Some(Some(c)) if matches!(c.kind, 10..=12) => {
                    Some(Some(c.parent_id.unwrap_or(channel)))
                }
                Some(Some(_)) => Some(None),
                // Untrusted, or a channel the cache doesn't have: the
                // journal looks the thread up.
                Some(None) | None => None,
            }
        });
        vec![
            quack_bot::modules::tickets(Arc::clone(&self.journal), threads),
            quack_bot::modules::activity_log(self.activity.clone()),
            quack_bot::modules::honeypot(),
        ]
    }

    /// The modules' job kinds and handlers (§16.4). The server activity
    /// log has none: its delivery is in memory.
    pub fn jobs(engine: &Engine) -> Vec<ModuleJobs> {
        vec![
            quack_tickets::jobs(engine.clone()),
            quack_honeypot::jobs(engine.clone()),
        ]
    }

    /// What the API's module routes call: the enable and disable hooks
    /// (§13.1.7) and the activity log's counters.
    pub fn services(&self) -> ModuleServices {
        ModuleServices {
            tickets: Some(quack_tickets::hooks()),
            activity_log: Some(quack_activity_log::hooks()),
            honeypot: Some(quack_honeypot::hooks()),
            activity: Some(self.activity.clone()),
        }
    }

    /// Runs in the shard loop for each event a full queue dropped (§11.9):
    /// a dropped message in a ticket thread sets that ticket's
    /// `journal_gaps`. Only remembers the channel; a task records it.
    pub fn on_dropped(&self) -> impl Fn(&Event) + Send + Sync + 'static {
        let gaps = self.gaps.clone();
        move |event| gaps.on_dropped(event)
    }

    /// Startup (§17.5): every pending honeypot hit gets its job, the
    /// activity log's senders start, and the open-tickets gauge and the
    /// journal-gap recorder run until `stop`.
    pub async fn start(
        &self,
        engine: &Engine,
        stop: &CancellationToken,
    ) -> Result<Vec<JoinHandle<()>>, quack_db::DbError> {
        let members = quack_honeypot::recover(engine).await?;
        if members > 0 {
            tracing::info!(members, "re-queued pending honeypot hits");
        }
        *self.senders.lock().unwrap_or_else(PoisonError::into_inner) = self.activity.spawn();
        let gauge = OpenGauge::install();
        let (gauge_stop, gauge_db) = (stop.clone(), engine.db().clone());
        let (gaps, gaps_stop, gaps_engine) = (self.gaps.clone(), stop.clone(), engine.clone());
        Ok(vec![
            tokio::spawn(async move {
                tokio::select! {
                    () = gauge_stop.cancelled() => {}
                    () = gauge.run(gauge_db) => {}
                }
            }),
            tokio::spawn(async move {
                tokio::select! {
                    () = gaps_stop.cancelled() => {}
                    () = gaps.run(gaps_engine) => {}
                }
            }),
        ])
    }

    /// Shutdown (§17.5): posts the activity log's open delete batches and
    /// drains its queue within `deadline`, then stops its senders.
    pub async fn shutdown(&self, deadline: Duration) {
        self.activity.shutdown(deadline).await;
        for task in self
            .senders
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain(..)
        {
            task.abort();
        }
    }
}

/// Why the API couldn't be configured.
#[derive(Debug, thiserror::Error)]
pub enum ApiSetupError {
    /// A required key is missing.
    #[error("{0} is required to serve the dashboard and its API")]
    Missing(&'static str),
    /// The API refused the configuration.
    #[error(transparent)]
    Invalid(#[from] quack_api::ConfigError),
}

/// The API's configuration from the binary's. `discord_api_base` replaces
/// Discord's REST base for the OAuth calls (the test seam).
pub fn api_config(
    config: &Config,
    discord_api_base: Option<&str>,
) -> Result<ApiConfig, ApiSetupError> {
    let public_url = config
        .public_url
        .as_deref()
        .ok_or(ApiSetupError::Missing("QUACK_PUBLIC_URL"))?;
    let secret = config
        .session_secret
        .as_ref()
        .ok_or(ApiSetupError::Missing("QUACK_SESSION_SECRET"))?;
    let discord = config
        .discord
        .as_ref()
        .ok_or(ApiSetupError::Missing("QUACK_DISCORD_APPLICATION_ID"))?;
    let client_secret = config
        .discord_client_secret
        .as_ref()
        .ok_or(ApiSetupError::Missing("QUACK_DISCORD_CLIENT_SECRET"))?;
    let mut oauth =
        DiscordOauthConfig::discord(discord.application_id.to_string(), client_secret.expose());
    if let Some(base) = discord_api_base {
        base.clone_into(&mut oauth.api_base);
    }
    Ok(ApiConfig {
        public_url: PublicUrl::parse(public_url)?,
        session_secret: SessionSecret::from_base64(secret.expose())?,
        discord: oauth,
        trusted_proxy_hops: config.trusted_proxy_hops,
        dashboard_dir: Some(config.dashboard_dir.clone()),
    })
}

/// The HTTP application (§14): the API with the modules' services, the
/// dashboard's files, `/livez`, and `/readyz` from `ready`.
pub fn api_router(
    engine: &Engine,
    api: ApiConfig,
    ready: &Arc<Readiness>,
    modules: &Modules,
) -> Result<axum::Router, ApiSetupError> {
    let state =
        ApiState::new(engine.clone(), api, Arc::clone(ready))?.with_modules(modules.services());
    Ok(quack_api::router(state))
}

/// `quack import-mysql` (§19): imports, prints the report's summary, and
/// fails when the report is not ok.
pub async fn import_mysql(
    config: &Config,
    dry_run: bool,
    report: Option<&std::path::Path>,
) -> anyhow::Result<()> {
    let source = config
        .import_source_url
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("QUACK_IMPORT_SOURCE_URL is required"))?;
    let db = quack_db::Db::connect(config.database_url.expose(), 4).await?;
    let options = quack_import::ImportOptions {
        source_url: source.expose().to_owned(),
        dry_run,
        report_path: report.map(std::path::Path::to_path_buf),
    };
    let result = quack_import::run(&db, &options).await;
    db.close().await;
    let report = result?;
    println!("{}", report.summary());
    if report.ok {
        Ok(())
    } else {
        anyhow::bail!("the import didn't pass its checks; see the report above")
    }
}
