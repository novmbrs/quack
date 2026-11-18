//! The staff-only channel check (§13.1.3).

use super::{ActorFacts, Permission, Permissions, is_staff_for_access};
use crate::ids::{AccountId, GuildId, MemberId, RoleId};

/// Whom a permission overwrite applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverwriteTarget {
    /// A role (the server's id is @everyone).
    Role(RoleId),
    /// One member.
    Member(MemberId),
}

/// One channel permission overwrite.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Overwrite {
    /// Whom it applies to.
    pub target: OverwriteTarget,
    /// Permissions it allows.
    pub allow: Permissions,
    /// Permissions it denies.
    pub deny: Permissions,
}

/// Cached facts about a channel and its server, enough to decide whether
/// only staff can read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StaffChannelFacts {
    /// The server being configured.
    pub guild_id: GuildId,
    /// The server the channel belongs to.
    pub channel_guild_id: GuildId,
    /// Whether it is a text channel (Discord type 0).
    pub is_text: bool,
    /// The channel's permission overwrites.
    pub overwrites: Vec<Overwrite>,
    /// Quack's computed permissions in the channel.
    pub quack_permissions: Permissions,
    /// The server-level permissions of each role that has an overwrite.
    pub role_permissions: Vec<(RoleId, Permissions)>,
    /// The server owner.
    pub owner_id: AccountId,
    /// Quack's own account.
    pub quack_id: AccountId,
    /// Facts for each member that has an overwrite. A member without facts
    /// is treated as not staff.
    pub members: Vec<(MemberId, ActorFacts)>,
}

/// One reason a channel is not a usable staff destination.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChannelProblem {
    /// Not a text channel in this server.
    NotText,
    /// @everyone can view it.
    EveryoneCanView,
    /// A role that is not staff can view it.
    RoleCanView(RoleId),
    /// A member who is not staff can view it.
    MemberCanView(MemberId),
    /// Quack lacks these permissions there.
    QuackMissing(Vec<Permission>),
}

impl ChannelProblem {
    /// The problem's machine code.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NotText => "not_text",
            Self::EveryoneCanView => "everyone_can_view",
            Self::RoleCanView(_) => "role_can_view",
            Self::MemberCanView(_) => "member_can_view",
            Self::QuackMissing(_) => "quack_missing",
        }
    }
}

/// Lists why `channel` is not a staff-only destination where Quack holds
/// `quack_needs`; empty means it passes. Runs when a channel is saved and
/// before every post to a staff destination.
pub fn check_staff_channel(
    channel: &StaffChannelFacts,
    moderator_role_ids: &[RoleId],
    quack_needs: Permissions,
) -> Vec<ChannelProblem> {
    if !channel.is_text || channel.channel_guild_id != channel.guild_id {
        return vec![ChannelProblem::NotText];
    }
    let view = Permissions::from(Permission::ViewChannel);
    let everyone = channel.guild_id.everyone_role();
    let mut problems = Vec::new();
    let everyone_denied = channel
        .overwrites
        .iter()
        .any(|o| o.target == OverwriteTarget::Role(everyone) && o.deny.contains(view));
    if !everyone_denied {
        problems.push(ChannelProblem::EveryoneCanView);
    }
    for overwrite in channel.overwrites.iter().filter(|o| o.allow.contains(view)) {
        match overwrite.target {
            OverwriteTarget::Role(role)
                if role != everyone && !role_is_staff(channel, role, moderator_role_ids) =>
            {
                problems.push(ChannelProblem::RoleCanView(role));
            }
            OverwriteTarget::Member(member)
                if !member_is_staff(channel, member, moderator_role_ids) =>
            {
                problems.push(ChannelProblem::MemberCanView(member));
            }
            _ => {}
        }
    }
    let missing = channel.quack_permissions.missing(quack_needs);
    if !missing.is_empty() {
        problems.push(ChannelProblem::QuackMissing(missing));
    }
    problems
}

fn role_is_staff(channel: &StaffChannelFacts, role: RoleId, moderator_role_ids: &[RoleId]) -> bool {
    let staff_bits = |p: Permissions| {
        p.contains(Permission::Administrator.into())
            || p.contains(Permission::ModerateMembers.into())
    };
    moderator_role_ids.contains(&role)
        || channel
            .role_permissions
            .iter()
            .any(|(id, perms)| *id == role && staff_bits(*perms))
}

