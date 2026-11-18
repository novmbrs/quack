//! Discord's permission computation (its documented algorithm) over
//! Quack's facts: the one calculation every caller uses, with the implicit
//! rules Twilight's omits (no View Channel means nothing in the channel; no
//! send permission drops what depends on it; in a thread the send
//! permission is Send Messages in Threads) and timeouts, applied after
//! every overwrite.

use super::{GuildFacts, Overwrite, OverwriteTarget, Permission, Permissions};
use crate::ids::{AccountId, RoleId};

/// Every permission: what the owner and Administrator get.
pub const ALL: Permissions = Permissions(u64::MAX);

const VIEW_CHANNEL: u64 = Permission::ViewChannel.bit();
const READ_MESSAGE_HISTORY: u64 = Permission::ReadMessageHistory.bit();
const SEND_MESSAGES: u64 = Permission::SendMessages.bit();
const SEND_IN_THREADS: u64 = Permission::SendMessagesInThreads.bit();
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

/// Permissions in a channel with these overwrites (a thread's are its
/// parent's): server permissions, then the @everyone overwrite, then all
/// role overwrites together, then the member's own overwrite, then the
/// timeout; no View Channel means nothing. In a `thread`, sending is Send
/// Messages in Threads, reported as Send Messages ("can send here").
pub fn in_channel(
    guild: &GuildFacts,
    who: Subject<'_>,
    overwrites: &[Overwrite],
    thread: bool,
) -> Permissions {
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
    let send = if thread {
        SEND_IN_THREADS
    } else {
        SEND_MESSAGES
    };
    if bits & send == 0 {
        bits &= !(NEEDS_SEND | SEND_MESSAGES);
    } else {
        bits |= SEND_MESSAGES;
    }
    Permissions(bits)
}
