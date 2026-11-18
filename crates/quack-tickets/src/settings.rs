//! Ticket settings (§13.2.1): reading, the one-transaction update, the
//! enable check, and the panel that follows the entry channel.

use quack_core::DomainError;
use quack_core::ids::{ChannelId, GuildId, MessageRowId};
use quack_core::messages::{MessageKind, MessageState};
use quack_core::modlog::{ModLogEntry, ModLogLinks, ModuleSettingsChangedDetail};
use quack_core::modules::{ModuleDisabledReason, ModuleName, ModuleProblem};
use quack_core::permissions::{
    Capability, ChannelFacts, ChannelProblem, Permission, Permissions, TEXT_CHANNEL,
};
use quack_core::settings::check_retention_days;
use quack_db::messages::{self, NewTrackedMessage, Owner};
use quack_db::tickets::{self, TicketSettings};
use quack_db::{DbError, PgConnection, Tx, notify};
use quack_engine::channels::{self, ChannelCheck, Gathered};
use quack_engine::modlog::{self, By};
use quack_engine::opkey::{Keyed, begin_keyed, complete, request_hash};
use quack_engine::port::{BoxFuture, Outcome};
use quack_engine::settings::{FieldProblem, ModuleFacts, ModuleHooks, SettingsError};
use quack_engine::{Engine, OpKey, Principal};
use serde::{Deserialize, Serialize};

use crate::obs::span;

/// What Quack needs in the entry channel to run tickets.
pub(crate) fn entry_needs() -> Permissions {
    Permission::ViewChannel
        | Permission::SendMessages
        | Permission::ReadMessageHistory
        | Permission::CreatePrivateThreads
        | Permission::SendMessagesInThreads
        | Permission::ManageThreads
}

/// What Quack needs in the ticket staff (queue) channel.
pub(crate) fn queue_needs() -> Permissions {
    Permission::ViewChannel | Permission::SendMessages | Permission::AttachFiles
}

/// The ticket settings (§14.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TicketSettingsView {
    /// Whether tickets are on.
    pub enabled: bool,
    /// Why Quack turned them off.
    pub disabled_reason: Option<ModuleDisabledReason>,
    /// Where members open tickets.
    pub entry_channel_id: Option<ChannelId>,
    /// Where staff see ticket posts.
    pub queue_channel_id: Option<ChannelId>,
    /// How long transcripts are kept, in days.
    pub transcript_retention_days: u16,
    /// Incremented by every change; an update names it.
    pub version: u64,
}

impl From<&TicketSettings> for TicketSettingsView {
    fn from(s: &TicketSettings) -> Self {
        Self {
            enabled: s.enabled,
            disabled_reason: s.disabled_reason,
            entry_channel_id: s.entry_channel,
            queue_channel_id: s.queue_channel,
            transcript_retention_days: s.transcript_retention_days,
            version: s.version,
        }
    }
}

/// A full replacement of the ticket settings (`PUT …/modules/tickets/settings`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketSettingsInput {
    /// The version the editor saw; a different one is `settings_changed`.
    pub expected_version: u64,
    /// Where members open tickets.
    pub entry_channel_id: Option<ChannelId>,
    /// Where staff see ticket posts.
    pub queue_channel_id: Option<ChannelId>,
    /// How long transcripts are kept, in days (1–365).
    pub transcript_retention_days: u16,
}

/// The ticket settings, for `manage_settings`.
pub async fn settings(
    engine: &Engine,
    principal: &Principal,
) -> Result<TicketSettingsView, SettingsError> {
    span("ticket_settings", principal.guild, async {
        let mut conn = engine.db().conn().await?;
        principal
            .require(&mut conn, Capability::ManageSettings)
            .await??;
        Ok(view(&mut conn, principal.guild).await?)
    })
    .await
}

async fn view(conn: &mut PgConnection, guild: GuildId) -> Result<TicketSettingsView, DbError> {
    let s = tickets::settings(conn, guild)
        .await?
        .ok_or(DbError::NotFound)?;
    Ok(TicketSettingsView::from(&s))
}

