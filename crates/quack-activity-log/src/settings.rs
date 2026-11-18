//! Server activity log settings (§13.3.1): privacy flags and one channel
//! per event, the one-transaction update, `/setup`'s "route everything
//! here", and the enable check.

use std::collections::BTreeMap;

use quack_core::DomainError;
use quack_core::ids::{ChannelId, GuildId, RoleId};
use quack_core::modlog::{ModLogEntry, ModLogLinks, ModuleSettingsChangedDetail};
use quack_core::modules::{ActivityEvent, ModuleDisabledReason, ModuleName, ModuleProblem};
use quack_core::permissions::{Capability, ChannelProblem, Permission, Permissions};
use quack_db::activity_log::{self, ActivityFlags, ActivityLogSettings};
use quack_db::{DbError, PgConnection, Tx, notify};
use quack_engine::channels::{self, ChannelCheck, Gathered};
use quack_engine::modlog::{self, By};
use quack_engine::opkey::{Keyed, begin_keyed, complete, request_hash};
use quack_engine::port::{BoxFuture, Outcome};
use quack_engine::settings::{FieldProblem, ModuleFacts, ModuleHooks, SettingsError};
use quack_engine::{Engine, OpKey, Principal};
use serde::{Deserialize, Serialize};

use crate::obs::span;

/// What Quack needs in a route channel.
fn needs() -> Permissions {
    Permission::ViewChannel | Permission::SendMessages
}

/// The server activity log settings (§14.3).
#[allow(
    clippy::struct_excessive_bools,
    reason = "the flags mirror the settings"
)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ActivitySettingsView {
    /// Whether the module is on.
    pub enabled: bool,
    /// Why Quack turned it off.
    pub disabled_reason: Option<ModuleDisabledReason>,
    /// Post message text.
    pub include_message_content: bool,
    /// Post file names (with their links).
    pub include_file_names: bool,
    /// Post link preview types.
    pub include_embed_types: bool,
    /// One channel per routed event.
    pub routes: BTreeMap<ActivityEvent, ChannelId>,
    /// Incremented by every change; an update names it.
    pub version: u64,
    /// Quack lacks View Audit Log, so bans and unbans are not posted
    /// (`module.problem.activity_needs_audit_log`); not an enable problem.
    pub needs_audit_log: bool,
}

/// A full replacement of the settings (`PUT …/modules/activity_log/settings`).
#[allow(
    clippy::struct_excessive_bools,
    reason = "the flags mirror the settings"
)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivitySettingsInput {
    /// The version the editor saw; a different one is `settings_changed`.
    pub expected_version: u64,
    /// Post message text.
    pub include_message_content: bool,
    /// Post file names.
    pub include_file_names: bool,
    /// Post link preview types.
    pub include_embed_types: bool,
    /// One staff-only channel per routed event; problems are reported
    /// under the event's name.
    pub routes: BTreeMap<ActivityEvent, ChannelId>,
}

/// The settings, for `manage_settings`.
pub async fn settings(
    engine: &Engine,
    principal: &Principal,
) -> Result<ActivitySettingsView, SettingsError> {
    span("activity_settings", principal.guild, async {
        let mut conn = engine.db().conn().await?;
        principal
            .require(&mut conn, Capability::ManageSettings)
            .await??;
        let s = load(&mut conn, principal.guild).await?;
        drop(conn);
        let needs_audit_log = match engine.discord().guild_facts(principal.guild).await {
            Outcome::Done(g) => g.quack.is_some_and(|q| {
                !q.is_administrator() && !q.permissions.contains(Permission::ViewAuditLog.into())
            }),
            _ => false,
        };
        Ok(view(s, needs_audit_log))
    })
    .await
}

async fn load(conn: &mut PgConnection, guild: GuildId) -> Result<ActivityLogSettings, DbError> {
    activity_log::settings(conn, guild, false)
        .await?
        .ok_or(DbError::NotFound)
}

