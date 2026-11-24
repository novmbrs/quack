//! The gateway events Quack handles (§11.9), as plain typed values.
//!
//! Role events only update the guild cache and are not dispatched; every
//! other event below reaches the handler after the caches were updated.

use quack_core::ids::{AccountId, ChannelId, GuildId, MessageId};
use quack_core::permissions::AccountFacts;
use twilight_model::gateway::event::Event as Raw;
use twilight_model::gateway::payload::incoming::GuildCreate;
use twilight_model::guild::audit_log::AuditLogEventType;

use crate::cache::member_update;
use crate::convert;
use crate::interaction::{self, Interaction};
use crate::model::{Channel, Guild, Member, MemberCount, Message};

/// A ban or unban someone placed or lifted (from the audit log, §11.9).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BanChange {
    /// Audit log entry 22.
    Banned,
    /// Audit log entry 23.
    Unbanned,
}

/// One handled gateway event.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A new session. `missed_events` is true for every READY after the
    /// first: a new session means events may have been missed.
    Ready {
        /// Quack's own account.
        quack_id: AccountId,
        /// The servers in READY's list.
        guild_ids: Vec<GuildId>,
        /// Whether events may have been missed since the last session.
        missed_events: bool,
    },
    /// The session resumed; nothing was missed.
    Resumed,
    /// A server became available (join, startup, or end of an outage).
    GuildCreate {
        /// The server's own facts.
        guild: Guild,
        /// Its channels (not threads).
        channel_ids: Vec<ChannelId>,
        /// Its active threads.
        thread_ids: Vec<ChannelId>,
        /// Its member count, when Discord sent one (it does on every
        /// available `GUILD_CREATE`).
        members: Option<MemberCount>,
    },
    /// Name, icon, owner, MFA level, or roles changed.
    GuildUpdate {
        /// The server's facts after the change.
        guild: Guild,
    },
    /// Quack left (`unavailable == false`) or the server had an outage.
    GuildDelete {
        /// The server.
        guild_id: GuildId,
        /// Whether this is an outage rather than Quack leaving.
        unavailable: bool,
    },
    /// A channel was created.
    ChannelCreate(Channel),
    /// A channel changed.
    ChannelUpdate(Channel),
    /// A channel was deleted.
    ChannelDelete(Channel),
    /// A thread was deleted.
    ThreadDelete {
        /// Its server.
        guild_id: GuildId,
        /// The thread.
        thread_id: ChannelId,
        /// Its parent channel.
        parent_id: ChannelId,
    },
    /// A member joined.
    MemberAdd {
        /// The server.
        guild_id: GuildId,
        /// The member.
        member: Member,
        /// The server's member count after the join, when known.
        members: Option<MemberCount>,
    },
    /// A member changed (roles, nickname, timeout).
    MemberUpdate {
        /// The server.
        guild_id: GuildId,
        /// The member after the change.
        member: Member,
    },
    /// A member left, was kicked, or was banned.
    MemberRemove {
        /// The server.
        guild_id: GuildId,
        /// Their account.
        account: AccountFacts,
        /// The server's member count after the leave, when known.
        members: Option<MemberCount>,
    },
    /// A message was posted.
    MessageCreate(Message),
    /// A message changed. Discord now sends the whole message.
    MessageUpdate(Message),
    /// A message was deleted.
    MessageDelete {
        /// Its server.
        guild_id: Option<GuildId>,
        /// Its channel.
        channel_id: ChannelId,
        /// The message.
        message_id: MessageId,
    },
    /// Several messages were deleted at once.
    MessageDeleteBulk {
        /// Their server.
        guild_id: Option<GuildId>,
        /// Their channel.
        channel_id: ChannelId,
        /// The messages.
        message_ids: Vec<MessageId>,
    },
    /// A ban or unban, from `GUILD_AUDIT_LOG_ENTRY_CREATE`. Quack's own
    /// bans are included; the router skips them by `executor`.
    Ban {
        /// The server.
        guild_id: GuildId,
        /// Banned or unbanned.
        change: BanChange,
        /// The account.
        target: AccountId,
        /// Who did it.
        executor: Option<AccountId>,
        /// The audit-log reason.
        reason: Option<String>,
    },
    /// An interaction.
    Interaction(Box<Interaction>),
}