/// Replaces the ticket settings in one transaction (§13.1.2): facts first,
/// then the locked row, the version check, every field validated (even
/// while tickets are off), the write, the panel moved to a new entry
/// channel, the log entry, and the config cache notified.
pub async fn update_settings(
    engine: &Engine,
    principal: &Principal,
    input: &TicketSettingsInput,
    op_key: &OpKey,
) -> Result<TicketSettingsView, SettingsError> {
    let guild = principal.guild;
    span("update_ticket_settings", guild, async {
        let mut conn = engine.db().conn().await?;
        principal
            .require(&mut conn, Capability::ManageSettings)
            .await??;
        drop(conn);
        let entry = match input.entry_channel_id {
            Some(c) => Some(entry_facts(engine, guild, c).await?),
            None => None,
        };
        let queue = match input.queue_channel_id {
            Some(c) => match channels::gather(engine, guild, c).await {
                Gathered::Unavailable => return Err(SettingsError::DiscordUnavailable),
                g => Some(g),
            },
            None => None,
        };
        let key = op_key.scoped(Some(guild), Some(principal.account));
        let mut tx = match begin_keyed(engine.db(), &key, &request_hash(input)).await? {
            Keyed::Replay(_) => {
                let mut conn = engine.db().conn().await?;
                return Ok(view(&mut conn, guild).await?);
            }
            Keyed::Fresh(tx) => tx,
        };
        principal
            .require(&mut tx, Capability::ManageSettings)
            .await??;
        let current = tickets::lock_settings(&mut tx, guild)
            .await?
            .ok_or(DbError::NotFound)?;
        if current.version != input.expected_version {
            return Err(DomainError::SettingsChanged.into());
        }
        let roles = quack_db::settings::get(&mut tx, guild)
            .await?
            .map_or_default(|s| s.moderator_role_ids);
        let problems = validate(input, entry.as_ref(), queue.as_ref(), &roles)?;
        if !problems.is_empty() {
            return Err(SettingsError::Invalid(problems));
        }
        let saved = tickets::update_settings(
            &mut tx,
            guild,
            input.entry_channel_id,
            input.queue_channel_id,
            input.transcript_retention_days,
            input.expected_version,
            principal.account,
        )
        .await?;
        if saved.is_none() {
            return Err(DomainError::SettingsChanged.into());
        }
        if let Some(entry) = input.entry_channel_id
            && current.entry_channel != Some(entry)
        {
            place_panel(&mut tx, guild, entry).await?;
        }
        let fields = changed_fields(&current, input);
        modlog::write(
            &mut tx,
            guild,
            ModLogEntry::ModuleSettingsChanged(ModuleSettingsChangedDetail {
                module: ModuleName::Tickets,
                fields,
            }),
            By {
                actor: Some(principal.account),
                source: principal.origin.mod_log_source(),
            },
            ModLogLinks::default(),
        )
        .await?;
        notify::settings_changed(&mut tx, guild).await?;
        complete(
            &mut tx,
            &key,
            200,
            serde_json::json!({ "module": "tickets" }),
        )
        .await?;
        let out = view(&mut tx, guild).await?;
        tx.commit().await?;
        Ok(out)
    })
    .await
}

/// Every field's problems (§13.2.1); the entry and queue facts were
/// gathered before the transaction (`None` when the field is cleared).
fn validate(
    input: &TicketSettingsInput,
    entry: Option<&Option<ChannelFacts>>,
    queue: Option<&Gathered>,
    roles: &[quack_core::ids::RoleId],
) -> Result<Vec<(&'static str, FieldProblem)>, SettingsError> {
    let mut problems = Vec::new();
    if let Some(entry) = entry
        && let Some(p) = entry_problems(entry.as_ref())
    {
        problems.push(("entry_channel_id", FieldProblem::Channel(p)));
    }
    if let Some(queue) = queue {
        let p = match channels::check(queue, roles, queue_needs()) {
            ChannelCheck::Ok => None,
            ChannelCheck::Problems(p) => Some(p),
            ChannelCheck::Missing => Some(vec![ChannelProblem::Missing]),
            ChannelCheck::Unavailable => return Err(SettingsError::DiscordUnavailable),
        };
        if let Some(p) = p {
            problems.push(("queue_channel_id", FieldProblem::Channel(p)));
        }
    }
    if let Err(p) = check_retention_days(input.transcript_retention_days) {
        problems.push(("transcript_retention_days", FieldProblem::Value(p)));
    }
    Ok(problems)
}

fn changed_fields(current: &TicketSettings, input: &TicketSettingsInput) -> Vec<String> {
    [
        (
            "entry_channel_id",
            current.entry_channel != input.entry_channel_id,
        ),
        (
            "queue_channel_id",
            current.queue_channel != input.queue_channel_id,
        ),
        (
            "transcript_retention_days",
            current.transcript_retention_days != input.transcript_retention_days,
        ),
    ]
    .into_iter()
    .filter(|(_, changed)| *changed)
    .map(|(name, _)| name.to_owned())
    .collect()
}

