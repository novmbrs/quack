//! The state every handler shares: the engine, configuration, the OAuth
//! client, readiness facts, the rate limiters, and the account-servers
//! cache.

use std::sync::Arc;

use quack_engine::Engine;

use crate::auth::guild_cache::AccountGuildsCache;
use crate::auth::oauth::DiscordOauth;
use crate::config::{ApiConfig, ConfigError};
use crate::middleware::rate_limit::RateLimits;
use crate::readiness::ReadinessFacts;
use crate::routes::modules::ModuleServices;

/// Everything the API's handlers share. Cheap to clone.
#[derive(Clone)]
pub struct ApiState {
    inner: Arc<Inner>,
}

struct Inner {
    engine: Engine,
    config: ApiConfig,
    oauth: DiscordOauth,
    readiness: Arc<dyn ReadinessFacts>,
    limits: RateLimits,
    account_guilds: AccountGuildsCache,
    modules: ModuleServices,
    fingerprint: [u8; 8],
}

impl std::fmt::Debug for ApiState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiState")
            .field("config", &self.inner.config)
            .finish_non_exhaustive()
    }
}

impl ApiState {
    /// Builds the state after validating `config` (§17.7: the dashboard
    /// directory must hold `index.html`). `readiness` answers `/readyz`.
    pub fn new(
        engine: Engine,
        config: ApiConfig,
        readiness: Arc<dyn ReadinessFacts>,
    ) -> Result<Self, ConfigError> {
        config.validate()?;
        let oauth = DiscordOauth::new(&config.discord, &config.public_url)?;
        let fingerprint = config.session_secret.fingerprint();
        Ok(Self {
            inner: Arc::new(Inner {
                engine,
                config,
                oauth,
                readiness,
                limits: RateLimits::new(),
                account_guilds: AccountGuildsCache::new(),
                modules: ModuleServices::default(),
                fingerprint,
            }),
        })
    }

    /// The same state with the module services the module routes call
    /// (the integration point in [`ModuleServices`]).
    #[must_use]
    pub fn with_modules(self, modules: ModuleServices) -> Self {
        let inner = &self.inner;
        Self {
            inner: Arc::new(Inner {
                engine: inner.engine.clone(),
                config: inner.config.clone(),
                oauth: inner.oauth.clone(),
                readiness: inner.readiness.clone(),
                limits: RateLimits::new(),
                account_guilds: AccountGuildsCache::new(),
                modules,
                fingerprint: inner.fingerprint,
            }),
        }
    }

    /// The engine.
    pub fn engine(&self) -> &Engine {
        &self.inner.engine
    }

    /// The configuration.
    pub fn config(&self) -> &ApiConfig {
        &self.inner.config
    }

    pub(crate) fn oauth(&self) -> &DiscordOauth {
        &self.inner.oauth
    }

    pub(crate) fn readiness(&self) -> &dyn ReadinessFacts {
        self.inner.readiness.as_ref()
    }

    pub(crate) fn limits(&self) -> &RateLimits {
        &self.inner.limits
    }

    pub(crate) fn account_guilds(&self) -> &AccountGuildsCache {
        &self.inner.account_guilds
    }

    pub(crate) fn modules(&self) -> &ModuleServices {
        &self.inner.modules
    }

    /// The session secret's fingerprint (sessions sealed under another
    /// secret don't load).
    pub(crate) fn fingerprint(&self) -> &[u8; 8] {
        &self.inner.fingerprint
    }
}
