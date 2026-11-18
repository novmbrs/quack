//! The startup sequence (§17.5 steps 3–10).

use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use quack_bot::{Bot, CommandMentions, DiscordResponder};
use quack_db::jobs::{Runner, RunnerConfig, ensure_housekeeping};
use quack_db::notify::Listener;
use quack_db::{Db, Fence};
use quack_discord::cache::{GuildCache, MemberCache};
use quack_discord::commands::{Scope, definitions, sync};
use quack_discord::facts::Facts;
use quack_discord::gateway::{Gateway, check_shards};
use quack_discord::{ClientConfig, DiscordClient, Outcome};
use quack_engine::gauges::Gauges;
use quack_engine::{Engine, EngineConfig};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::ready::{self, GatewaySource, Readiness};
use super::{DiscordMode, LOAD_WAIT, Options, ServeError, listen};
use crate::config::Config;
use crate::port::DiscordPort;
use crate::wiring::{self, Modules};

/// How often a held-elsewhere fence is tried again.
const FENCE_RETRY: Duration = Duration::from_secs(5);

/// What startup leaves running.
pub(super) struct Started {
    pub(super) db: Db,
    pub(super) client: DiscordClient,
    pub(super) runner: Option<Runner>,
    pub(super) tasks: Vec<JoinHandle<()>>,
    pub(super) fence: Arc<Mutex<Option<Fence>>>,
    pub(super) fence_lost: CancellationToken,
    pub(super) modules: Arc<Modules>,
}

fn failed(what: &'static str) -> impl FnOnce(quack_db::DbError) -> ServeError {
    move |error| ServeError::Startup(format!("{what}: {error}"))
}

/// Step 3: connect, migrate or verify.
async fn database(config: &Config, options: &Options, ready: &Readiness) -> Result<Db, ServeError> {
    let db = Db::connect(
        config.database_url.expose(),
        config.database_max_connections,
    )
    .await
    .map_err(failed("couldn't connect to the database"))?;
    if options.migrate {
        let applied = quack_db::migrate(&db)
            .await
            .map_err(failed("couldn't apply migrations"))?;
        tracing::info!(count = applied.len(), "applied migrations");
    }
    match quack_db::verify_migrations(&db).await {
        Ok(Ok(())) => Readiness::set(&ready.migrations, true),
        Ok(Err(mismatch)) => {
            return Err(ServeError::Startup(format!(
                "the database's migrations don't match this build ({mismatch:?}); run `quack migrate` or start with --migrate"
            )));
        }
        Err(error) => return Err(failed("couldn't check migrations")(error)),
    }
    Ok(db)
}

/// Step 4: the singleton fence, retried every 5 s while another process
/// holds it; then a watcher that cancels `lost` if it is ever lost.
async fn fence(
    db: &Db,
    ready: &Readiness,
    stop: &CancellationToken,
) -> Result<(Arc<Mutex<Option<Fence>>>, CancellationToken, JoinHandle<()>), ServeError> {
    let held = loop {
        match Fence::try_acquire(db.connect_options()).await {
            Ok(Some(fence)) => break fence,
            Ok(None) => {
                tracing::info!("another Quack holds the database lock; waiting for it to stop");
            }
            Err(error) => tracing::warn!(%error, "couldn't try the database lock"),
        }
        tokio::select! {
            () = stop.cancelled() => return Err(ServeError::Startup("stopped while waiting for the database lock".to_owned())),
            () = tokio::time::sleep(FENCE_RETRY) => {}
        }
    };
    Readiness::set(&ready.fence, true);
    let fence = Arc::new(Mutex::new(Some(held)));
    let lost = CancellationToken::new();
    let watch = {
        let (fence, lost, stop) = (Arc::clone(&fence), lost.clone(), stop.clone());
        tokio::spawn(async move {
            let mut guard = fence.lock().await;
            let Some(held) = guard.as_mut() else { return };
            tokio::select! {
                () = stop.cancelled() => {}
                () = held.lost() => {
                    tracing::error!("lost the database lock");
                    lost.cancel();
                }
            }
        })
    };
    Ok((fence, lost, watch))
}

