//! Server health (§14.2 route 7, §14.3 `Health`): what Quack is missing in
//! the server, and whether each configured staff channel still works.

use quack_core::ids::ChannelId;
use quack_core::permissions::{Capability, ChannelProblem, Permission, Permissions};
use quack_db::settings;
use serde::Serialize;

use super::{ModulesView, SettingsError, module_state};
use crate::channels::{ChannelCheck, Gathered, gather, staff_channel};
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
    /// The setting's API name: a core setting (`log_channel_id`) or a
    /// module's (`tickets.queue_channel_id`, `tickets.entry_channel_id`,
    /// `activity_log.routes`, `honeypot.channel_id`).
    pub setting: String,
    /// Whether the channel must be staff-only (the ticket entry and
    /// honeypot channels are for members, so only Quack's access counts).
    pub staff_only: bool,
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

/// A module channel to check: its setting, the channel, what Quack needs
/// there, and whether it must be staff-only.
type ModuleChannel = (String, ChannelId, Permissions, bool);

/// The channels of the modules that are on (§13.2–13.4).
async fn module_channels(
    conn: &mut quack_db::PgConnection,
    guild: quack_core::ids::GuildId,
    modules: &ModulesView,
) -> Result<Vec<ModuleChannel>, quack_db::DbError> {
    let post = Permission::ViewChannel | Permission::SendMessages;
    let mut out = Vec::new();
    if modules.tickets.enabled
        && let Some(t) = quack_db::tickets::settings(conn, guild).await?
    {
        if let Some(queue) = t.queue_channel {
            out.push(("tickets.queue_channel_id".to_owned(), queue, post, true));
        }
        if let Some(entry) = t.entry_channel {
            let needs = post
                | Permission::CreatePrivateThreads
                | Permission::SendMessagesInThreads
                | Permission::ManageThreads;
            out.push(("tickets.entry_channel_id".to_owned(), entry, needs, false));
        }
    }
    if modules.activity_log.enabled
        && let Some(a) = quack_db::activity_log::settings(conn, guild, false).await?
    {
        let mut seen = Vec::new();
        for (_, channel) in a.routes {
            if !seen.contains(&channel) {
                seen.push(channel);
                out.push(("activity_log.routes".to_owned(), channel, post, true));
            }
        }
    }
    if modules.honeypot.enabled
        && let Some(channel) = quack_db::honeypot::settings(conn, guild)
            .await?
            .and_then(|h| h.channel)
    {
        let needs = post | Permission::ManageMessages;
        out.push(("honeypot.channel_id".to_owned(), channel, needs, false));
    }
    Ok(out)
}

/// One channel's health: the staff-only check (§13.1.3), or for a member
/// channel only that it exists, is text, and Quack holds `needs` there.
async fn checked(
    engine: &Engine,
    guild: quack_core::ids::GuildId,
    setting: String,
    channel: ChannelId,
    needs: Permissions,
    staff_only: bool,
) -> Result<ChannelHealth, SettingsError> {
    let problems = if staff_only {
        match staff_channel(engine, guild, channel, needs).await {
            ChannelCheck::Ok => Vec::new(),
            ChannelCheck::Problems(p) => p,
            ChannelCheck::Missing => vec![ChannelProblem::Missing],
            ChannelCheck::Unavailable => return Err(SettingsError::DiscordUnavailable),
        }
    } else {
        match gather(engine, guild, channel).await {
            Gathered::Facts(f) => {
                let mut p = Vec::new();
                if !f.is_text {
                    p.push(ChannelProblem::NotText);
                }
                let missing = f.quack_permissions.missing(needs);
                if !missing.is_empty() {
                    p.push(ChannelProblem::QuackMissing(missing));
                }
                p
            }
            Gathered::Missing => vec![ChannelProblem::Missing],
            Gathered::Inaccessible => vec![ChannelProblem::Inaccessible],
            Gathered::Unavailable => return Err(SettingsError::DiscordUnavailable),
        }
    };
    Ok(ChannelHealth {
        setting,
        staff_only,
        channel_id: channel,
        problems: problems.iter().map(ChannelProblem::code).collect(),
        details: problems,
    })
}

/// The server's health, for `manage_settings`.
pub async fn health(engine: &Engine, principal: &Principal) -> Result<Health, SettingsError> {
    let guild = principal.guild;
    obs::use_case("server_health", Some(guild), async {
        let (configured, modules, module_channels) = {
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
            let module_channels = module_channels(&mut conn, guild, &modules).await?;
            (configured, modules, module_channels)
        };
        let quack = match engine.discord().guild_facts(guild).await {
            Outcome::Done(facts) => facts.quack,
            Outcome::Refused(_) => None,
            _ => return Err(SettingsError::DiscordUnavailable),
        };
        let mut channels = Vec::new();
        for (setting, channel, needs) in configured {
            let Some(channel) = channel else { continue };
            channels.push(checked(engine, guild, setting.to_owned(), channel, needs, true).await?);
        }
        for (setting, channel, needs, staff_only) in module_channels {
            channels.push(checked(engine, guild, setting, channel, needs, staff_only).await?);
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
