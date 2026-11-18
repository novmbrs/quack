//! Honeypot settings (§13.4.1): reading, the one-transaction update, the
//! enable check, the default rule, and the warning that follows the
//! channel.

use quack_core::DomainError;
use quack_core::ids::{ChannelId, GuildId, MessageRowId, RuleId};
use quack_core::limits::{HONEYPOT_WARNING, MAX_ACTIVE_RULES};
use quack_core::messages::{MessageKind, MessageState};
use quack_core::modlog::{
    ModLogEntry, ModLogLinks, ModuleSettingsChangedDetail, RuleVersionDetail,
};
use quack_core::modules::{ModuleDisabledReason, ModuleName, ModuleProblem};
use quack_core::permissions::{
    Capability, ChannelFacts, ChannelProblem, Permission, Permissions, TEXT_CHANNEL,
};
use quack_core::rule::honeypot_rule;
use quack_core::settings::normalize_optional_text;
use quack_db::honeypot::{self, HoneypotSettings};
use quack_db::messages::{self, NewTrackedMessage, Owner};
use quack_db::{Conflict, DbError, PgConnection, Tx, notify, rules};
use quack_engine::modlog::{self, By};
use quack_engine::opkey::request_hash;
use quack_engine::port::{BoxFuture, Outcome};
use quack_engine::settings::{
    FieldProblem, ModuleFacts, ModuleHooks, SettingsError, begin_module_write, finish_module_write,
};
use quack_engine::{Engine, OpKey, Principal};
use serde::{Deserialize, Serialize};

use crate::obs::span;

/// What Quack needs in the honeypot channel.
fn channel_needs() -> Permissions {
    Permission::ViewChannel
        | Permission::SendMessages
        | Permission::ReadMessageHistory
        | Permission::ManageMessages
}

/// The honeypot settings (§14.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HoneypotSettingsView {
    /// Whether the honeypot is on.
    pub enabled: bool,
    /// Why Quack turned it off.
    pub disabled_reason: Option<ModuleDisabledReason>,
    /// The trap channel.
    pub channel_id: Option<ChannelId>,
    /// The rule its cases use.
    pub rule_id: Option<RuleId>,
    /// The server's own warning; `None` uses the generated one.
    pub warning_text: Option<String>,
    /// Incremented by every change; an update names it.
    pub version: u64,
    /// Hits waiting for their case.
    pub pending_hits: u64,
    /// Hits that didn't become a case.
    pub failed_hits: u64,
}

/// A full replacement of the honeypot settings
/// (`PUT …/modules/honeypot/settings`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HoneypotSettingsInput {
    /// The version the editor saw; a different one is `settings_changed`.
    pub expected_version: u64,
    /// The trap channel.
    pub channel_id: Option<ChannelId>,
    /// The rule (active, no required fields). Cleared only while off.
    pub rule_id: Option<RuleId>,
    /// The custom warning (0–1,500 characters; empty uses the generated one).
    pub warning_text: Option<String>,
}

/// The honeypot settings, for `manage_settings`.
pub async fn settings(
    engine: &Engine,
    principal: &Principal,
) -> Result<HoneypotSettingsView, SettingsError> {
    span("honeypot_settings", principal.guild, async {
        let mut conn = engine.db().conn().await?;
        principal
            .require(&mut conn, Capability::ManageSettings)
            .await??;
        Ok(view(&mut conn, principal.guild).await?)
    })
    .await
}

async fn view(conn: &mut PgConnection, guild: GuildId) -> Result<HoneypotSettingsView, DbError> {
    let s = honeypot::settings(conn, guild)
        .await?
        .ok_or(DbError::NotFound)?;
    let (pending_hits, failed_hits) = honeypot::hit_counts(conn, guild).await?;
    Ok(HoneypotSettingsView {
        enabled: s.enabled,
        disabled_reason: s.disabled_reason,
        channel_id: s.channel,
        rule_id: s.rule,
        warning_text: s.warning_text,
        version: s.version,
        pending_hits,
        failed_hits,
    })
}