/// Step 6: the REST client; with Discord connected, the token and shard
/// checks.
async fn rest(config: &Config, options: &Options) -> Result<DiscordClient, ServeError> {
    let discord = config.discord.as_ref().ok_or_else(|| {
        ServeError::Startup("the Discord token and application id are required".to_owned())
    })?;
    let mut client_config = ClientConfig::new(discord.token.expose(), discord.application_id);
    if let DiscordMode::Off { api_base } = &options.discord {
        client_config.api_base.clone_from(api_base);
    }
    let client = DiscordClient::new(&client_config)
        .map_err(|e| ServeError::Startup(format!("couldn't build the Discord client: {e}")))?;
    if options.discord == DiscordMode::Connect {
        match client.current_user().await {
            Outcome::Done(me) => tracing::info!(account = me.id.get(), "signed in to Discord"),
            other => {
                return Err(ServeError::Startup(format!(
                    "Discord didn't accept QUACK_DISCORD_TOKEN ({})",
                    match other {
                        Outcome::Refused(r) => r.machine_code(),
                        _ => "no answer".to_owned(),
                    }
                )));
            }
        }
        check_shards(&client)
            .await
            .map_err(|e| ServeError::Startup(e.to_string()))?;
    }
    Ok(client)
}

/// Step 7 (and `quack commands sync`): command sync (§11.2).
pub async fn sync_commands(
    config: &Config,
    client: &DiscordClient,
) -> Result<quack_text::CommandIds, ServeError> {
    let scope = config.dev_guild_id.map_or(Scope::Global, Scope::Guild);
    let report = sync(client, scope, &definitions())
        .await
        .map_err(|e| ServeError::Startup(format!("couldn't register the slash commands: {e}")))?;
    tracing::info!(changed = report.changed, "slash commands are registered");
    Ok(report.ids)
}

/// The in-process pieces built from the client: caches, the engine over
/// the `DiscordPort`, the modules, and the bot.
struct Parts {
    engine: Engine,
    mentions: CommandMentions,
    guilds: Arc<GuildCache>,
    members: Arc<MemberCache>,
    modules: Arc<Modules>,
}

impl Parts {
    fn new(config: &Config, db: &Db, client: &DiscordClient) -> Self {
        let mentions = CommandMentions::default();
        let guilds = Arc::new(GuildCache::new());
        let members = Arc::new(MemberCache::new(
            MemberCache::CAPACITY,
            MemberCache::REST_TTL,
        ));
        let facts = Facts::new(client.clone(), Arc::clone(&guilds), Arc::clone(&members));
        let engine = Engine::new(
            db.clone(),
            Arc::new(DiscordPort::new(facts, mentions.clone())),
            EngineConfig {
                public_url: config.public_url.clone().unwrap_or_default(),
                session_key_fingerprint: config.session_key_fingerprint(),
            },
        );
        let modules = Arc::new(Modules::new(&engine));
        Self {
            engine,
            mentions,
            guilds,
            members,
            modules,
        }
    }

    fn bot(&self, client: &DiscordClient) -> Bot {
        Bot::new(
            self.engine.clone(),
            Arc::new(DiscordResponder::new(client.clone(), self.mentions.clone())),
            self.modules.adapters(Arc::clone(&self.guilds)),
        )
    }
}

/// Step 5: bind and serve HTTP (the API, `/livez` at once, `/readyz`
/// false until ready) until `stop`.
async fn http(
    config: &Config,
    options: &Options,
    ready: &Arc<Readiness>,
    engine: &Engine,
    stop: &CancellationToken,
) -> Result<JoinHandle<()>, ServeError> {
    let discord_base = match &options.discord {
        DiscordMode::Off { api_base } => Some(api_base.as_str()),
        DiscordMode::Connect => None,
    };
    let api =
        wiring::api_config(config, discord_base).map_err(|e| ServeError::Startup(e.to_string()))?;
    let app = wiring::api_router(engine, api, ready)
        .map_err(|e| ServeError::Startup(format!("couldn't start the API: {e}")))?;
    let listener = tokio::net::TcpListener::bind(config.listen_addr)
        .await
        .map_err(|e| {
            ServeError::Startup(format!("couldn't listen on {}: {e}", config.listen_addr))
        })?;
    let stop = stop.clone();
    Ok(tokio::spawn(async move {
        let shutdown = async move { stop.cancelled().await };
        if let Err(error) = quack_api::serve(listener, app, shutdown).await {
            tracing::error!(%error, "the HTTP server stopped");
        }
    }))
}

/// Step 8: the listener, the caches, and the routing loop.
async fn notifications(
    engine: &Engine,
    runner: &Arc<OnceLock<Runner>>,
    stop: &CancellationToken,
) -> Result<JoinHandle<()>, ServeError> {
    let db = engine.db();
    let listener = Listener::connect(db.connect_options())
        .await
        .map_err(failed("couldn't listen for notifications"))?;
    engine
        .rules_cache()
        .load_all(db)
        .await
        .map_err(failed("couldn't load the rules"))?;
    engine
        .config_cache()
        .load_all(db)
        .await
        .map_err(failed("couldn't load the settings"))?;
    Ok(tokio::spawn(listen::route(
        listener,
        engine.clone(),
        Arc::clone(runner),
        stop.clone(),
    )))
}

