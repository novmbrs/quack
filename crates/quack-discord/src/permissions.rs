//! Discord's permission computation over quack-core facts, and the facts
//! structs built from Discord data.
//!
//! Twilight's calculator (`twilight-util`) omits Discord's implicit rules
//! (no View Channel means no permissions in the channel; no Send Messages
//! drops the permissions that depend on it) and timeouts, so this is a small
//! purpose-built version of Discord's documented algorithm.

use quack_core::ids::{AccountId, RoleId};
use quack_core::permissions::{
    ChannelFacts, GuildFacts, MemberFacts, Overwrite, OverwriteTarget, Permission, Permissions,
};
use time::OffsetDateTime;

use crate::model::{Channel, Member};

/// Every permission: what the owner and Administrator get.
pub const ALL: Permissions = Permissions(u64::MAX);

const VIEW_CHANNEL: u64 = Permission::ViewChannel.bit();
const READ_MESSAGE_HISTORY: u64 = Permission::ReadMessageHistory.bit();
const SEND_MESSAGES: u64 = Permission::SendMessages.bit();
/// What losing Send Messages also takes away in a text channel: Mention
/// Everyone, Send TTS Messages, Attach Files, Embed Links.
const NEEDS_SEND: u64 =
    (1 << 17) | (1 << 12) | Permission::AttachFiles.bit() | Permission::EmbedLinks.bit();
const ADMINISTRATOR: u64 = Permission::Administrator.bit();

/// Who permissions are computed for.
#[derive(Clone, Copy, Debug)]
pub struct Subject<'a> {
    /// Their account.
    pub account: AccountId,
    /// Their role ids (without @everyone).
    pub role_ids: &'a [RoleId],
    /// Whether they are timed out right now.
    pub timed_out: bool,
}

/// Server-level permissions: @everyone plus every role; the owner and
/// Administrator get everything; a timed-out member keeps only View Channel
/// and Read Message History.
pub fn in_guild(guild: &GuildFacts, who: Subject<'_>) -> Permissions {
    if who.account == guild.owner_id {
        return ALL;
    }
    let mut bits = guild
        .role(guild.guild_id.everyone_role())
        .map_or(0, |r| r.permissions.0);
    for id in who.role_ids {
        if let Some(role) = guild.role(*id) {
            bits |= role.permissions.0;
        }
    }
    if bits & ADMINISTRATOR != 0 {
        return ALL;
    }
    if who.timed_out {
        bits &= VIEW_CHANNEL | READ_MESSAGE_HISTORY;
    }
    Permissions(bits)
}

/// Permissions in a channel with these overwrites: server permissions, then
/// the @everyone overwrite, then all role overwrites together, then the
/// member's own overwrite; no View Channel means nothing.
pub fn in_channel(guild: &GuildFacts, who: Subject<'_>, overwrites: &[Overwrite]) -> Permissions {
    let base = in_guild(guild, who);
    if base.0 & ADMINISTRATOR != 0 {
        return ALL;
    }
    let everyone = guild.guild_id.everyone_role();
    let mut bits = base.0;
    if let Some(o) = overwrites
        .iter()
        .find(|o| o.target == OverwriteTarget::Role(everyone))
    {
        bits = (bits & !o.deny.0) | o.allow.0;
    }
    let (mut allow, mut deny) = (0, 0);
    for o in overwrites {
        if let OverwriteTarget::Role(role) = o.target
            && role != everyone
            && who.role_ids.contains(&role)
        {
            allow |= o.allow.0;
            deny |= o.deny.0;
        }
    }
    bits = (bits & !deny) | allow;
    if let Some(o) = overwrites
        .iter()
        .find(|o| o.target == OverwriteTarget::Member(who.account.member()))
    {
        bits = (bits & !o.deny.0) | o.allow.0;
    }
    if who.timed_out {
        bits &= VIEW_CHANNEL | READ_MESSAGE_HISTORY;
    }
    if bits & VIEW_CHANNEL == 0 {
        return Permissions::NONE;
    }
    if bits & SEND_MESSAGES == 0 {
        bits &= !NEEDS_SEND;
    }
    Permissions(bits)
}

/// The position of the member's highest role (0 for @everyone only).
/// Discord breaks position ties by role id, but Quack only compares
/// positions and refuses equal ones (§2.3), so the position is enough.
pub fn highest_position(guild: &GuildFacts, role_ids: &[RoleId]) -> u32 {
    role_ids
        .iter()
        .filter_map(|id| guild.role(*id))
        .map(|r| r.position)
        .max()
        .unwrap_or(0)
}

fn timed_out(until: Option<OffsetDateTime>, now: OffsetDateTime) -> bool {
    until.is_some_and(|until| until > now)
}

/// A member's facts in a server: roles that still exist, highest position,
/// and computed permissions at `now`.
pub fn member_facts(guild: &GuildFacts, member: &Member, now: OffsetDateTime) -> MemberFacts {
    let role_ids: Vec<RoleId> = member
        .role_ids
        .iter()
        .copied()
        .filter(|id| guild.role(*id).is_some())
        .collect();
    let permissions = in_guild(
        guild,
        Subject {
            account: member.id.account(),
            role_ids: &role_ids,
            timed_out: timed_out(member.timeout_until, now),
        },
    );
    MemberFacts {
        id: member.id,
        bot: member.bot,
        highest_role_position: highest_position(guild, &role_ids),
        role_ids,
        permissions,
        joined_at: member.joined_at,
        timeout_until: member.timeout_until,
        username: member.username.clone(),
        global_name: member.global_name.clone(),
        nick: member.nick.clone(),
    }
}

/// A channel's facts with Quack's permissions in it. A thread uses its
/// parent's overwrites (the ones that apply to it), so pass the parent for
/// threads; without it a thread gets server-level permissions.
pub fn channel_facts(
    guild: &GuildFacts,
    channel: &Channel,
    parent: Option<&Channel>,
    now: OffsetDateTime,
) -> ChannelFacts {
    let overwrites = match parent {
        Some(parent) if channel.is_thread() => parent.overwrites.clone(),
        _ => channel.overwrites.clone(),
    };
    let quack_permissions = guild.quack.as_ref().map_or(Permissions::NONE, |quack| {
        in_channel(
            guild,
            Subject {
                account: quack.id.account(),
                role_ids: &quack.role_ids,
                timed_out: timed_out(quack.timeout_until, now),
            },
            &overwrites,
        )
    });
    ChannelFacts {
        id: channel.id,
        guild_id: channel.guild_id.unwrap_or(guild.guild_id),
        kind: channel.kind,
        name: channel.name.clone(),
        parent_id: channel.parent_id,
        overwrites,
        quack_permissions,
    }
}

#[cfg(test)]
mod tests;
