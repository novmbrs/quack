//! Turning a module on or off (§13.1.7). The engine runs the transaction;
//! each module supplies two hooks: why it can't be turned on (it locks and
//! reads its own settings row), and the writes that go with the change
//! (bumping its panel or warning).

use quack_core::ids::GuildId;
use quack_core::modlog::{
    ModLogEntry, ModLogLinks, ModuleDetail, ModuleDisabledDetail, ModuleSettingsChangedDetail,
};
use quack_core::modules::{ModuleName, ModuleProblem};
use quack_core::permissions::{Capability, ChannelFacts, GuildFacts};
use quack_db::operation_keys::OperationKey;
use quack_db::settings::set_module_enabled;
use quack_db::{DbError, Tx, notify};

use super::{ModuleState, SettingsError, module_state};
use crate::modlog::{self, By};
use crate::opkey::{Keyed, begin_keyed, complete, request_hash};
use crate::port::{BoxFuture, Outcome};
use crate::{Engine, OpKey, Principal, obs};

/// The Discord facts a module's enable check reads, gathered before the
/// transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleFacts {
    /// The server.
    pub guild: GuildFacts,
    /// Its channels.
    pub channels: Vec<ChannelFacts>,
}

/// Why a module can't be turned on (empty: it can), computed inside the
/// transaction from its locked settings row and the facts.
pub type EnableCheck = for<'a> fn(
    &'a mut Tx,
    GuildId,
    &'a ModuleFacts,
) -> BoxFuture<'a, Result<Vec<ModuleProblem>, DbError>>;

/// Writes that go with turning a module on (`true`) or off, inside that
/// transaction.
pub type EnabledChanged =
    for<'a> fn(&'a mut Tx, GuildId, bool) -> BoxFuture<'a, Result<(), DbError>>;

/// A module's enable and disable hooks.
#[derive(Clone, Copy, Debug)]
pub struct ModuleHooks {
    /// Which module.
    pub name: ModuleName,
    /// Its enable check.
    pub enable_problems: EnableCheck,
    /// Its writes on a change.
    pub on_enabled_changed: EnabledChanged,
}

/// Turns a module on or off (§13.1.7). Turning it on runs the module's
/// enable check against its locked row; a module already in that state is
/// left alone and its state returned, with nothing written.
pub async fn set_enabled(
    engine: &Engine,
    principal: &Principal,
    hooks: &ModuleHooks,
    enabled: bool,
    op_key: &OpKey,
) -> Result<ModuleState, SettingsError> {
    let guild = principal.guild;
    obs::use_case("set_module_enabled", Some(guild), async {
        let mut conn = engine.db().conn().await?;
        principal
            .require(&mut conn, Capability::ManageSettings)
            .await??;
        drop(conn);
        let facts = if enabled {
            Some(facts(engine, guild).await?)
        } else {
            None
        };
        let hash = request_hash(&(hooks.name.as_str(), enabled));
        let Some((mut tx, key)) = begin_module_write(engine, principal, op_key, &hash).await?
        else {
            let mut conn = engine.db().conn().await?;
            return Ok(module_state(&mut conn, guild, hooks.name).await?);
        };
        if let Some(facts) = &facts {
            let problems = (hooks.enable_problems)(&mut tx, guild, facts).await?;
            if !problems.is_empty() {
                return Err(SettingsError::ModuleNotReady(problems));
            }
        }
        let changed = set_module_enabled(
            &mut tx,
            guild,
            hooks.name,
            enabled,
            None,
            Some(principal.account),
        )
        .await?;
        if changed.is_some() {
            (hooks.on_enabled_changed)(&mut tx, guild, enabled).await?;
            let entry = if enabled {
                ModLogEntry::ModuleEnabled(ModuleDetail { module: hooks.name })
            } else {
                ModLogEntry::ModuleDisabled(ModuleDisabledDetail {
                    module: hooks.name,
                    reason: None,
                })
            };
            let by = By {
                actor: Some(principal.account),
                source: principal.origin.mod_log_source(),
            };
            modlog::write(&mut tx, guild, entry, by, ModLogLinks::default()).await?;
            notify::settings_changed(&mut tx, guild).await?;
            complete(
                &mut tx,
                &key,
                200,
                serde_json::json!({ "module": hooks.name.as_str() }),
            )
            .await?;
            tx.commit().await?;
        } else {
            tx.rollback().await?;
        }
        let mut conn = engine.db().conn().await?;
        Ok(module_state(&mut conn, guild, hooks.name).await?)
    })
    .await
}

async fn facts(engine: &Engine, guild: GuildId) -> Result<ModuleFacts, SettingsError> {
    let discord = engine.discord();
    let Outcome::Done(guild_facts) = discord.guild_facts(guild).await else {
        return Err(SettingsError::DiscordUnavailable);
    };
    let Outcome::Done(channels) = discord.guild_channels(guild).await else {
        return Err(SettingsError::DiscordUnavailable);
    };
    Ok(ModuleFacts {
        guild: guild_facts,
        channels,
    })
}

/// Starts a module settings update (§13.1.2): reserves the operation key
/// and rechecks `manage_settings` inside the transaction. `None` is a
/// replay; the caller answers with the current settings.
pub async fn begin_module_write(
    engine: &Engine,
    principal: &Principal,
    op_key: &OpKey,
    hash: &[u8; 32],
) -> Result<Option<(Tx, OperationKey)>, SettingsError> {
    let key = op_key.scoped(Some(principal.guild), Some(principal.account));
    let Keyed::Fresh(mut tx) = begin_keyed(engine.db(), &key, hash).await? else {
        return Ok(None);
    };
    principal
        .require(&mut tx, Capability::ManageSettings)
        .await??;
    Ok(Some((tx, key)))
}

/// Ends a module settings update before its commit: logs the changed
/// fields, notifies the config caches, and records the operation key.
pub async fn finish_module_write(
    tx: &mut Tx,
    principal: &Principal,
    key: &OperationKey,
    module: ModuleName,
    fields: Vec<String>,
) -> Result<(), DbError> {
    let guild = principal.guild;
    let by = By {
        actor: Some(principal.account),
        source: principal.origin.mod_log_source(),
    };
    let entry = ModLogEntry::ModuleSettingsChanged(ModuleSettingsChangedDetail { module, fields });
    modlog::write(tx, guild, entry, by, ModLogLinks::default()).await?;
    notify::settings_changed(tx, guild).await?;
    let response = serde_json::json!({ "module": module.as_str() });
    complete(tx, key, 200, response).await
}
