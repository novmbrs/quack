//! Whether a staff member can read a channel, from current facts (§7.2
//! step 3): View Channel and Read Message History computed with
//! overwrites; threads use their parent channel; private threads also need
//! thread membership or Manage Threads. Access is never assumed.

use quack_core::ids::{AccountId, ChannelId, GuildId};
use quack_core::permissions::{ChannelFacts, Permission, Subject, in_channel};

use crate::Engine;
use crate::port::{Fresh, Membership, Outcome};

/// Discord's channel types for threads, and the private one.
const THREAD_TYPES: [u8; 3] = [10, 11, 12];
const PRIVATE_THREAD: u8 = 12;

/// Facts were unavailable; try again later.
pub(crate) struct Unavailable;

/// Whether `account` can read `channel` in `guild` now.
pub(crate) async fn can_read(
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
    let thread = THREAD_TYPES.contains(&target.kind);
    let (parent, private) = if thread {
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
    let timed_out = member
        .timeout_until
        .is_some_and(|until| until > time::OffsetDateTime::now_utc());
    let who = Subject {
        account: member.id.account(),
        role_ids: &member.role_ids,
        timed_out,
    };
    let perms = in_channel(&facts, who, &parent.overwrites, thread);
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
