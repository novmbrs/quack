//! The staff-only channel check from current facts (§13.1.3), run when a
//! channel is saved and before every post to a staff destination (log
//! channel, appeal posts, evidence copies, ticket posts).

use quack_core::ids::{ChannelId, GuildId, RoleId};
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

/// A channel's facts gathered for the check, before any lock.
#[derive(Clone, Debug)]
pub(crate) enum Gathered {
    /// The facts the pure check takes.
    Facts(Box<StaffChannelFacts>),
    /// The channel no longer exists.
    Missing,
    /// Discord answers 403 for it.
    Inaccessible,
    /// Discord didn't answer.
    Unavailable,
}

/// Gathers what the staff-only check needs about `channel` (§13.1.3).
pub(crate) async fn gather(engine: &Engine, guild: GuildId, channel: ChannelId) -> Gathered {
    let discord = engine.discord();
    let Outcome::Done(facts) = discord.guild_facts(guild).await else {
        return Gathered::Unavailable;
    };
    let channel = match discord.channel_facts(guild, channel).await {
        Outcome::Done(Some(c)) => c,
        Outcome::Done(None) => return Gathered::Missing,
        Outcome::Refused(r) if r.kind == RefusalKind::UnknownChannel => return Gathered::Missing,
        Outcome::Refused(_) => return Gathered::Inaccessible,
        _ => return Gathered::Unavailable,
    };
    match staff_facts(engine, &facts, &channel).await {
        Some(staff) => Gathered::Facts(Box::new(staff)),
        None => Gathered::Unavailable,
    }
}

/// Checks gathered facts with these moderator roles (the ones a settings
/// change is saving, or the saved ones).
pub(crate) fn check(
    gathered: &Gathered,
    moderator_roles: &[RoleId],
    needs: Permissions,
) -> ChannelCheck {
    match gathered {
        Gathered::Facts(facts) => {
            let problems = check_staff_channel(facts, moderator_roles, needs);
            if problems.is_empty() {
                ChannelCheck::Ok
            } else {
                ChannelCheck::Problems(problems)
            }
        }
        Gathered::Missing => ChannelCheck::Missing,
        Gathered::Inaccessible => ChannelCheck::Problems(vec![ChannelProblem::Inaccessible]),
        Gathered::Unavailable => ChannelCheck::Unavailable,
    }
}

/// Checks `channel` of `guild` as a staff destination where Quack needs
/// `needs`, from current facts and the saved moderator roles.
pub(crate) async fn staff_channel(
    engine: &Engine,
    guild: GuildId,
    channel: ChannelId,
    needs: Permissions,
) -> ChannelCheck {
    let gathered = gather(engine, guild, channel).await;
    let roles = {
        let Ok(mut conn) = engine.db().conn().await else {
            return ChannelCheck::Unavailable;
        };
        match staff_roles(&mut conn, guild).await {
            Ok(roles) => roles,
            Err(_) => return ChannelCheck::Unavailable,
        }
    };
    check(&gathered, &roles.moderator_role_ids, needs)
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