fn view(s: ActivityLogSettings, needs_audit_log: bool) -> ActivitySettingsView {
    ActivitySettingsView {
        enabled: s.enabled,
        disabled_reason: s.disabled_reason,
        include_message_content: s.include_message_content,
        include_file_names: s.include_file_names,
        include_embed_types: s.include_embed_types,
        routes: s.routes.into_iter().collect(),
        version: s.version,
        needs_audit_log,
    }
}

/// Replaces the settings in one transaction (§13.1.2): every route channel
/// must be staff-only with View Channel and Send Messages for Quack; a
/// module that is on keeps at least one route.
pub async fn update_settings(
    engine: &Engine,
    principal: &Principal,
    input: &ActivitySettingsInput,
    op_key: &OpKey,
) -> Result<ActivitySettingsView, SettingsError> {
    let guild = principal.guild;
    span("update_activity_settings", guild, async {
        let mut conn = engine.db().conn().await?;
        principal
            .require(&mut conn, Capability::ManageSettings)
            .await??;
        drop(conn);
        let mut gathered: BTreeMap<ChannelId, Gathered> = BTreeMap::new();
        for channel in input.routes.values() {
            if !gathered.contains_key(channel) {
                let g = channels::gather(engine, guild, *channel).await;
                if matches!(g, Gathered::Unavailable) {
                    return Err(SettingsError::DiscordUnavailable);
                }
                gathered.insert(*channel, g);
            }
        }
        let key = op_key.scoped(Some(guild), Some(principal.account));
        let mut tx = match begin_keyed(engine.db(), &key, &request_hash(input)).await? {
            Keyed::Replay(_) => {
                let mut conn = engine.db().conn().await?;
                return Ok(view(load(&mut conn, guild).await?, false));
            }
            Keyed::Fresh(tx) => tx,
        };
        principal
            .require(&mut tx, Capability::ManageSettings)
            .await??;
        let current = activity_log::settings(&mut tx, guild, true)
            .await?
            .ok_or(DbError::NotFound)?;
        if current.version != input.expected_version {
            return Err(DomainError::SettingsChanged.into());
        }
        if current.enabled && input.routes.is_empty() {
            return Err(SettingsError::ModuleNotReady(vec![
                ModuleProblem::ActivityNoRoute,
            ]));
        }
        let roles = quack_db::settings::get(&mut tx, guild)
            .await?
            .map_or_default(|s| s.moderator_role_ids);
        let problems = validate(input, &gathered, &roles)?;
        if !problems.is_empty() {
            return Err(SettingsError::Invalid(problems));
        }
        write(&mut tx, principal, &current, input).await?;
        complete(
            &mut tx,
            &key,
            200,
            serde_json::json!({ "module": "activity_log" }),
        )
        .await?;
        let out = view(load(&mut tx, guild).await?, false);
        tx.commit().await?;
        engine.config_cache().reload(engine.db(), guild).await?;
        Ok(out)
    })
    .await
}

fn validate(
    input: &ActivitySettingsInput,
    gathered: &BTreeMap<ChannelId, Gathered>,
    roles: &[RoleId],
) -> Result<Vec<(&'static str, FieldProblem)>, SettingsError> {
    let mut problems = Vec::new();
    for (event, channel) in &input.routes {
        let Some(g) = gathered.get(channel) else {
            continue;
        };
        let p = match channels::check(g, roles, needs()) {
            ChannelCheck::Ok => continue,
            ChannelCheck::Problems(p) => p,
            ChannelCheck::Missing => vec![ChannelProblem::Missing],
            ChannelCheck::Unavailable => return Err(SettingsError::DiscordUnavailable),
        };
        problems.push((event.as_str(), FieldProblem::Channel(p)));
    }
    Ok(problems)
}

