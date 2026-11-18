//! Server health (§14.2 route 7, §14.3 `Health`): what Quack is missing in
//! the server, and whether each configured staff channel still works.

use quack_core::ids::ChannelId;
use quack_core::permissions::{Capability, ChannelProblem, Permission, Permissions};
use quack_db::settings;
use serde::Serialize;

use super::{ModulesView, SettingsError, module_state};
use crate::channels::{ChannelCheck, staff_channel};
use crate::port::Outcome;
use crate::{Engine, Principal, obs};
use quack_core::modules::ModuleName;

/// What a server is missing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Health {
    /// Quack is in the server.
    pub quack_in_server: bool,
    /// Server permissions Quack needs and lacks.
    pub missing_permissions: Vec<Permission>,
    /// Each configured staff channel and its problems.
    pub channels: Vec<ChannelHealth>,
    /// Each module's state.
    pub modules: ModulesView,
}

/// One configured channel's problems.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ChannelHealth {
    /// The setting's API name.
    pub setting: &'static str,
    /// The channel.
    pub channel_id: ChannelId,
    /// Why it is not a usable staff destination (empty: it is), as
    /// machine codes.
    pub problems: Vec<&'static str>,
    /// The same problems, typed, for messages.
    #[serde(skip)]
    pub details: Vec<ChannelProblem>,
}

/// The permissions Quack's install asks for (§20.3).
fn wanted() -> Permissions {
    [
        Permission::ViewChannel,
        Permission::SendMessages,
        Permission::SendMessagesInThreads,
        Permission::EmbedLinks,
        Permission::AttachFiles,
        Permission::ReadMessageHistory,
        Permission::ModerateMembers,
        Permission::KickMembers,
        Permission::BanMembers,
        Permission::ManageMessages,
        Permission::ManageChannels,
        Permission::ManageThreads,
        Permission::CreatePrivateThreads,
        Permission::ViewAuditLog,
    ]
    .into_iter()
    .collect()
}

/// The server's health, for `manage_settings`.
pub async fn health(engine: &Engine, principal: &Principal) -> Result<Health, SettingsError> {
    let guild = principal.guild;
    obs::use_case("server_health", Some(guild), async {
        let (configured, modules) = {
            let mut conn = engine.db().conn().await?;
            principal
                .require(&mut conn, Capability::ManageSettings)
                .await??;
            let s = settings::get(&mut conn, guild).await?;
            let configured = s.map_or_default(|s| {
                vec![
                    (
                        "log_channel_id",
                        s.log_channel,
                        Permission::ViewChannel | Permission::SendMessages,
                    ),
                    (
                        "appeal_channel_id",
                        s.appeal_channel,
                        Permission::ViewChannel | Permission::SendMessages,
                    ),
                    (
                        "evidence_channel_id",
                        s.evidence_channel,
                        Permission::ViewChannel
                            | Permission::SendMessages
                            | Permission::AttachFiles,
                    ),
                ]
            });
            let modules = ModulesView {
                tickets: module_state(&mut conn, guild, ModuleName::Tickets).await?,
                activity_log: module_state(&mut conn, guild, ModuleName::ActivityLog).await?,
                honeypot: module_state(&mut conn, guild, ModuleName::Honeypot).await?,
            };
            (configured, modules)
        };
        let quack = match engine.discord().guild_facts(guild).await {
            Outcome::Done(facts) => facts.quack,
            Outcome::Refused(_) => None,
            _ => return Err(SettingsError::DiscordUnavailable),
        };
        let mut channels = Vec::new();
        for (setting, channel, needs) in configured {
            let Some(channel) = channel else { continue };
            let problems = match staff_channel(engine, guild, channel, needs).await {
                ChannelCheck::Ok => Vec::new(),
                ChannelCheck::Problems(p) => p,
                ChannelCheck::Missing => vec![ChannelProblem::Missing],
                ChannelCheck::Unavailable => return Err(SettingsError::DiscordUnavailable),
            };
            channels.push(ChannelHealth {
                setting,
                channel_id: channel,
                problems: problems.iter().map(ChannelProblem::code).collect(),
                details: problems,
            });
        }
        Ok(Health {
            quack_in_server: quack.is_some(),
            missing_permissions: quack.map_or_else(
                || Permission::ALL.to_vec(),
                |q| q.permissions.missing(wanted()),
            ),
            channels,
            modules,
        })
    })
    .await
}
