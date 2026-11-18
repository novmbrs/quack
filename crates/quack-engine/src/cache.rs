//! The rules cache (§11.10, §17.6): each server's rules in memory, for
//! autocomplete, pickers, `/case add` modals, and early validation. It
//! never decides which version a case uses (§3.1); the case transaction
//! reads the rule row under lock.
//!
//! Reloaded per server on `NOTIFY quack_rules`, and fully after the
//! listener reconnects. A reload never replaces a newer entry, so a slow
//! reload can't undo a faster one.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use quack_core::ids::{GuildId, RuleId};
use quack_core::rule::RuleDefinition;
use quack_db::notify::Notification;
use quack_db::rules::{self, Rule};
use quack_db::{Db, DbError, guilds};
use quack_telemetry::labels::CacheName;
use quack_telemetry::metrics;
use time::OffsetDateTime;

/// Most autocomplete choices Discord shows.
pub const MAX_CHOICES: usize = 25;
/// Longest choice label Discord accepts, in characters.
const CHOICE_LABEL_CHARS: usize = 100;
const CACHE: CacheName = CacheName::new("rules");

/// One active rule, with its current definition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CachedRule {
    /// The rule.
    pub id: RuleId,
    /// Its current version.
    pub version: u32,
    /// The current version's definition.
    pub definition: RuleDefinition,
}

/// An archived rule: only its name and description are kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchivedRule {
    /// The rule.
    pub id: RuleId,
    /// Its name.
    pub name: String,
    /// Its description.
    pub description: String,
}

/// One server's rules as the cache holds them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuildRules {
    /// Active rules, by name.
    pub active: Vec<CachedRule>,
    /// Archived rules (loaded on a per-server reload).
    pub archived: Vec<ArchivedRule>,
    stamp: Option<OffsetDateTime>,
}

impl GuildRules {
    /// An active rule by id.
    pub fn active(&self, id: RuleId) -> Option<&CachedRule> {
        self.active.iter().find(|r| r.id == id)
    }

    /// Whether an archived rule has this id.
    pub fn is_archived(&self, id: RuleId) -> bool {
        self.archived.iter().any(|r| r.id == id)
    }

    /// The active rule whose name equals `name`, case-insensitively and
    /// exactly (typed text that was not picked, §11.1).
    pub fn active_by_name(&self, name: &str) -> Option<&CachedRule> {
        let name = name.trim().to_lowercase();
        self.active
            .iter()
            .find(|r| r.definition.name.to_lowercase() == name)
    }

    fn from_rules(active: Vec<Rule>, archived: Vec<Rule>) -> Self {
        let stamp = active.iter().chain(&archived).map(|r| r.updated_at).max();
        let mut active: Vec<CachedRule> = active
            .into_iter()
            .map(|r| CachedRule {
                id: r.id,
                version: r.current_version,
                definition: r.definition,
            })
            .collect();
        active.sort_by_key(|r| r.definition.name.to_lowercase());
        Self {
            active,
            archived: archived
                .into_iter()
                .map(|r| ArchivedRule {
                    id: r.id,
                    name: r.definition.name,
                    description: r.definition.description,
                })
                .collect(),
            stamp,
        }
    }
}

/// One autocomplete choice: the name (cut to 100 characters) and the id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    /// The label.
    pub name: String,
    /// The rule.
    pub rule: RuleId,
}

/// Every server's rules, in memory.
#[derive(Debug, Default)]
pub struct RulesCache {
    guilds: Arc<RwLock<HashMap<GuildId, Arc<GuildRules>>>>,
}

impl RulesCache {
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

    /// Loads every installed server's active rules in one query (startup,
    /// and after the listener reconnects).
    pub async fn load_all(&self, db: &Db) -> Result<(), DbError> {
        let mut conn = db.conn().await?;
        let installed = guilds::installed(&mut conn).await?;
        let rules = rules::load_cache(&mut conn, &installed).await?;
        let mut by_guild: HashMap<GuildId, Vec<Rule>> =
            installed.iter().map(|g| (*g, Vec::new())).collect();
        for rule in rules {
            by_guild.entry(rule.guild).or_default().push(rule);
        }
        for (guild, active) in by_guild {
            self.store(guild, GuildRules::from_rules(active, Vec::new()));
        }
        Ok(())
    }

    /// Reloads one server (a `quack_rules` notification).
    pub async fn reload(&self, db: &Db, guild: GuildId) -> Result<Arc<GuildRules>, DbError> {
        let mut conn = db.conn().await?;
        let active = rules::list(&mut conn, guild, false).await?;
        let archived = rules::list(&mut conn, guild, true).await?;
        Ok(self.store(guild, GuildRules::from_rules(active, archived)))
    }

    /// Reacts to a listener notification: reloads the server whose rules
    /// changed, or everything after a reconnect.
    pub async fn on_notification(
        &self,
        db: &Db,
        notification: Notification,
    ) -> Result<(), DbError> {
        match notification {
            Notification::Rules(guild) => self.reload(db, guild).await.map(|_| ()),
            Notification::Reconnected => self.load_all(db).await,
            Notification::Job(_) | Notification::Settings(_) => Ok(()),
        }
    }

    /// A server's rules, loading them on a miss.
    pub async fn get(&self, db: &Db, guild: GuildId) -> Result<Arc<GuildRules>, DbError> {
        if let Some(found) = self.cached(guild) {
            metrics().caches.lookup(CACHE, true);
            return Ok(found);
        }
        metrics().caches.lookup(CACHE, false);
        self.reload(db, guild).await
    }

    /// A server's rules if they are in memory.
    pub fn cached(&self, guild: GuildId) -> Option<Arc<GuildRules>> {
        self.guilds.read().ok()?.get(&guild).cloned()
    }

    /// Forgets a server (Quack left it).
    pub fn forget(&self, guild: GuildId) {
        if let Ok(mut guilds) = self.guilds.write() {
            guilds.remove(&guild);
        }
    }

    /// Autocomplete for a `rule` option, from memory only (§11.1):
    /// case-insensitive substring match on name and description, at most
    /// 25 choices; archived rules only when asked (`/case list`).
    pub fn autocomplete(&self, guild: GuildId, query: &str, include_archived: bool) -> Vec<Choice> {
        let Some(rules) = self.cached(guild) else {
            return Vec::new();
        };
        let query = query.trim().to_lowercase();
        let matches = |name: &str, description: &str| {
            query.is_empty()
                || name.to_lowercase().contains(&query)
                || description.to_lowercase().contains(&query)
        };
        let active = rules
            .active
            .iter()
            .filter(|r| matches(&r.definition.name, &r.definition.description))
            .map(|r| (r.id, r.definition.name.as_str()));
        let archived = rules
            .archived
            .iter()
            .filter(|r| include_archived && matches(&r.name, &r.description))
            .map(|r| (r.id, r.name.as_str()));
        active
            .chain(archived)
            .take(MAX_CHOICES)
            .map(|(rule, name)| Choice {
                name: name.chars().take(CHOICE_LABEL_CHARS).collect(),
                rule,
            })
            .collect()
    }

    fn store(&self, guild: GuildId, rules: GuildRules) -> Arc<GuildRules> {
        let rules = Arc::new(rules);
        let Ok(mut guilds) = self.guilds.write() else {
            return rules;
        };
        match guilds.get(&guild) {
            Some(existing) if existing.stamp > rules.stamp => Arc::clone(existing),
            _ => {
                guilds.insert(guild, Arc::clone(&rules));
                rules
            }
        }
    }
}