/// Replaces the honeypot settings in one transaction (§13.1.2): facts
/// first, then the locked row (and the rule `FOR SHARE`), the version
/// check, every field validated, the write, the warning moved or
/// refreshed, the log entry, and the config cache notified. A change bumps
/// the settings version, so hits admitted before it fail `policy_changed`.
pub async fn update_settings(
    engine: &Engine,
    principal: &Principal,
    input: &HoneypotSettingsInput,
    op_key: &OpKey,
) -> Result<HoneypotSettingsView, SettingsError> {
    let guild = principal.guild;
    span("update_honeypot_settings", guild, async {
        let mut conn = engine.db().conn().await?;
        principal
            .require(&mut conn, Capability::ManageSettings)
            .await??;
        drop(conn);
        let channel = match input.channel_id {
            Some(c) => match engine.discord().channel_facts(guild, c).await {
                Outcome::Done(facts) => Some(facts),
                Outcome::Refused(_) => Some(None),
                _ => return Err(SettingsError::DiscordUnavailable),
            },
            None => None,
        };
        let Some((mut tx, key)) =
            begin_module_write(engine, principal, op_key, &request_hash(input)).await?
        else {
            let mut conn = engine.db().conn().await?;
            return Ok(view(&mut conn, guild).await?);
        };
        let current = honeypot::lock_settings(&mut tx, guild, true)
            .await?
            .ok_or(DbError::NotFound)?;
        if current.version != input.expected_version {
            return Err(DomainError::SettingsChanged.into());
        }
        let warning = validate(&mut tx, guild, &current, input, channel.as_ref()).await?;
        honeypot::update_settings(
            &mut tx,
            guild,
            input.channel_id,
            input.rule_id,
            warning.as_deref(),
            input.expected_version,
            principal.account,
        )
        .await?
        .ok_or(DomainError::SettingsChanged)?;
        place_warning(&mut tx, guild, input.channel_id).await?;
        let fields = changed_fields(&current, input, warning.as_deref());
        finish_module_write(&mut tx, principal, &key, ModuleName::Honeypot, fields).await?;
        let out = view(&mut tx, guild).await?;
        tx.commit().await?;
        engine.config_cache().reload(engine.db(), guild).await?;
        Ok(out)
    })
    .await
}

/// Validates every field against the locked row; returns the normalized
/// warning text.
async fn validate(
    tx: &mut Tx,
    guild: GuildId,
    current: &HoneypotSettings,
    input: &HoneypotSettingsInput,
    channel: Option<&Option<ChannelFacts>>,
) -> Result<Option<String>, SettingsError> {
    if current.enabled {
        let mut missing = Vec::new();
        if input.channel_id.is_none() {
            missing.push(ModuleProblem::HoneypotChannelMissing);
        }
        if input.rule_id.is_none() {
            missing.push(ModuleProblem::HoneypotRuleMissing);
        }
        if !missing.is_empty() {
            return Err(SettingsError::ModuleNotReady(missing));
        }
    }
    if let Some(rule) = input.rule_id {
        check_rule(tx, guild, rule).await?;
    }
    let mut problems = Vec::new();
    if let Some(channel) = channel
        && let Some(p) = channel_problems(channel.as_ref())
    {
        problems.push(("channel_id", FieldProblem::Channel(p)));
    }
    let warning = match normalize_optional_text(
        input.warning_text.as_deref().unwrap_or_default(),
        HONEYPOT_WARNING,
    ) {
        Ok(w) => w,
        Err(p) => {
            problems.push(("warning_text", FieldProblem::Value(p)));
            None
        }
    };
    if problems.is_empty() {
        Ok(warning)
    } else {
        Err(SettingsError::Invalid(problems))
    }
}

