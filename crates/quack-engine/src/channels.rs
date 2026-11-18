//! The staff-only channel check from current facts (§13.1.3), run when a
//! channel is saved and before every post to a staff destination (log
//! channel, appeal posts, evidence copies, ticket posts).

use quack_core::ids::{ChannelId, GuildId};
use quack_core::permissions::{
    ActorFacts, ChannelFacts, ChannelProblem, GuildFacts, OverwriteTarget, Permissions,
    StaffChannelFacts, check_staff_channel,
};

use crate::Engine;
use crate::port::{Fresh, Membership, Outcome, RefusalKind};
use crate::principal::staff_roles;

/// What the check found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ChannelCheck {
    /// A staff-only text channel where Quack holds what it needs.
    Ok,
    /// Why it is not a usable staff destination.
    Problems(Vec<ChannelProblem>),
    /// The channel no longer exists (404 Unknown Channel).
    Missing,
    /// Discord didn't answer; try again later.
    Unavailable,
}

/// Checks `channel` of `guild` as a staff destination where Quack needs
/// `needs`, from current facts.
pub(crate) async fn staff_channel(
    engine: &Engine,
    guild: GuildId,
    channel: ChannelId,
    needs: Permissions,
) -> ChannelCheck {
    let discord = engine.discord();
    let Outcome::Done(facts) = discord.guild_facts(guild).await else {
        return ChannelCheck::Unavailable;
    };
    let channel = match discord.channel_facts(guild, channel).await {
        Outcome::Done(Some(c)) => c,
        Outcome::Done(None) => return ChannelCheck::Missing,
        Outcome::Refused(r) if r.kind == RefusalKind::UnknownChannel => {
            return ChannelCheck::Missing;
        }
        Outcome::Refused(_) => return ChannelCheck::Problems(vec![ChannelProblem::Inaccessible]),
        _ => return ChannelCheck::Unavailable,
    };
    let roles = {
        let Ok(mut conn) = engine.db().conn().await else {
            return ChannelCheck::Unavailable;
        };
        match staff_roles(&mut conn, guild).await {
            Ok(roles) => roles.existing(&facts.role_ids()),
            Err(_) => return ChannelCheck::Unavailable,
        }
    };
    let Some(staff) = staff_facts(engine, &facts, &channel).await else {
        return ChannelCheck::Unavailable;
    };
    let problems = check_staff_channel(&staff, &roles.moderator_role_ids, needs);
    if problems.is_empty() {
        ChannelCheck::Ok
    } else {
        ChannelCheck::Problems(problems)
    }
}

/// The facts the pure check takes: role permissions of every role with an
/// overwrite, and the facts of every member with one.
async fn staff_facts(
    engine: &Engine,
    guild: &GuildFacts,
    channel: &ChannelFacts,
) -> Option<StaffChannelFacts> {
    let quack_id = guild.quack.as_ref()?.id.account();
    let mut role_permissions = Vec::new();
    let mut members = Vec::new();
    for overwrite in &channel.overwrites {
        match overwrite.target {
            OverwriteTarget::Role(role) => {
                let permissions = guild
                    .role(role)
                    .map_or(Permissions::NONE, |r| r.permissions);
                role_permissions.push((role, permissions));
            }
            OverwriteTarget::Member(member) => {
                match engine
                    .discord()
                    .member(guild.guild_id, member, Fresh::Cached)
                    .await
                {
                    Outcome::Done(Membership::Present(m)) => members.push((
                        member,
                        ActorFacts {
                            is_owner: member.account() == guild.owner_id,
                            permissions: m.permissions,
                            role_ids: m.role_ids,
                            mfa_enabled: false,
                        },
                    )),
                    Outcome::Done(Membership::Absent) => {}
                    _ => return None,
                }
            }
        }
    }
    Some(StaffChannelFacts {
        guild_id: guild.guild_id,
        channel_guild_id: channel.guild_id,
        is_text: channel.is_text(),
        overwrites: channel.overwrites.clone(),
        quack_permissions: channel.quack_permissions,
        role_permissions,
        owner_id: guild.owner_id,
        quack_id,
        members,
    })
}