/// The entry channel's facts: `None` when it no longer exists.
async fn entry_facts(
    engine: &Engine,
    guild: GuildId,
    channel: ChannelId,
) -> Result<Option<ChannelFacts>, SettingsError> {
    match engine.discord().channel_facts(guild, channel).await {
        Outcome::Done(facts) => Ok(facts),
        Outcome::Refused(_) => Ok(None),
        _ => Err(SettingsError::DiscordUnavailable),
    }
}

/// The entry channel must be a text channel of this server.
fn entry_problems(entry: Option<&ChannelFacts>) -> Option<Vec<ChannelProblem>> {
    match entry {
        None => Some(vec![ChannelProblem::Missing]),
        Some(c) if c.kind != TEXT_CHANNEL => Some(vec![ChannelProblem::NotText]),
        Some(_) => None,
    }
}

/// Puts the ticket panel in `entry`: posts it the first time, moves it
/// from an older entry channel (the old message becomes "moved"), or
/// brings back a panel that was gone.
pub(crate) async fn place_panel(
    tx: &mut Tx,
    guild: GuildId,
    entry: ChannelId,
) -> Result<(), DbError> {
    let owner = Owner::Server(MessageKind::TicketPanel);
    match messages::by_owner(tx, guild, owner).await? {
        None => {
            messages::insert(
                tx,
                &NewTrackedMessage {
                    id: MessageRowId::from_uuid(uuid::Uuid::now_v7()),
                    guild,
                    owner,
                    channel: Some(entry),
                    private: false,
                    interaction_token: None,
                },
            )
            .await
        }
        Some(row) if row.state == MessageState::Gone => {
            messages::reactivate(tx, guild, owner, entry).await?;
            Ok(())
        }
        Some(row) if row.channel == Some(entry) => {
            messages::bump(tx, guild, owner).await?;
            Ok(())
        }
        Some(row) => {
            messages::relocate(tx, guild, row.id, row.generation, Some(entry)).await?;
            Ok(())
        }
    }
}

/// The tickets module's enable and disable hooks (§13.1.7), for
/// `quack_engine::settings::set_enabled`.
pub fn hooks() -> ModuleHooks {
    ModuleHooks {
        name: ModuleName::Tickets,
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
        let s = tickets::lock_settings(tx, guild)
            .await?
            .ok_or(DbError::NotFound)?;
        let roles = quack_db::settings::get(tx, guild)
            .await?
            .map_or_default(|s| s.moderator_role_ids);
        Ok(problems(&s, facts, &roles))
    })
}

/// Why tickets can't be turned on (§13.2.1), from the locked settings and
/// the server's facts.
fn problems(
    s: &TicketSettings,
    facts: &ModuleFacts,
    roles: &[quack_core::ids::RoleId],
) -> Vec<ModuleProblem> {
    let find = |id: ChannelId| facts.channels.iter().find(|c| c.id == id);
    let mut out = Vec::new();
    let entry = s.entry_channel.and_then(find);
    let queue = s.queue_channel.and_then(find);
    match entry {
        None => out.push(ModuleProblem::TicketEntryMissing),
        Some(e) => {
            let missing = e.quack_permissions.missing(entry_needs());
            if !missing.is_empty() {
                out.push(ModuleProblem::ChannelPermissions {
                    channel: e.id,
                    missing,
                });
            }
        }
    }
    let Some(queue) = queue else {
        out.push(ModuleProblem::TicketQueueMissing);
        return out;
    };
    if entry.is_some_and(|e| e.id == queue.id) {
        out.push(ModuleProblem::TicketSameChannel);
        return out;
    }
    let mut not_staff_only = false;
    for p in channels::check_from_facts(&facts.guild, queue, roles, queue_needs()) {
        match p {
            ChannelProblem::QuackMissing(missing) => out.push(ModuleProblem::ChannelPermissions {
                channel: queue.id,
                missing,
            }),
            _ => not_staff_only = true,
        }
    }
    if not_staff_only {
        out.push(ModuleProblem::ChannelNotStaffOnly(queue.id));
    }
    out
}

fn on_enabled_changed(
    tx: &mut Tx,
    guild: GuildId,
    _enabled: bool,
) -> BoxFuture<'_, Result<(), DbError>> {
    Box::pin(async move {
        let entry = tickets::settings(tx, guild)
            .await?
            .and_then(|s| s.entry_channel);
        match entry {
            Some(entry) => place_panel(tx, guild, entry).await,
            None => Ok(()),
        }
    })
}
