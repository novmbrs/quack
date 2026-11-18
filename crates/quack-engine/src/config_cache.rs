//! The config cache (§11.10, §17.6): each server's `guild_settings` and
//! its three module settings rows in memory, for autocomplete capability
//! checks, setup modals, honeypot filtering, and activity routing. Like the
//! rules cache it never grants access to a write: every write reads its
//! rows inside its transaction.
//!
//! Reloaded per server on `NOTIFY quack_settings`, and fully after the
//! listener reconnects. Each row carries its `version`, and a reload never
//! replaces a row with an older version, so a slow reload can't undo a
//! faster one.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use quack_core::ids::GuildId;
use quack_db::activity_log::{self, ActivityLogSettings};
use quack_db::honeypot::{self, HoneypotSettings};
use quack_db::notify::Notification;
use quack_db::settings::{self, GuildSettings};
use quack_db::tickets::{self, TicketSettings};
use quack_db::{Db, DbError, PgConnection, guilds};
use quack_telemetry::labels::CacheName;
use quack_telemetry::metrics;

const CACHE: CacheName = CacheName::new("config");

/// One server's configuration as the cache holds it. A module row is
/// `None` until the module was first configured.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuildConfig {
    /// `guild_settings`.
    pub settings: GuildSettings,
    /// `ticket_settings`.
    pub tickets: Option<TicketSettings>,
    /// `activity_log_settings` with its routes.
    pub activity_log: Option<ActivityLogSettings>,
    /// `honeypot_settings`.
    pub honeypot: Option<HoneypotSettings>,
}

impl GuildConfig {
    /// Reads a server's configuration; `None` when it has no settings row
    /// (not installed).
    pub async fn load(conn: &mut PgConnection, guild: GuildId) -> Result<Option<Self>, DbError> {
        let Some(settings) = settings::get(conn, guild).await? else {
            return Ok(None);
        };
        Ok(Some(Self {
            settings,
            tickets: tickets::settings(conn, guild).await?,
            activity_log: activity_log::settings(conn, guild, false).await?,
            honeypot: honeypot::settings(conn, guild).await?,
        }))
    }

    /// `self`, with each row replaced by `newer`'s only when that row's
    /// version is not older, so a slow reload can't undo a faster one.
    fn merged(&self, newer: Self) -> Self {
        fn pick<T>(old: Option<&T>, new: Option<T>, version: fn(&T) -> u64) -> Option<T>
        where
            T: Clone,
        {
            match (old, new) {
                (Some(old), Some(new)) if version(old) > version(&new) => Some(old.clone()),
                (old, None) => old.cloned(),
                (_, new) => new,
            }
        }
        Self {
            settings: if self.settings.version > newer.settings.version {
                self.settings.clone()
            } else {
                newer.settings
            },
            tickets: pick(self.tickets.as_ref(), newer.tickets, |t| t.version),
            activity_log: pick(self.activity_log.as_ref(), newer.activity_log, |a| {
                a.version
            }),
            honeypot: pick(self.honeypot.as_ref(), newer.honeypot, |h| h.version),
        }
    }
}

/// Every installed server's configuration, in memory.
#[derive(Debug, Default)]
pub struct ConfigCache {
    guilds: Arc<RwLock<HashMap<GuildId, Arc<GuildConfig>>>>,
}

impl ConfigCache {
    /// An empty cache, with its size gauge installed.
    pub fn new() -> Self {
        let cache = Self::default();
        let guilds = Arc::clone(&cache.guilds);
        metrics().caches.observe_size(CACHE, move || {
            guilds
                .read()
                .map_or(0, |g| u64::try_from(g.len()).unwrap_or(u64::MAX))
        });
        cache
    }

    /// Loads every installed server (startup, and after the listener
    /// reconnects).
    pub async fn load_all(&self, db: &Db) -> Result<(), DbError> {
        let mut conn = db.conn().await?;
        for guild in guilds::installed(&mut conn).await? {
            if let Some(config) = GuildConfig::load(&mut conn, guild).await? {
                self.store(guild, config);
            }
        }
        Ok(())
    }

    /// Reloads one server (a `quack_settings` notification). `None` when
    /// it has no settings (it is forgotten).
    pub async fn reload(
        &self,
        db: &Db,
        guild: GuildId,
    ) -> Result<Option<Arc<GuildConfig>>, DbError> {
        let mut conn = db.conn().await?;
        let loaded = GuildConfig::load(&mut conn, guild).await?;
        if loaded.is_none() {
            self.forget(guild);
        }
        Ok(loaded.map(|config| self.store(guild, config)))
    }

    /// Reacts to a listener notification: reloads the server whose
    /// settings changed, or everything after a reconnect.
    pub async fn on_notification(
        &self,
        db: &Db,
        notification: Notification,
    ) -> Result<(), DbError> {
        match notification {
            Notification::Settings(guild) => self.reload(db, guild).await.map(|_| ()),
            Notification::Reconnected => self.load_all(db).await,
            Notification::Job(_) | Notification::Rules(_) => Ok(()),
        }
    }

    /// A server's configuration, loading it on a miss.
    pub async fn get(&self, db: &Db, guild: GuildId) -> Result<Option<Arc<GuildConfig>>, DbError> {
        if let Some(found) = self.cached(guild) {
            metrics().caches.lookup(CACHE, true);
            return Ok(Some(found));
        }
        metrics().caches.lookup(CACHE, false);
        self.reload(db, guild).await
    }

    /// A server's configuration if it is in memory.
    pub fn cached(&self, guild: GuildId) -> Option<Arc<GuildConfig>> {
        self.guilds.read().ok()?.get(&guild).cloned()
    }

    /// Forgets a server (Quack left it).
    pub fn forget(&self, guild: GuildId) {
        if let Ok(mut guilds) = self.guilds.write() {
            guilds.remove(&guild);
        }
    }

    fn store(&self, guild: GuildId, config: GuildConfig) -> Arc<GuildConfig> {
        let Ok(mut guilds) = self.guilds.write() else {
            return Arc::new(config);
        };
        let config = Arc::new(match guilds.get(&guild) {
            Some(existing) => existing.merged(config),
            None => config,
        });
        guilds.insert(guild, Arc::clone(&config));
        config
    }
}
