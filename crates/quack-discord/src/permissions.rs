//! The facts structs built from Discord data, with permissions from
//! quack-core's one calculation (`quack_core::permissions::in_channel`).

use quack_core::ids::RoleId;
pub use quack_core::permissions::{ALL, Subject, in_channel, in_guild};
use quack_core::permissions::{ChannelFacts, GuildFacts, MemberFacts, Permissions};
use time::OffsetDateTime;

use crate::model::{Channel, Member};

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
/// threads; without it Quack has no permissions there (fail closed).
pub fn channel_facts(
    guild: &GuildFacts,
    channel: &Channel,
    parent: Option<&Channel>,
    now: OffsetDateTime,
) -> ChannelFacts {
    let thread = channel.is_thread();
    let overwrites = match parent {
        Some(parent) if thread => Some(parent.overwrites.clone()),
        // A thread without its parent: the overwrites that apply are
        // unknown, so Quack is taken to have nothing there.
        None if thread => None,
        _ => Some(channel.overwrites.clone()),
    };
    let quack_permissions = match (&guild.quack, &overwrites) {
        (Some(quack), Some(overwrites)) => in_channel(
            guild,
            Subject {
                account: quack.id.account(),
                role_ids: &quack.role_ids,
                timed_out: timed_out(quack.timeout_until, now),
            },
            overwrites,
            thread,
        ),
        _ => Permissions::NONE,
    };
    let overwrites = overwrites.unwrap_or_default();
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