/// The rule must be an active rule of this server with no required fields;
/// it is held `FOR SHARE` until the commit, so it can't be archived or
/// gain a required field meanwhile.
async fn check_rule(tx: &mut Tx, guild: GuildId, rule: RuleId) -> Result<(), SettingsError> {
    let lock = rules::lock_for_share(tx, guild, rule)
        .await?
        .ok_or(DomainError::RuleNotFound)?;
    if lock.archived {
        return Err(DomainError::RuleArchived.into());
    }
    let def = rules::get(tx, guild, rule)
        .await?
        .ok_or(DomainError::RuleNotFound)?
        .definition;
    def.check_honeypot_compatible()?;
    Ok(())
}

/// A text channel of this server where Quack can read, post, and delete.
fn channel_problems(channel: Option<&ChannelFacts>) -> Option<Vec<ChannelProblem>> {
    match channel {
        None => Some(vec![ChannelProblem::Missing]),
        Some(c) if c.kind != TEXT_CHANNEL => Some(vec![ChannelProblem::NotText]),
        Some(c) => {
            let missing = c.quack_permissions.missing(channel_needs());
            (!missing.is_empty()).then(|| vec![ChannelProblem::QuackMissing(missing)])
        }
    }
}

fn changed_fields(
    current: &HoneypotSettings,
    input: &HoneypotSettingsInput,
    warning: Option<&str>,
) -> Vec<String> {
    [
        ("channel_id", current.channel != input.channel_id),
        ("rule_id", current.rule != input.rule_id),
        ("warning_text", current.warning_text.as_deref() != warning),
    ]
    .into_iter()
    .filter(|(_, changed)| *changed)
    .map(|(name, _)| name.to_owned())
    .collect()
}

/// Puts the warning in `channel` (first post, a move, or back from
/// `gone`) or refreshes it where it is.
async fn place_warning(
    tx: &mut Tx,
    guild: GuildId,
    channel: Option<ChannelId>,
) -> Result<(), DbError> {
    let owner = Owner::Server(MessageKind::HoneypotWarning);
    let row = messages::by_owner(tx, guild, owner).await?;
    let Some(channel) = channel else {
        messages::bump(tx, guild, owner).await?;
        return Ok(());
    };
    match row {
        None => {
            messages::insert(
                tx,
                &NewTrackedMessage {
                    id: MessageRowId::from_uuid(uuid::Uuid::now_v7()),
                    guild,
                    owner,
                    channel: Some(channel),
                    private: false,
                    interaction_token: None,
                },
            )
            .await
        }
        Some(row) if row.state == MessageState::Gone => {
            messages::reactivate(tx, guild, owner, channel).await?;
            Ok(())
        }
        Some(row) if row.channel == Some(channel) => {
            messages::bump(tx, guild, owner).await?;
            Ok(())
        }
        Some(row) => {
            messages::relocate(tx, guild, row.id, row.generation, Some(channel)).await?;
            Ok(())
        }
    }
}

