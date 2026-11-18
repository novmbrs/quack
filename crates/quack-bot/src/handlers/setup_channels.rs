//! "Create #…" in `/setup` (§13.1.4): find a channel Quack just made,
//! else check Quack's permissions and create one with the overwrites of
//! its kind (staff-only destinations, the ticket entry channel, the
//! honeypot). Saving it is the caller's settings update.

use quack_core::ids::{ChannelId, GuildId, RoleId};
use quack_core::permissions::{
    ChannelFacts, GuildFacts, Overwrite, OverwriteTarget, Permission, Permissions,
};
use quack_engine::Engine;
use quack_engine::port::{ChannelSpec, Nonce, Outcome};
use quack_telemetry::labels::Reason;
use quack_text::ChannelRef;
use quack_text::setup::{
    ChannelKind, channel_name, channel_topic, create_failed_discord, create_failed_permission,
    create_unconfirmed, first_message,
};
use time::OffsetDateTime;

use crate::ack::Failure;
use crate::errors;

/// How recent a channel must be to count as one Quack just made.
const ADOPT_WINDOW_MS: u64 = 10 * 60 * 1_000;
/// Discord's epoch (2015-01-01) in Unix milliseconds.
const DISCORD_EPOCH_MS: u64 = 1_420_070_400_000;

/// A channel Quack made or found.
pub(crate) enum Made {
    /// Created now.
    Created(ChannelRef),
    /// Created a moment ago (a double click or a lost answer).
    Adopted(ChannelRef),
}

impl Made {
    /// The channel.
    pub(crate) const fn channel(&self) -> &ChannelRef {
        match self {
            Self::Created(c) | Self::Adopted(c) => c,
        }
    }
}

/// The overwrites a created channel gets (§13.1.4): Quack's member
/// overwrite, the @everyone overwrite, and what staff roles are allowed
/// (`None` for the channels members use).
struct Shape {
    quack: Permissions,
    everyone_allow: Permissions,
    everyone_deny: Permissions,
    staff: Option<Permissions>,
}

fn shape(kind: ChannelKind) -> Shape {
    let read = Permission::ViewChannel | Permission::SendMessages | Permission::ReadMessageHistory;
    match kind {
        ChannelKind::TicketEntry => Shape {
            quack: read
                | Permission::CreatePrivateThreads
                | Permission::ManageThreads
                | Permission::SendMessagesInThreads,
            everyone_allow: Permission::ViewChannel
                | Permission::ReadMessageHistory
                | Permission::SendMessagesInThreads,
            everyone_deny: Permission::SendMessages.into(),
            staff: None,
        },
        ChannelKind::Honeypot => Shape {
            quack: read | Permission::ManageMessages,
            everyone_allow: read,
            everyone_deny: Permissions::NONE,
            staff: None,
        },
        ChannelKind::Log
        | ChannelKind::Appeals
        | ChannelKind::Evidence
        | ChannelKind::TicketQueue
        | ChannelKind::ActivityLog => Shape {
            quack: read | Permission::AttachFiles | Permission::EmbedLinks,
            everyone_allow: Permissions::NONE,
            everyone_deny: Permission::ViewChannel.into(),
            staff: Some(read),
        },
    }
}

/// Whether members can see a channel of this kind (`setup.created_public`).
pub(crate) const fn is_public(kind: ChannelKind) -> bool {
    matches!(kind, ChannelKind::TicketEntry | ChannelKind::Honeypot)
}

/// Whether a snowflake was made in the last 10 minutes.
fn recent(id: u64, now: OffsetDateTime) -> bool {
    let made_ms = (id >> 22) + DISCORD_EPOCH_MS;
    let now_ms = u64::try_from(now.unix_timestamp_nanos() / 1_000_000).unwrap_or(0);
    now_ms.saturating_sub(made_ms) < ADOPT_WINDOW_MS
}

/// Step 1: a text channel with the catalog name, under 10 minutes old,
/// with a member overwrite for Quack.
fn just_made(
    channels: &[ChannelFacts],
    name: &str,
    quack: Option<quack_core::ids::MemberId>,
) -> Option<ChannelId> {
    let now = OffsetDateTime::now_utc();
    channels
        .iter()
        .find(|c| {
            c.kind == 0
                && c.name == name
                && recent(c.id.get(), now)
                && c.overwrites
                    .iter()
                    .any(|o| Some(o.target) == quack.map(OverwriteTarget::Member))
        })
        .map(|c| c.id)
}

