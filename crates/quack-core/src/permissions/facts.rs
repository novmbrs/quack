//! Plain facts about Discord that the `Discord` port returns and the pure
//! checks take (§17.3). No Discord library types: the port converts.

use time::OffsetDateTime;

use super::{MemberSubject, Overwrite, Permission, Permissions, PresentMember};
use crate::action::parse_case_marker;
use crate::ids::{AccountId, ChannelId, GuildId, MemberId, RoleId};

/// Discord's channel type for a server text channel.
pub const TEXT_CHANNEL: u8 = 0;

/// One role of a server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoleFacts {
    /// The role.
    pub id: RoleId,
    /// Its name.
    pub name: String,
    /// Its position (@everyone is 0).
    pub position: u32,
    /// Its server-level permissions.
    pub permissions: Permissions,
    /// Whether an integration manages it (it can't be assigned by hand).
    pub managed: bool,
}

/// A server's facts (§2): owner, MFA level, roles, and Quack's own member.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuildFacts {
    /// The server.
    pub guild_id: GuildId,
    /// Its name.
    pub name: String,
    /// Its icon hash, when it has one.
    pub icon_hash: Option<String>,
    /// The owner.
    pub owner_id: AccountId,
    /// Whether the server's MFA level is elevated (§2.2).
    pub mfa_required: bool,
    /// Every role, @everyone included.
    pub roles: Vec<RoleFacts>,
    /// Quack's own member, or `None` when Quack is not in the server.
    pub quack: Option<MemberFacts>,
}

impl GuildFacts {
    /// Looks up a role.
    pub fn role(&self, id: RoleId) -> Option<&RoleFacts> {
        self.roles.iter().find(|r| r.id == id)
    }

    /// Every role id, for dropping configured roles the server no longer has.
    pub fn role_ids(&self) -> Vec<RoleId> {
        self.roles.iter().map(|r| r.id).collect()
    }
}

/// A member of a server: their roles, permissions, membership, and names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberFacts {
    /// The member.
    pub id: MemberId,
    /// Whether the account is a bot (this includes Quack).
    pub bot: bool,
    /// Their role ids (without @everyone).
    pub role_ids: Vec<RoleId>,
    /// Position of their highest role (@everyone is 0).
    pub highest_role_position: u32,
    /// Their computed server-level permissions.
    pub permissions: Permissions,
    /// When this membership began, when Discord says.
    pub joined_at: Option<OffsetDateTime>,
    /// When their current timeout ends (`communication_disabled_until`).
    pub timeout_until: Option<OffsetDateTime>,
    /// Their username.
    pub username: String,
    /// Their global display name.
    pub global_name: Option<String>,
    /// Their nickname in this server.
    pub nick: Option<String>,
}

impl MemberFacts {
    /// Whether they have Administrator.
    pub const fn is_administrator(&self) -> bool {
        self.permissions
            .contains(Permissions(Permission::Administrator.bit()))
    }

    /// The name people see: nickname, else global name, else username.
    pub fn display_name(&self) -> &str {
        self.nick
            .as_deref()
            .or(self.global_name.as_deref())
            .unwrap_or(&self.username)
    }

    /// The role facts the member checks use.
    pub const fn present(&self) -> PresentMember {
        PresentMember {
            highest_role_position: self.highest_role_position,
            permissions: self.permissions,
        }
    }

    /// The member as the subject of a case, present in the server.
    pub const fn subject(&self) -> MemberSubject {
        MemberSubject {
            id: self.id,
            bot: self.bot,
            present: Some(self.present()),
        }
    }
}

/// A channel or thread and what Quack can do there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelFacts {
    /// The channel.
    pub id: ChannelId,
    /// The server it belongs to.
    pub guild_id: GuildId,
    /// Discord's channel type ([`TEXT_CHANNEL`] for a text channel).
    pub kind: u8,
    /// Its name.
    pub name: String,
    /// Its parent: the category, or a thread's channel.
    pub parent_id: Option<ChannelId>,
    /// Its permission overwrites.
    pub overwrites: Vec<Overwrite>,
    /// Quack's computed permissions in it.
    pub quack_permissions: Permissions,
}

impl ChannelFacts {
    /// Whether it is a text channel.
    pub const fn is_text(&self) -> bool {
        self.kind == TEXT_CHANNEL
    }
}

/// A ban as Discord stores it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BanFacts {
    /// The ban's reason, when it has one.
    pub reason: Option<String>,
}

impl BanFacts {
    /// The case number in the reason's case marker, when Quack placed it
    /// ([`parse_case_marker`]).
    pub fn case_marker(&self) -> Option<u64> {
        self.reason.as_deref().and_then(parse_case_marker)
    }
}

/// A Discord account, whether or not it is in a server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountFacts {
    /// The account.
    pub id: AccountId,
    /// Its username.
    pub username: String,
    /// Its global display name.
    pub global_name: Option<String>,
    /// Its avatar hash, when it has one.
    pub avatar_hash: Option<String>,
    /// Whether it is a bot.
    pub bot: bool,
}

impl AccountFacts {
    /// The account as the subject of a case when it is not in the server.
    pub const fn absent_subject(&self) -> MemberSubject {
        MemberSubject {
            id: self.id.member(),
            bot: self.bot,
            present: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member() -> MemberFacts {
        MemberFacts {
            id: MemberId::new(5).unwrap(),
            bot: false,
            role_ids: vec![],
            highest_role_position: 3,
            permissions: Permission::Administrator.into(),
            joined_at: None,
            timeout_until: None,
            username: "user".into(),
            global_name: Some("Global".into()),
            nick: None,
        }
    }

    #[test]
    fn member_facts() {
        let mut m = member();
        assert!(m.is_administrator());
        assert_eq!(m.display_name(), "Global");
        m.nick = Some("Nick".into());
        assert_eq!(m.display_name(), "Nick");
        m.nick = None;
        m.global_name = None;
        assert_eq!(m.display_name(), "user");
        let subject = m.subject();
        assert_eq!(subject.present.map(|p| p.highest_role_position), Some(3));
        m.permissions = Permission::BanMembers.into();
        assert!(!m.is_administrator());
    }

    #[test]
    fn bans_and_channels() {
        let ban = BanFacts {
            reason: Some("Quack case #4: Spam".into()),
        };
        assert_eq!(ban.case_marker(), Some(4));
        assert_eq!(BanFacts { reason: None }.case_marker(), None);
        let account = AccountFacts {
            id: AccountId::new(7).unwrap(),
            username: "a".into(),
            global_name: None,
            avatar_hash: None,
            bot: true,
        };
        assert_eq!(account.absent_subject().present, None);
        assert!(account.absent_subject().bot);
    }
}
