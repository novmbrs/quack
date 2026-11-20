//! Server settings (§13.1): reading them, the one-transaction patch with
//! per-field validation, acknowledging the starter notice, turning modules
//! on and off, and server health.

mod health;
mod modules;
mod patch;

use quack_core::DomainError;
use quack_core::ids::{ChannelId, RoleId, RuleId};
use quack_core::modules::{ModuleDisabledReason, ModuleName, ModuleProblem};
use quack_core::permissions::{Capability, ChannelProblem};
use quack_core::settings::SettingsProblem;
use quack_db::{DbError, PgConnection, activity_log, honeypot, settings, tickets};
use serde::Serialize;
use time::OffsetDateTime;

use crate::opkey::KeyError;
use crate::{Engine, Principal, obs};

pub use health::{ChannelHealth, Health, health, wanted};
pub use modules::{
    EnableCheck, EnabledChanged, ModuleFacts, ModuleHooks, begin_module_write, finish_module_write,
    set_enabled,
};
pub use patch::{SettingsPatch, acknowledge_starter_notice, patch};

/// Why a settings change was refused.
#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    /// A refusal with a `quack-text` message (`settings_changed`,
    /// capability, 2FA).
    #[error("refused: {0:?}")]
    Refused(DomainError),
    /// Fields have problems, by API field name; nothing was saved.
    #[error("invalid settings")]
    Invalid(Vec<(&'static str, FieldProblem)>),
    /// A module can't be turned on yet (`409 module_not_ready`).
    #[error("module not ready")]
    ModuleNotReady(Vec<ModuleProblem>),
    /// The operation key was used for a different request.
    #[error("operation key reused")]
    OperationKeyReused,
    /// Discord didn't answer the facts the checks need.
    #[error("Discord unavailable")]
    DiscordUnavailable,
    /// The database failed.
    #[error(transparent)]
    Db(#[from] DbError),
}

impl From<DomainError> for SettingsError {
    fn from(error: DomainError) -> Self {
        Self::Refused(error)
    }
}

impl From<KeyError> for SettingsError {
    fn from(error: KeyError) -> Self {
        match error {
            KeyError::Reused => Self::OperationKeyReused,
            KeyError::Db(db) => Self::Db(db),
        }
    }
}

/// What is wrong with one field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldProblem {
    /// The value itself (§13.1.1 validation).
    Value(SettingsProblem),
    /// The channel is not a usable staff destination (§13.1.3).
    Channel(Vec<ChannelProblem>),
}

/// The server's settings (§14.3 `Settings`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SettingsView {
    /// Incremented by every change; role list changes name it.
    pub version: u64,
    /// Roles that grant `moderate`.
    pub moderator_role_ids: Vec<RoleId>,
    /// Roles that grant `manage_rules`.
    pub rules_manager_role_ids: Vec<RoleId>,
    /// The log channel.
    pub log_channel_id: Option<ChannelId>,
    /// The appeal channel.
    pub appeal_channel_id: Option<ChannelId>,
    /// The evidence channel.
    pub evidence_channel_id: Option<ChannelId>,
    /// The rejoin invite.
    pub appeal_invite_url: Option<String>,
    /// The DM introduction.
    pub dm_intro: Option<String>,
    /// The DM footer.
    pub dm_footer: Option<String>,
    /// The starter rule.
    pub starter_rule_id: Option<RuleId>,
    /// When the starter notice was acknowledged.
    #[serde(with = "time::serde::rfc3339::option")]
    pub starter_notice_acknowledged_at: Option<OffsetDateTime>,
    /// Each module's state.
    pub modules: ModulesView,
}

/// Each module's state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ModulesView {
    /// Tickets.
    pub tickets: ModuleState,
    /// The server activity log.
    pub activity_log: ModuleState,
    /// The honeypot.
    pub honeypot: ModuleState,
}

/// Whether a module is on, and why Quack turned it off.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ModuleState {
    /// It is on.
    pub enabled: bool,
    /// Why Quack turned it off.
    pub disabled_reason: Option<ModuleDisabledReason>,
}

/// The server's settings, for `manage_settings`.
pub async fn get(engine: &Engine, principal: &Principal) -> Result<SettingsView, SettingsError> {
    obs::use_case("get_settings", Some(principal.guild), async {
        let mut conn = engine.db().conn().await?;
        principal
            .require(&mut conn, Capability::ManageSettings)
            .await??;
        view(&mut conn, principal.guild).await
    })
    .await
}

pub(crate) async fn view(
    conn: &mut PgConnection,
    guild: quack_core::ids::GuildId,
) -> Result<SettingsView, SettingsError> {
    let s = settings::get(conn, guild).await?.ok_or(DbError::NotFound)?;
    Ok(SettingsView {
        version: s.version,
        moderator_role_ids: s.moderator_role_ids,
        rules_manager_role_ids: s.rules_manager_role_ids,
        log_channel_id: s.log_channel,
        appeal_channel_id: s.appeal_channel,
        evidence_channel_id: s.evidence_channel,
        appeal_invite_url: s.appeal_invite_url,
        dm_intro: s.dm_intro,
        dm_footer: s.dm_footer,
        starter_rule_id: s.starter_rule,
        starter_notice_acknowledged_at: s.starter_notice_acknowledged_at,
        modules: ModulesView {
            tickets: module_state(conn, guild, ModuleName::Tickets).await?,
            activity_log: module_state(conn, guild, ModuleName::ActivityLog).await?,
            honeypot: module_state(conn, guild, ModuleName::Honeypot).await?,
        },
    })
}

pub(crate) async fn module_state(
    conn: &mut PgConnection,
    guild: quack_core::ids::GuildId,
    module: ModuleName,
) -> Result<ModuleState, DbError> {
    Ok(match module {
        ModuleName::Tickets => {
            tickets::settings(conn, guild)
                .await?
                .map_or_default(|s| ModuleState {
                    enabled: s.enabled,
                    disabled_reason: s.disabled_reason,
                })
        }
        ModuleName::ActivityLog => activity_log::settings(conn, guild, false)
            .await?
            .map_or_default(|s| ModuleState {
                enabled: s.enabled,
                disabled_reason: s.disabled_reason,
            }),
        ModuleName::Honeypot => {
            honeypot::settings(conn, guild)
                .await?
                .map_or_default(|s| ModuleState {
                    enabled: s.enabled,
                    disabled_reason: s.disabled_reason,
                })
        }
    })
}