/// The staff roles the channel opens to: configured moderator roles that
/// still exist, else every role with Administrator or Moderate Members.
fn staff_roles(facts: &GuildFacts, configured: &[RoleId]) -> Vec<RoleId> {
    let existing: Vec<RoleId> = configured
        .iter()
        .copied()
        .filter(|r| facts.role(*r).is_some())
        .collect();
    if !existing.is_empty() {
        return existing;
    }
    let staff = Permissions::from(Permission::Administrator);
    let moderate = Permissions::from(Permission::ModerateMembers);
    facts
        .roles
        .iter()
        .filter(|r| r.id != facts.guild_id.everyone_role())
        .filter(|r| r.permissions.contains(staff) || r.permissions.contains(moderate))
        .map(|r| r.id)
        .collect()
}

/// Creates (or adopts) a channel of `kind` with its overwrites.
pub(crate) async fn create(
    engine: &Engine,
    guild: GuildId,
    kind: ChannelKind,
    moderator_roles: &[RoleId],
) -> Result<Made, Failure> {
    let discord = engine.discord();
    let name = channel_name(kind);
    let Outcome::Done(facts) = discord.guild_facts(guild).await else {
        return Err(errors::discord_unavailable());
    };
    let quack = facts.quack.as_ref().map(|q| q.id);
    let Outcome::Done(channels) = discord.guild_channels(guild).await else {
        return Err(errors::discord_unavailable());
    };
    if let Some(id) = just_made(&channels, name, quack) {
        return Ok(Made::Adopted(ChannelRef::new(id, name)));
    }
    let shape = shape(kind);
    let held = facts
        .quack
        .as_ref()
        .map_or(Permissions::NONE, |q| q.permissions);
    // Discord lets Quack allow or deny only what it holds itself.
    let needed = shape.quack
        | shape.everyone_allow
        | shape.everyone_deny
        | shape.staff.unwrap_or(Permissions::NONE)
        | Permission::ManageChannels;
    let missing = if held.contains(Permission::Administrator.into()) {
        Vec::new()
    } else {
        held.missing(needed)
    };
    if !missing.is_empty() {
        return Err(Failure::refused(
            create_failed_permission(name, &missing),
            Reason::MissingPermission,
        ));
    }
    let Some(quack) = quack else {
        return Err(Failure::refused(
            quack_text::error::quack_not_in_server(),
            Reason::MissingPermission,
        ));
    };
    // The @everyone role's id is the server's id.
    let Some(everyone) = RoleId::new(guild.get()) else {
        return Err(Failure::failed(
            quack_text::error::internal(),
            Reason::Internal,
        ));
    };
    let mut overwrites = vec![
        Overwrite {
            target: OverwriteTarget::Role(everyone),
            allow: shape.everyone_allow,
            deny: shape.everyone_deny,
        },
        Overwrite {
            target: OverwriteTarget::Member(quack),
            allow: shape.quack,
            deny: Permissions::NONE,
        },
    ];
    if let Some(staff_allow) = shape.staff {
        overwrites.extend(
            staff_roles(&facts, moderator_roles)
                .into_iter()
                .map(|role| Overwrite {
                    target: OverwriteTarget::Role(role),
                    allow: staff_allow,
                    deny: Permissions::NONE,
                }),
        );
    }
    let spec = ChannelSpec {
        name: name.to_owned(),
        topic: Some(channel_topic(kind).to_owned()),
        overwrites,
    };
    match discord.create_channel(guild, &spec).await {
        Outcome::Done(id) => Ok(Made::Created(ChannelRef::new(id, name))),
        Outcome::Unknown(_) => Err(Failure::failed(create_unconfirmed(name), Reason::Timeout)),
        _ => Err(Failure::failed(
            create_failed_discord(name),
            Reason::Discord,
        )),
    }
}

/// Posts a new staff channel's intro message (§12.11 "First message"); a
/// failure undoes nothing.
pub(crate) async fn introduce(engine: &Engine, kind: ChannelKind, channel: ChannelId) {
    if let Some(msg) = first_message(kind) {
        let nonce = Nonce::new(format!("intro{}", channel.get()));
        let sent = engine.discord().send(channel, &msg, &[], &nonce).await;
        if !matches!(sent, Outcome::Done(_)) {
            tracing::info!(
                channel = channel.get(),
                "couldn't post a new channel's first message"
            );
        }
    }
}
