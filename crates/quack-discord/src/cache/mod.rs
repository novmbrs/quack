//! In-memory Discord state (§11.10): the guild cache, the bounded member
//! cache, and the account cache the member cache owns. All are fed by the
//! gateway loop before events are dispatched and produce quack-core facts.
//!
//! The server activity log's message cache stores redacted text under the
//! module's privacy settings, so it lives in `quack-activity-log` (§11.10),
//! not here.

mod account;
mod guild;
mod lru;
mod member;

#[cfg(test)]
mod tests;

use twilight_model::gateway::event::Event;
use twilight_model::gateway::payload::incoming::{GuildCreate, MemberUpdate};

use crate::convert::{self, MemberParts};
use crate::model::Member;

pub use account::AccountCache;
pub use guild::GuildCache;
pub use member::{MemberCache, Since};

/// The member a `GUILD_MEMBER_UPDATE` describes.
pub(crate) fn member_update(update: &MemberUpdate) -> Option<Member> {
    convert::member_parts(&MemberParts {
        user: &update.user,
        nick: update.nick.as_ref(),
        roles: &update.roles,
        joined_at: update.joined_at,
        timeout_until: update.communication_disabled_until,
    })
}

/// Moves the guild cache's member count for a join or leave. Runs before
/// [`feed_members`]: a join for a member the member cache already holds
/// from the gateway is a repeat and moves nothing.
pub(crate) fn count_members(guilds: &GuildCache, members: &MemberCache, event: &Event) {
    match event {
        Event::MemberAdd(add) => {
            if let (Some(guild), Some(member)) =
                (convert::id(add.guild_id), convert::id(add.member.user.id))
                && !members.holds_from_event(guild, member)
            {
                guilds.count_member(guild, true);
            }
        }
        Event::MemberRemove(remove) => {
            if let Some(guild) = convert::id(remove.guild_id) {
                guilds.count_member(guild, false);
            }
        }
        _ => {}
    }
}

/// Feeds the member cache, and through it the account cache, from a
/// gateway event. Interaction and message authors refresh their accounts.
pub(crate) fn feed_members(members: &MemberCache, event: &Event) {
    let accounts = members.accounts();
    match event {
        Event::Ready(_) => members.clear(),
        Event::GuildCreate(create) => {
            if let GuildCreate::Available(guild) = &**create
                && let Some(id) = convert::id(guild.id)
            {
                for member in guild.members.iter().filter_map(convert::member) {
                    members.put_event(id, member);
                }
            }
        }
        Event::GuildDelete(delete) => {
            if let Some(id) = convert::id(delete.id) {
                members.clear_guild(id);
            }
        }
        Event::MemberAdd(add) => {
            if let (Some(guild), Some(member)) =
                (convert::id(add.guild_id), convert::member(&add.member))
            {
                members.put_event(guild, member);
            }
        }
        Event::MemberUpdate(update) => {
            if let (Some(guild), Some(member)) =
                (convert::id(update.guild_id), member_update(update))
            {
                members.put_event(guild, member);
            }
        }
        Event::MemberRemove(remove) => {
            if let (Some(guild), Some(member)) =
                (convert::id(remove.guild_id), convert::id(remove.user.id))
            {
                members.remove(guild, member);
            }
            accounts_of(accounts, [&remove.user]);
        }
        Event::InteractionCreate(create) => accounts_of(accounts, create.author()),
        Event::MessageCreate(create) if create.webhook_id.is_none() => {
            accounts_of(accounts, [&create.author]);
        }
        _ => {}
    }
}

fn accounts_of<'a>(
    accounts: &AccountCache,
    users: impl IntoIterator<Item = &'a twilight_model::user::User>,
) {
    let now = std::time::Instant::now();
    for account in users.into_iter().filter_map(convert::user) {
        accounts.put(account, now);
    }
}
