//! The engine handle every use case and job handler runs on.

use std::sync::Arc;

use quack_core::ids::{AppealId, CaseId, GuildId, MemberId, RuleId};
use quack_db::Db;

use crate::cache::RulesCache;
use crate::config_cache::ConfigCache;
use crate::port::Discord;

/// What the engine needs to know about its deployment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineConfig {
    /// The dashboard's public origin (`QUACK_PUBLIC_URL`), without a
    /// trailing slash; links in messages point here.
    pub public_url: String,
    /// The first 8 bytes of SHA-256 of the session secret: housekeeping
    /// deletes sessions sealed under any other secret (§14.4).
    pub session_key_fingerprint: [u8; 8],
}

/// The database, the `Discord` port, the rules, config, and name caches,
/// and configuration.
/// Cheap to clone; every use case and job handler takes `&Engine`.
#[derive(Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}

struct Inner {
    db: Db,
    discord: Arc<dyn Discord>,
    rules: RulesCache,
    guild_config: ConfigCache,
    config: EngineConfig,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("config", &self.inner.config)
            .finish_non_exhaustive()
    }
}

impl Engine {
    /// Builds the engine. The rules and config caches start empty and fill
    /// per server on first use; [`RulesCache::load_all`] and
    /// [`ConfigCache::load_all`] warm them at startup.
    pub fn new(db: Db, discord: Arc<dyn Discord>, config: EngineConfig) -> Self {
        Self {
            inner: Arc::new(Inner {
                db,
                discord,
                rules: RulesCache::new(),
                guild_config: ConfigCache::new(),
                config,
            }),
        }
    }

    /// The database.
    pub fn db(&self) -> &Db {
        &self.inner.db
    }

    /// The `Discord` port.
    pub fn discord(&self) -> &dyn Discord {
        self.inner.discord.as_ref()
    }

    /// The in-memory rules cache (autocomplete, pickers, early checks).
    pub fn rules_cache(&self) -> &RulesCache {
        &self.inner.rules
    }

    /// The in-memory config cache: each server's settings and module
    /// settings (autocomplete checks, setup, honeypot filtering, activity
    /// routing). Writes never rely on it.
    pub fn config_cache(&self) -> &ConfigCache {
        &self.inner.guild_config
    }

    /// The configuration.
    pub fn config(&self) -> &EngineConfig {
        &self.inner.config
    }

    /// Dashboard links (§14.11).
    pub fn links(&self) -> Links<'_> {
        Links {
            base: &self.inner.config.public_url,
        }
    }
}

/// Builds dashboard URLs from the public origin (§14.11).
#[derive(Clone, Copy, Debug)]
pub struct Links<'a> {
    base: &'a str,
}

impl Links<'_> {
    /// `/guilds/{guild}/cases/{number}` (staff).
    pub fn case(&self, guild: GuildId, number: u64) -> String {
        format!("{}/guilds/{guild}/cases/{number}", self.base)
    }

    /// `/guilds/{guild}/cases/{case_id}/appeal` (the member's case page).
    pub fn member_case(&self, guild: GuildId, case: CaseId) -> String {
        format!("{}/guilds/{guild}/cases/{}/appeal", self.base, case.get())
    }

    /// `/guilds/{guild}/appeals/{appeal_id}` (staff).
    pub fn appeal(&self, guild: GuildId, appeal: AppealId) -> String {
        format!("{}/guilds/{guild}/appeals/{}", self.base, appeal.get())
    }

    /// `/guilds/{guild}/failures` (staff).
    pub fn failures(&self, guild: GuildId) -> String {
        format!("{}/guilds/{guild}/failures", self.base)
    }

    /// `/guilds/{guild}/members/{member}` (staff).
    pub fn member(&self, guild: GuildId, member: MemberId) -> String {
        format!("{}/guilds/{guild}/members/{member}", self.base)
    }

    /// `/guilds/{guild}/rules/{rule}` (staff).
    pub fn rule(&self, guild: GuildId, rule: RuleId) -> String {
        format!("{}/guilds/{guild}/rules/{}", self.base, rule.get())
    }

    /// `/guilds/{guild}/mod-log` (staff).
    pub fn mod_log(&self, guild: GuildId) -> String {
        format!("{}/guilds/{guild}/mod-log", self.base)
    }

    /// `/guilds/{guild}/settings` (staff).
    pub fn settings(&self, guild: GuildId) -> String {
        format!("{}/guilds/{guild}/settings", self.base)
    }
}