impl Event {
    /// Attaches the gateway's member count to a `GUILD_CREATE`, a join, or
    /// a leave (the shard loop reads it right after updating its caches).
    #[must_use]
    pub(crate) fn with_members(mut self, count: Option<MemberCount>) -> Self {
        if let Self::GuildCreate { members, .. }
        | Self::MemberAdd { members, .. }
        | Self::MemberRemove { members, .. } = &mut self
        {
            *members = count;
        }
        self
    }

    /// The server the event belongs to, for queue choice and spans.
    pub fn guild_id(&self) -> Option<GuildId> {
        match self {
            Self::Ready { .. } | Self::Resumed => None,
            Self::GuildCreate { guild, .. } | Self::GuildUpdate { guild } => Some(guild.id),
            Self::GuildDelete { guild_id, .. }
            | Self::ThreadDelete { guild_id, .. }
            | Self::MemberAdd { guild_id, .. }
            | Self::MemberUpdate { guild_id, .. }
            | Self::MemberRemove { guild_id, .. }
            | Self::Ban { guild_id, .. } => Some(*guild_id),
            Self::ChannelCreate(c) | Self::ChannelUpdate(c) | Self::ChannelDelete(c) => c.guild_id,
            Self::MessageCreate(m) | Self::MessageUpdate(m) => m.guild_id,
            Self::MessageDelete { guild_id, .. } | Self::MessageDeleteBulk { guild_id, .. } => {
                *guild_id
            }
            Self::Interaction(i) => i.guild_id,
        }
    }

    /// The gateway event name (a static name for metrics and spans).
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Ready { .. } => "READY",
            Self::Resumed => "RESUMED",
            Self::GuildCreate { .. } => "GUILD_CREATE",
            Self::GuildUpdate { .. } => "GUILD_UPDATE",
            Self::GuildDelete { .. } => "GUILD_DELETE",
            Self::ChannelCreate(_) => "CHANNEL_CREATE",
            Self::ChannelUpdate(_) => "CHANNEL_UPDATE",
            Self::ChannelDelete(_) => "CHANNEL_DELETE",
            Self::ThreadDelete { .. } => "THREAD_DELETE",
            Self::MemberAdd { .. } => "GUILD_MEMBER_ADD",
            Self::MemberUpdate { .. } => "GUILD_MEMBER_UPDATE",
            Self::MemberRemove { .. } => "GUILD_MEMBER_REMOVE",
            Self::MessageCreate(_) => "MESSAGE_CREATE",
            Self::MessageUpdate(_) => "MESSAGE_UPDATE",
            Self::MessageDelete { .. } => "MESSAGE_DELETE",
            Self::MessageDeleteBulk { .. } => "MESSAGE_DELETE_BULK",
            Self::Ban { .. } => "GUILD_AUDIT_LOG_ENTRY_CREATE",
            Self::Interaction(_) => "INTERACTION_CREATE",
        }
    }
}

fn guild_create(guild: &twilight_model::guild::Guild) -> Option<Event> {
    Some(Event::GuildCreate {
        guild: convert::guild(guild)?,
        channel_ids: guild
            .channels
            .iter()
            .filter_map(|c| convert::id(c.id))
            .collect(),
        thread_ids: guild
            .threads
            .iter()
            .filter_map(|c| convert::id(c.id))
            .collect(),
        members: None,
    })
}

