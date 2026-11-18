//! Whether a staff member can read a channel, from current facts (§7.2
//! step 2): View Channel and Read Message History computed with
//! overwrites; threads use their parent channel; private threads also need
//! thread membership or Manage Threads. Access is never assumed.

use quack_core::ids::{AccountId, ChannelId, GuildId};
use quack_core::permissions::{
    ChannelFacts, GuildFacts, MemberFacts, OverwriteTarget, Permission, Permissions,
};

use crate::Engine;
use crate::port::{Fresh, Membership, Outcome};

/// Discord's channel types for threads, and the private one.
const THREAD_TYPES: [u8; 3] = [10, 11, 12];
const PRIVATE_THREAD: u8 = 12;

/// Facts were unavailable; try again later.
pub(super) struct Unavailable;

/// Whether `account` can read `channel` in `guild` now.
pub(super) async fn can_read(
    engine: &Engine,
    guild: GuildId,
    channel: ChannelId,
    account: AccountId,
) -> Result<bool, Unavailable> {
    let discord = engine.discord();
    let Outcome::Done(facts) = discord.guild_facts(guild).await else {
        return Err(Unavailable);
    };
    let member = match discord.member(guild, account.member(), Fresh::Cached).await {
        Outcome::Done(Membership::Present(m)) => m,
        Outcome::Done(Membership::Absent) => return Ok(false),
        _ => return Err(Unavailable),
    };
    let Some(target) = channel_facts(engine, guild, channel).await? else {
        return Ok(false);
    };
    let (parent, private) = if THREAD_TYPES.contains(&target.kind) {
        let Some(parent) = target.parent_id else {
            return Ok(false);
        };
        let Some(parent) = channel_facts(engine, guild, parent).await? else {
            return Ok(false);
        };
        (parent, target.kind == PRIVATE_THREAD)
    } else {
        (target, false)
    };
    let perms = in_channel(&facts, &member, &parent);
    let read = Permission::ViewChannel | Permission::ReadMessageHistory;
    if !perms.contains(read) {
        return Ok(false);
    }
    if !private || perms.allows(Permission::ManageThreads) {
        return Ok(true);
    }
    match discord.is_thread_member(channel, account).await {
        Outcome::Done(member) => Ok(member),
        _ => Err(Unavailable),
    }
}

async fn channel_facts(
    engine: &Engine,
    guild: GuildId,
    channel: ChannelId,
) -> Result<Option<ChannelFacts>, Unavailable> {
    match engine.discord().channel_facts(guild, channel).await {
        Outcome::Done(c) => Ok(c),
        Outcome::Refused(_) => Ok(None),
        _ => Err(Unavailable),
    }
}

/// A member's permissions in a channel (Discord's algorithm): owner and
/// Administrator hold everything; then the @everyone overwrite, the
/// member's role overwrites together, and the member's own overwrite.
pub(super) fn in_channel(
    guild: &GuildFacts,
    member: &MemberFacts,
    channel: &ChannelFacts,
) -> Permissions {
    let all = Permissions(u64::MAX);
    if member.id.account() == guild.owner_id || member.is_administrator() {
        return all;
    }
    let mut perms = member.permissions.0;
    let everyone = guild.guild_id.everyone_role();
    let mut role_allow = 0;
    let mut role_deny = 0;
    for o in &channel.overwrites {
        match o.target {
            OverwriteTarget::Role(r) if r == everyone => {
                perms = (perms & !o.deny.0) | o.allow.0;
            }
            OverwriteTarget::Role(r) if member.role_ids.contains(&r) => {
                role_allow |= o.allow.0;
                role_deny |= o.deny.0;
            }
            _ => {}
        }
    }
    perms = (perms & !role_deny) | role_allow;
    for o in &channel.overwrites {
        if o.target == OverwriteTarget::Member(member.id) {
            perms = (perms & !o.deny.0) | o.allow.0;
        }
    }
    Permissions(perms)
}