fn member_is_staff(
    channel: &StaffChannelFacts,
    member: MemberId,
    moderator_role_ids: &[RoleId],
) -> bool {
    let account = member.account();
    account == channel.quack_id
        || account == channel.owner_id
        || channel
            .members
            .iter()
            .any(|(id, facts)| *id == member && is_staff_for_access(facts, moderator_role_ids))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GUILD: u64 = 100;

    fn role(n: u64) -> RoleId {
        RoleId::new(n).unwrap()
    }

    fn member(n: u64) -> MemberId {
        MemberId::new(n).unwrap()
    }

    fn deny_everyone() -> Overwrite {
        Overwrite {
            target: OverwriteTarget::Role(role(GUILD)),
            allow: Permissions::NONE,
            deny: Permission::ViewChannel.into(),
        }
    }

    fn allow(target: OverwriteTarget) -> Overwrite {
        Overwrite {
            target,
            allow: Permission::ViewChannel.into(),
            deny: Permissions::NONE,
        }
    }

    fn channel(overwrites: Vec<Overwrite>) -> StaffChannelFacts {
        StaffChannelFacts {
            guild_id: GuildId::new(GUILD).unwrap(),
            channel_guild_id: GuildId::new(GUILD).unwrap(),
            is_text: true,
            overwrites,
            quack_permissions: Permission::ViewChannel | Permission::SendMessages,
            role_permissions: vec![
                (role(1), Permission::ModerateMembers.into()),
                (role(2), Permission::Administrator.into()),
                (role(3), Permission::ManageGuild.into()),
            ],
            owner_id: AccountId::new(50).unwrap(),
            quack_id: AccountId::new(51).unwrap(),
            members: vec![
                (
                    member(60),
                    ActorFacts {
                        permissions: Permission::ModerateMembers.into(),
                        ..ActorFacts::default()
                    },
                ),
                (member(61), ActorFacts::default()),
            ],
        }
    }

    fn needs() -> Permissions {
        Permission::ViewChannel | Permission::SendMessages
    }

    #[test]
    fn a_staff_only_channel_passes() {
        let c = channel(vec![
            deny_everyone(),
            allow(OverwriteTarget::Role(role(1))),
            allow(OverwriteTarget::Role(role(2))),
            allow(OverwriteTarget::Role(role(9))),
            allow(OverwriteTarget::Member(member(50))),
            allow(OverwriteTarget::Member(member(51))),
            allow(OverwriteTarget::Member(member(60))),
        ]);
        assert_eq!(check_staff_channel(&c, &[role(9)], needs()), vec![]);
    }

    #[test]
    fn reports_every_problem() {
        let mut c = channel(vec![
            allow(OverwriteTarget::Role(role(3))),
            allow(OverwriteTarget::Member(member(61))),
            allow(OverwriteTarget::Member(member(62))),
        ]);
        c.quack_permissions = Permission::ViewChannel.into();
        assert_eq!(
            check_staff_channel(&c, &[], needs() | Permission::AttachFiles),
            vec![
                ChannelProblem::EveryoneCanView,
                ChannelProblem::RoleCanView(role(3)),
                ChannelProblem::MemberCanView(member(61)),
                ChannelProblem::MemberCanView(member(62)),
                ChannelProblem::QuackMissing(vec![
                    Permission::SendMessages,
                    Permission::AttachFiles
                ]),
            ]
        );
    }

    #[test]
    fn not_text_or_other_server_stops_early() {
        let mut c = channel(vec![]);
        c.is_text = false;
        assert_eq!(
            check_staff_channel(&c, &[], needs()),
            vec![ChannelProblem::NotText]
        );
        let mut c = channel(vec![]);
        c.channel_guild_id = GuildId::new(7).unwrap();
        assert_eq!(
            check_staff_channel(&c, &[], needs()),
            vec![ChannelProblem::NotText]
        );
        assert_eq!(ChannelProblem::NotText.code(), "not_text");
    }
}