fn guild_update(g: &twilight_model::guild::PartialGuild) -> Option<Event> {
    Some(Event::GuildUpdate {
        guild: Guild {
            id: convert::id(g.id)?,
            name: g.name.clone(),
            icon_hash: g.icon.map(|h| h.to_string()),
            owner_id: convert::id(g.owner_id)?,
            mfa_required: g.mfa_level == twilight_model::guild::MfaLevel::Elevated,
            roles: g.roles.iter().filter_map(convert::role).collect(),
        },
    })
}

fn ban(entry: &twilight_model::guild::audit_log::AuditLogEntry) -> Option<Event> {
    let change = match entry.action_type {
        AuditLogEventType::MemberBanAdd => BanChange::Banned,
        AuditLogEventType::MemberBanRemove => BanChange::Unbanned,
        _ => return None,
    };
    Some(Event::Ban {
        guild_id: convert::id(entry.guild_id?)?,
        change,
        target: convert::id(entry.target_id?)?,
        executor: entry.user_id.and_then(convert::id),
        reason: entry.reason.clone(),
    })
}

/// Maps a Twilight event; `None` for events Quack doesn't dispatch.
/// `sessions_before` is how many READYs this process saw before this one.
pub(crate) fn map(raw: &Raw, sessions_before: u32) -> Option<Event> {
    match raw {
        Raw::Ready(ready) => Some(Event::Ready {
            quack_id: convert::id(ready.user.id)?,
            guild_ids: ready
                .guilds
                .iter()
                .filter_map(|g| convert::id(g.id))
                .collect(),
            missed_events: sessions_before > 0,
        }),
        Raw::Resumed => Some(Event::Resumed),
        Raw::GuildCreate(create) => match &**create {
            GuildCreate::Available(guild) => guild_create(guild),
            GuildCreate::Unavailable(_) => None,
        },
        Raw::GuildUpdate(update) => guild_update(update),
        Raw::GuildDelete(delete) => Some(Event::GuildDelete {
            guild_id: convert::id(delete.id)?,
            unavailable: delete.unavailable.unwrap_or(false),
        }),
        Raw::ChannelCreate(c) => convert::channel(c).map(Event::ChannelCreate),
        Raw::ChannelUpdate(c) => convert::channel(c).map(Event::ChannelUpdate),
        Raw::ChannelDelete(c) => convert::channel(c).map(Event::ChannelDelete),
        Raw::ThreadDelete(t) => Some(Event::ThreadDelete {
            guild_id: convert::id(t.guild_id)?,
            thread_id: convert::id(t.id)?,
            parent_id: convert::id(t.parent_id)?,
        }),
        Raw::MemberAdd(add) => Some(Event::MemberAdd {
            guild_id: convert::id(add.guild_id)?,
            member: convert::member(&add.member)?,
            members: None,
        }),
        Raw::MemberUpdate(update) => Some(Event::MemberUpdate {
            guild_id: convert::id(update.guild_id)?,
            member: member_update(update)?,
        }),
        Raw::MemberRemove(remove) => Some(Event::MemberRemove {
            guild_id: convert::id(remove.guild_id)?,
            account: convert::user(&remove.user)?,
            members: None,
        }),
        Raw::MessageCreate(m) => convert::message(m).map(Event::MessageCreate),
        Raw::MessageUpdate(m) => convert::message(m).map(Event::MessageUpdate),
        Raw::MessageDelete(d) => Some(Event::MessageDelete {
            guild_id: d.guild_id.and_then(convert::id),
            channel_id: convert::id(d.channel_id)?,
            message_id: convert::id(d.id)?,
        }),
        Raw::MessageDeleteBulk(d) => Some(Event::MessageDeleteBulk {
            guild_id: d.guild_id.and_then(convert::id),
            channel_id: convert::id(d.channel_id)?,
            message_ids: d.ids.iter().filter_map(|id| convert::id(*id)).collect(),
        }),
        Raw::GuildAuditLogEntryCreate(entry) => ban(entry),
        Raw::InteractionCreate(create) => {
            Some(Event::Interaction(Box::new(interaction::decode(create))))
        }
        _ => None,
    }
}
