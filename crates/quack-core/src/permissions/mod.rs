//! Who may do what (§2): Discord permission bits, capabilities derived from
//! facts, the operation table, member checks, and the staff-only channel
//! check (§13.1.3).
//!
//! Everything here is pure over facts the caller gathered from Discord.
//! Nothing is cached as a grant: callers re-gather facts for every staff
//! operation.

mod capabilities;
mod channel;
mod compute;
mod facts;
mod member;

use std::ops::BitOr;

pub use capabilities::{
    AccessRefusal, ActorFacts, Capabilities, Capability, Operation, Requirement, StaffRoles,
    derive_capabilities, is_staff_for_access,
};
pub use channel::{
    ChannelProblem, Overwrite, OverwriteTarget, StaffChannelFacts, check_staff_channel,
};
pub use compute::{ALL, Subject, in_channel, in_guild};
pub use facts::{
    AccountFacts, BanFacts, ChannelFacts, GuildFacts, MemberFacts, RoleFacts, TEXT_CHANNEL,
};
pub use member::{
    ActorPresence, ApplyFacts, CheckPurpose, MemberRefusal, MemberRefusalCode, MemberSubject,
    PresentMember, QuackFacts, action_precheck, check_action_permission, check_member,
};

/// A set of Discord permission bits, as Discord computes them for a member
/// in a server or channel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Permissions(pub u64);

impl Permissions {
    /// No permissions.
    pub const NONE: Self = Self(0);

    /// Whether every bit of `other` is set.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether `permission` is granted, counting Administrator as granting
    /// everything (as Discord does).
    pub const fn allows(self, permission: Permission) -> bool {
        self.0 & Permission::Administrator.bit() != 0 || self.0 & permission.bit() != 0
    }

    /// The named permissions in `needed` that this set does not grant, in
    /// [`Permission::ALL`] order.
    pub fn missing(self, needed: Self) -> Vec<Permission> {
        Permission::ALL
            .iter()
            .copied()
            .filter(|p| needed.0 & p.bit() != 0 && !self.allows(*p))
            .collect()
    }
}

impl BitOr for Permissions {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl From<Permission> for Permissions {
    fn from(p: Permission) -> Self {
        Self(p.bit())
    }
}

impl FromIterator<Permission> for Permissions {
    fn from_iter<I: IntoIterator<Item = Permission>>(iter: I) -> Self {
        Self(iter.into_iter().fold(0, |acc, p| acc | p.bit()))
    }
}

str_enum! {
    /// A permission an action needs (`discord_permission`): what a
    /// `quack_missing_permission` failure or refusal names.
    pub enum DiscordPermission {
        /// Moderate Members (timeout, remove timeout).
        ModerateMembers = "moderate_members",
        /// Kick Members.
        KickMembers = "kick_members",
        /// Ban Members (ban, unban).
        BanMembers = "ban_members",
    }
}

impl DiscordPermission {
    /// The same permission among every permission Quack checks.
    pub const fn permission(self) -> Permission {
        match self {
            Self::ModerateMembers => Permission::ModerateMembers,
            Self::KickMembers => Permission::KickMembers,
            Self::BanMembers => Permission::BanMembers,
        }
    }
}

impl From<DiscordPermission> for Permission {
    fn from(p: DiscordPermission) -> Self {
        p.permission()
    }
}

str_enum! {
    /// A Discord permission Quack checks or asks for.
    pub enum Permission {
        /// Kick Members.
        KickMembers = "kick_members",
        /// Ban Members.
        BanMembers = "ban_members",
        /// Administrator (grants every permission).
        Administrator = "administrator",
        /// Manage Channels.
        ManageChannels = "manage_channels",
        /// Manage Server.
        ManageGuild = "manage_guild",
        /// View Audit Log.
        ViewAuditLog = "view_audit_log",
        /// View Channel.
        ViewChannel = "view_channel",
        /// Send Messages.
        SendMessages = "send_messages",
        /// Manage Messages.
        ManageMessages = "manage_messages",
        /// Embed Links.
        EmbedLinks = "embed_links",
        /// Attach Files.
        AttachFiles = "attach_files",
        /// Read Message History.
        ReadMessageHistory = "read_message_history",
        /// Manage Threads.
        ManageThreads = "manage_threads",
        /// Create Public Threads.
        CreatePublicThreads = "create_public_threads",
        /// Create Private Threads.
        CreatePrivateThreads = "create_private_threads",
        /// Send Messages in Threads.
        SendMessagesInThreads = "send_messages_in_threads",
        /// Moderate Members (timeouts).
        ModerateMembers = "moderate_members",
    }
}

impl Permission {
    /// The permission's bit in Discord's permission integer.
    pub const fn bit(self) -> u64 {
        1 << match self {
            Self::KickMembers => 1,
            Self::BanMembers => 2,
            Self::Administrator => 3,
            Self::ManageChannels => 4,
            Self::ManageGuild => 5,
            Self::ViewAuditLog => 7,
            Self::ViewChannel => 10,
            Self::SendMessages => 11,
            Self::ManageMessages => 13,
            Self::EmbedLinks => 14,
            Self::AttachFiles => 15,
            Self::ReadMessageHistory => 16,
            Self::ManageThreads => 34,
            Self::CreatePublicThreads => 35,
            Self::CreatePrivateThreads => 36,
            Self::SendMessagesInThreads => 38,
            Self::ModerateMembers => 40,
        }
    }
}

impl BitOr for Permission {
    type Output = Permissions;

    fn bitor(self, rhs: Self) -> Permissions {
        Permissions(self.bit() | rhs.bit())
    }
}

impl BitOr<Permission> for Permissions {
    type Output = Self;

    fn bitor(self, rhs: Permission) -> Self {
        Self(self.0 | rhs.bit())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits_match_discord() {
        assert_eq!(Permission::Administrator.bit(), 0x8);
        assert_eq!(Permission::ModerateMembers.bit(), 0x100_0000_0000);
        assert_eq!(Permission::ViewChannel.bit(), 0x400);
        let all: Permissions = Permission::ALL.iter().copied().collect();
        assert_eq!(all.0.count_ones() as usize, Permission::ALL.len());
    }

    #[test]
    fn action_permissions_are_the_database_enum() {
        let names: Vec<&str> = DiscordPermission::ALL.iter().map(|p| p.as_str()).collect();
        assert_eq!(names, ["moderate_members", "kick_members", "ban_members"]);
        for p in DiscordPermission::ALL {
            assert_eq!(p.permission().as_str(), p.as_str());
        }
    }

    #[test]
    fn administrator_grants_everything() {
        let admin = Permissions::from(Permission::Administrator);
        assert!(admin.allows(Permission::BanMembers));
        assert_eq!(
            admin.missing(Permission::ViewChannel | Permission::AttachFiles),
            vec![]
        );
        let some = Permissions::from(Permission::ViewChannel);
        assert_eq!(
            some.missing(Permission::ViewChannel | Permission::AttachFiles),
            vec![Permission::AttachFiles]
        );
        assert!(some.contains(Permission::ViewChannel.into()));
        assert!(!Permissions::NONE.contains(Permission::ViewChannel.into()));
    }
}
