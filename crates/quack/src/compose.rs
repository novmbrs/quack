//! The in-process application: the engine over a Discord port, the
//! optional modules, the bot, and the HTTP API. `serve` composes it over
//! the Discord REST client and feeds the bot from the gateway; the
//! whole-system tests compose the same thing over the fake port and feed
//! the bot typed events directly.

use std::sync::Arc;

use quack_bot::{Bot, Responder};
use quack_db::Db;
use quack_discord::cache::GuildCache;
use quack_engine::jobs::ModuleJobs;
use quack_engine::port::Discord;
use quack_engine::{Engine, EngineConfig};

use crate::config::Config;
use crate::serve::Readiness;
use crate::wiring::{self, ApiSetupError, Modules};

/// Everything `serve` runs in process besides the gateway, the job runner,
/// and the listeners.
pub struct App {
    /// The engine.
    pub engine: Engine,
    /// The bot the gateway (or a test) feeds.
    pub bot: Bot,
    /// The HTTP application (API, dashboard files, `/livez`, `/readyz`).
    pub router: axum::Router,
    /// The modules' process-wide state.
    pub modules: Arc<Modules>,
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("bot", &self.bot)
            .field("modules", &self.modules)
            .finish_non_exhaustive()
    }
}

impl App {
    /// Composes the application over `db` and the Discord `port`, answering
    /// interactions through `responder`. `guilds` lets the ticket journal
    /// tell threads from channels without waiting; `discord_api_base`
    /// replaces Discord's REST base for sign-in (the test seams).
    pub fn compose(
        config: &Config,
        db: Db,
        port: Arc<dyn Discord>,
        responder: Arc<dyn Responder>,
        guilds: Arc<GuildCache>,
        ready: &Arc<Readiness>,
        discord_api_base: Option<&str>,
    ) -> Result<Self, ApiSetupError> {
        let engine = Engine::new(
            db,
            port,
            EngineConfig {
                public_url: config.public_url.clone().unwrap_or_default(),
                session_key_fingerprint: config.session_key_fingerprint(),
            },
        );
        let modules = Arc::new(Modules::new(&engine));
        let bot = Bot::new(engine.clone(), responder, modules.adapters(guilds));
        let api = wiring::api_config(config, discord_api_base)?;
        let router = wiring::api_router(&engine, api, ready, &modules)?;
        Ok(Self {
            engine,
            bot,
            router,
            modules,
        })
    }

    /// The modules' job kinds and handlers, for the runner (§16.4).
    pub fn module_jobs(&self) -> Vec<ModuleJobs> {
        Modules::jobs(&self.engine)
    }
}