async fn write(
    tx: &mut Tx,
    principal: &Principal,
    current: &ActivityLogSettings,
    input: &ActivitySettingsInput,
) -> Result<(), SettingsError> {
    let guild = principal.guild;
    let routes: Vec<(ActivityEvent, ChannelId)> =
        input.routes.iter().map(|(e, c)| (*e, *c)).collect();
    let flags = ActivityFlags {
        include_message_content: input.include_message_content,
        include_file_names: input.include_file_names,
        include_embed_types: input.include_embed_types,
    };
    activity_log::update_settings(
        tx,
        guild,
        flags,
        &routes,
        input.expected_version,
        principal.account,
    )
    .await?
    .ok_or(DomainError::SettingsChanged)?;
    let fields: Vec<String> = [
        (
            "include_message_content",
            current.include_message_content != input.include_message_content,
        ),
        (
            "include_file_names",
            current.include_file_names != input.include_file_names,
        ),
        (
            "include_embed_types",
            current.include_embed_types != input.include_embed_types,
        ),
        ("routes", current.routes != routes),
    ]
    .into_iter()
    .filter(|(_, changed)| *changed)
    .map(|(name, _)| name.to_owned())
    .collect();
    modlog::write(
        tx,
        guild,
        ModLogEntry::ModuleSettingsChanged(ModuleSettingsChangedDetail {
            module: ModuleName::ActivityLog,
            fields,
        }),
        By {
            actor: Some(principal.account),
            source: principal.origin.mod_log_source(),
        },
        ModLogLinks::default(),
    )
    .await?;
    notify::settings_changed(tx, guild).await?;
    Ok(())
}

/// `/setup`'s channel select: routes all nine events to `channel`, keeping
/// the privacy flags.
pub async fn route_all(
    engine: &Engine,
    principal: &Principal,
    channel: ChannelId,
    op_key: &OpKey,
) -> Result<ActivitySettingsView, SettingsError> {
    let current = {
        let mut conn = engine.db().conn().await?;
        load(&mut conn, principal.guild).await?
    };
    let input = ActivitySettingsInput {
        expected_version: current.version,
        include_message_content: current.include_message_content,
        include_file_names: current.include_file_names,
        include_embed_types: current.include_embed_types,
        routes: ActivityEvent::ALL.iter().map(|e| (*e, channel)).collect(),
    };
    update_settings(engine, principal, &input, op_key).await
}

/// The module's enable and disable hooks (§13.1.7), for
/// `quack_engine::settings::set_enabled`.
pub fn hooks() -> ModuleHooks {
    ModuleHooks {
        name: ModuleName::ActivityLog,
        enable_problems,
        on_enabled_changed,
    }
}

fn enable_problems<'a>(
    tx: &'a mut Tx,
    guild: GuildId,
    facts: &'a ModuleFacts,
) -> BoxFuture<'a, Result<Vec<ModuleProblem>, DbError>> {
    Box::pin(async move {
        let s = activity_log::settings(tx, guild, true)
            .await?
            .ok_or(DbError::NotFound)?;
        if s.routes.is_empty() {
            return Ok(vec![ModuleProblem::ActivityNoRoute]);
        }
        let roles = quack_db::settings::get(tx, guild)
            .await?
            .map_or_default(|s| s.moderator_role_ids);
        let mut channels: Vec<ChannelId> = s.routes.iter().map(|(_, c)| *c).collect();
        channels.sort_unstable();
        channels.dedup();
        let mut out = Vec::new();
        for id in channels {
            let Some(channel) = facts.channels.iter().find(|c| c.id == id) else {
                out.push(ModuleProblem::ChannelPermissions {
                    channel: id,
                    missing: vec![Permission::ViewChannel, Permission::SendMessages],
                });
                continue;
            };
            let mut not_staff_only = false;
            for p in channels::check_from_facts(&facts.guild, channel, &roles, needs()) {
                match p {
                    ChannelProblem::QuackMissing(missing) => {
                        out.push(ModuleProblem::ChannelPermissions {
                            channel: id,
                            missing,
                        });
                    }
                    _ => not_staff_only = true,
                }
            }
            if not_staff_only {
                out.push(ModuleProblem::ChannelNotStaffOnly(id));
            }
        }
        Ok(out)
    })
}

/// Nothing durable goes with turning it on or off; each process empties
/// its cache and batches for the server lazily, on its next event.
fn on_enabled_changed(_: &mut Tx, _: GuildId, _: bool) -> BoxFuture<'_, Result<(), DbError>> {
    Box::pin(async { Ok(()) })
}