/// Step 9: the gateway shard feeding the bot; waits for the servers to
/// load. Off (the test seam), the gateway counts as connected.
async fn gateway(
    config: &Config,
    options: &Options,
    parts: &Parts,
    bot: Bot,
    stop: &CancellationToken,
    tasks: &mut Vec<JoinHandle<()>>,
) -> GatewaySource {
    let (DiscordMode::Connect, Some(discord)) = (&options.discord, &config.discord) else {
        return GatewaySource::Off;
    };
    let gateway = Gateway::new(
        discord.token.expose().to_owned(),
        Arc::clone(&parts.guilds),
        Arc::clone(&parts.members),
    );
    let status = gateway.status();
    let gateway_stop = stop.clone();
    let on_dropped = parts.modules.on_dropped();
    tasks.push(tokio::spawn(async move {
        let handler = move |event| {
            let bot = bot.clone();
            async move { bot.on_event(event).await }
        };
        if let Err(error) = gateway.run(handler, on_dropped, gateway_stop).await {
            tracing::error!(%error, "the gateway stopped");
        }
    }));
    wait_for_servers(parts.engine.db(), &parts.guilds, &status, stop).await;
    GatewaySource::Shard(status)
}

/// Runs the whole startup sequence.
pub(super) async fn start(
    config: &Config,
    options: &Options,
    ready: Arc<Readiness>,
    stop: CancellationToken,
) -> Result<Started, ServeError> {
    let db = database(config, options, &ready).await?;
    let (fence, fence_lost, fence_watch) = fence(&db, &ready, &stop).await?;
    let mut tasks = vec![fence_watch];
    let client = rest(config, options).await?;
    let parts = Parts::new(config, &db, &client);
    tasks.push(http(config, options, &ready, &parts.engine, &stop).await?);
    if options.discord == DiscordMode::Connect {
        parts.mentions.set(sync_commands(config, &client).await?);
    }
    let runner_cell = Arc::new(OnceLock::new());
    tasks.push(notifications(&parts.engine, &runner_cell, &stop).await?);
    let bot = parts.bot(&client);
    let gateway = gateway(config, options, &parts, bot, &stop, &mut tasks).await;
    tasks.extend(
        parts
            .modules
            .start(&parts.engine, &stop)
            .await
            .map_err(failed("couldn't start the modules"))?,
    );
    let runner = start_runner(config, &parts.engine, &db).await?;
    let _ = runner_cell.set(runner.clone());
    let gauges = Gauges::install();
    let (gauge_stop, gauge_db) = (stop.clone(), db.clone());
    tasks.push(tokio::spawn(async move {
        tokio::select! {
            () = gauge_stop.cancelled() => {}
            () = gauges.run(gauge_db) => {}
        }
    }));
    tasks.push(tokio::spawn(ready::refresh(
        Arc::clone(&ready),
        gateway,
        runner.clone(),
        stop.clone(),
    )));
    Ok(Started {
        db,
        client,
        runner: Some(runner),
        tasks,
        fence,
        fence_lost,
        modules: parts.modules,
    })
}

/// Waits until the session is connected and every installed server has
/// loaded, or 30 seconds (§17.5 step 9).
async fn wait_for_servers(
    db: &Db,
    guilds: &GuildCache,
    status: &quack_discord::gateway::GatewayStatus,
    stop: &CancellationToken,
) {
    let until = Instant::now() + LOAD_WAIT;
    let installed = match db.conn().await {
        Ok(mut conn) => quack_db::guilds::installed(&mut conn)
            .await
            .unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    while Instant::now() < until && !stop.is_cancelled() {
        if status.is_connected() && installed.iter().all(|g| guilds.is_trusted(*g)) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// The job runner with the engine's handlers and the modules' (§16.4).
async fn start_runner(config: &Config, engine: &Engine, db: &Db) -> Result<Runner, ServeError> {
    {
        let mut conn = db
            .conn()
            .await
            .map_err(failed("couldn't reach the database"))?;
        ensure_housekeeping(&mut conn)
            .await
            .map_err(failed("couldn't schedule housekeeping"))?;
    }
    let runner = quack_engine::jobs::runner(
        engine.clone(),
        RunnerConfig::new(config.worker_concurrency),
        Modules::jobs(engine),
    );
    runner.start();
    Ok(runner)
}