/// "Turn on" from `/setup` with no rule (§3.8): creates the "Honeypot"
/// rule and saves it as the honeypot's rule in one transaction. A server
/// that already has an active rule named "Honeypot" is refused
/// (`honeypot_rule_missing`) so the administrator picks one; Quack never
/// adopts a rule by its name. Does nothing when a rule is set. The caller
/// then turns the module on with `set_enabled`.
pub async fn use_default_rule(
    engine: &Engine,
    principal: &Principal,
) -> Result<HoneypotSettingsView, SettingsError> {
    let guild = principal.guild;
    span("honeypot_default_rule", guild, async {
        let mut tx = engine.db().begin().await?;
        principal
            .require(&mut tx, Capability::ManageSettings)
            .await??;
        let current = honeypot::lock_settings(&mut tx, guild, true)
            .await?
            .ok_or(DbError::NotFound)?;
        if current.rule.is_some() {
            tx.rollback().await?;
            let mut conn = engine.db().conn().await?;
            return Ok(view(&mut conn, guild).await?);
        }
        quack_db::settings::lock(&mut tx, guild).await?;
        if rules::count_active(&mut tx, guild).await? >= MAX_ACTIVE_RULES {
            return Err(DomainError::TooManyRules.into());
        }
        let id = RuleId::from_uuid(uuid::Uuid::now_v7());
        let definition = honeypot_rule();
        let name = definition.name.clone();
        let created = rules::create(
            &mut tx,
            &rules::NewRule {
                id,
                guild,
                definition,
                created_by: Some(principal.account),
                reconstructed: false,
            },
        )
        .await;
        match created {
            Ok(()) => {}
            Err(e) if e.conflict() == Some(Conflict::RuleNameTaken) => {
                return Err(SettingsError::ModuleNotReady(vec![
                    ModuleProblem::HoneypotRuleMissing,
                ]));
            }
            Err(e) => return Err(e.into()),
        }
        honeypot::update_settings(
            &mut tx,
            guild,
            current.channel,
            Some(id),
            current.warning_text.as_deref(),
            current.version,
            principal.account,
        )
        .await?
        .ok_or(DomainError::SettingsChanged)?;
        let by = By {
            actor: Some(principal.account),
            source: principal.origin.mod_log_source(),
        };
        let links = ModLogLinks {
            rule_id: Some(id),
            ..ModLogLinks::default()
        };
        let entry = ModLogEntry::RuleCreated(RuleVersionDetail { name, version: 1 });
        modlog::write(&mut tx, guild, entry, by, links).await?;
        let entry = ModLogEntry::ModuleSettingsChanged(ModuleSettingsChangedDetail {
            module: ModuleName::Honeypot,
            fields: vec!["rule_id".to_owned()],
        });
        modlog::write(&mut tx, guild, entry, by, ModLogLinks::default()).await?;
        messages::bump(&mut tx, guild, Owner::Server(MessageKind::HoneypotWarning)).await?;
        notify::rules_changed(&mut tx, guild).await?;
        notify::settings_changed(&mut tx, guild).await?;
        let out = view(&mut tx, guild).await?;
        tx.commit().await?;
        engine.rules_cache().reload(engine.db(), guild).await?;
        engine.config_cache().reload(engine.db(), guild).await?;
        Ok(out)
    })
    .await
}

/// The honeypot's enable and disable hooks (§13.1.7), for
/// `quack_engine::settings::set_enabled`.
pub fn hooks() -> ModuleHooks {
    ModuleHooks {
        name: ModuleName::Honeypot,
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
        let s = honeypot::lock_settings(tx, guild, true)
            .await?
            .ok_or(DbError::NotFound)?;
        let mut out = Vec::new();
        match s
            .channel
            .and_then(|c| facts.channels.iter().find(|f| f.id == c))
        {
            None => out.push(ModuleProblem::HoneypotChannelMissing),
            Some(c) => {
                let missing = c.quack_permissions.missing(channel_needs());
                if !missing.is_empty() {
                    out.push(ModuleProblem::ChannelPermissions {
                        channel: c.id,
                        missing,
                    });
                }
            }
        }
        let Some(rule) = s.rule else {
            out.push(ModuleProblem::HoneypotRuleMissing);
            return Ok(out);
        };
        let usable = rules::lock_for_share(tx, guild, rule)
            .await?
            .is_some_and(|l| !l.archived);
        match rules::get(tx, guild, rule).await? {
            Some(r) if usable => {
                if r.definition.check_honeypot_compatible().is_err() {
                    out.push(ModuleProblem::HoneypotRuleRequiredFields(rule));
                }
            }
            _ => out.push(ModuleProblem::HoneypotRuleMissing),
        }
        Ok(out)
    })
}

/// Turning it on brings back a `gone` warning; either way the warning is
/// refreshed (it shows "off" while the honeypot is off).
fn on_enabled_changed(
    tx: &mut Tx,
    guild: GuildId,
    _enabled: bool,
) -> BoxFuture<'_, Result<(), DbError>> {
    Box::pin(async move {
        let channel = honeypot::settings(tx, guild).await?.and_then(|s| s.channel);
        place_warning(tx, guild, channel).await
    })
}
