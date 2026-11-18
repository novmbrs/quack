//! Capabilities (§2.1), the 2FA rule (§2.2), and the operation table (§2.4).

use super::{Permission, Permissions};
use crate::DomainError;
use crate::ids::RoleId;

str_enum! {
    /// A staff capability. Nothing but [`derive_capabilities`] grants one.
    pub enum Capability {
        /// Apply rules, read and void cases, work actions, review appeals,
        /// read the moderation log and statistics, work tickets.
        Moderate = "moderate",
        /// Create, edit, archive, restore, import, and export rules.
        ManageRules = "manage_rules",
        /// Read and change settings (except moderator roles), guided setup,
        /// modules, server health.
        ManageSettings = "manage_settings",
        /// Change the moderator roles.
        ManageStaffRoles = "manage_staff_roles",
    }
}

/// Facts about one account in one server, read fresh from Discord for every
/// staff operation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ActorFacts {
    /// Whether they own the server.
    pub is_owner: bool,
    /// Their computed server-level permissions.
    pub permissions: Permissions,
    /// Their role ids.
    pub role_ids: Vec<RoleId>,
    /// Whether Quack recorded 2FA on their account at their last dashboard
    /// sign-in. An account never seen is `false`.
    pub mfa_enabled: bool,
}

impl ActorFacts {
    fn is_administrator(&self) -> bool {
        self.is_owner || self.permissions.contains(Permission::Administrator.into())
    }

    fn holds_any(&self, roles: &[RoleId]) -> bool {
        self.role_ids.iter().any(|r| roles.contains(r))
    }
}

/// The server's configured staff role lists.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StaffRoles {
    /// Roles that grant `moderate`.
    pub moderator_role_ids: Vec<RoleId>,
    /// Roles that grant `manage_rules`.
    pub rules_manager_role_ids: Vec<RoleId>,
}

impl StaffRoles {
    /// Drops configured roles the server no longer has, as capability
    /// derivation requires (saved ids are kept in settings; they are just
    /// ignored while deleted).
    #[must_use]
    pub fn existing(mut self, server_role_ids: &[RoleId]) -> Self {
        self.moderator_role_ids
            .retain(|r| server_role_ids.contains(r));
        self.rules_manager_role_ids
            .retain(|r| server_role_ids.contains(r));
        self
    }
}

/// The capabilities one actor holds in one server, plus whether the 2FA
/// rule lets them use any of them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "one flag per capability, matching the API shape"
)]
pub struct Capabilities {
    /// Holds `moderate`.
    pub moderate: bool,
    /// Holds `manage_rules`.
    pub manage_rules: bool,
    /// Holds `manage_settings`.
    pub manage_settings: bool,
    /// Holds `manage_staff_roles`.
    pub manage_staff_roles: bool,
    /// Whether the 2FA rule is satisfied: the server does not require MFA,
    /// or Quack recorded 2FA on the actor's account.
    pub mfa_ok: bool,
}

/// Why an actor may not use a capability.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AccessRefusal {
    /// They don't hold it.
    #[error("missing capability {0:?}")]
    MissingCapability(Capability),
    /// They hold it, but the server requires 2FA Quack has not recorded.
    #[error("two-factor authentication required")]
    MfaRequired,
}

impl From<AccessRefusal> for DomainError {
    fn from(r: AccessRefusal) -> Self {
        match r {
            AccessRefusal::MissingCapability(c) => Self::MissingCapability(c),
            AccessRefusal::MfaRequired => Self::MfaRequired,
        }
    }
}

impl Capabilities {
    /// Whether the actor holds `cap`, ignoring the 2FA rule (for showing
    /// what they could do once 2FA is on).
    pub const fn holds(self, cap: Capability) -> bool {
        match cap {
            Capability::Moderate => self.moderate,
            Capability::ManageRules => self.manage_rules,
            Capability::ManageSettings => self.manage_settings,
            Capability::ManageStaffRoles => self.manage_staff_roles,
        }
    }

    /// Allows `cap` only when it is held and the 2FA rule is satisfied.
    /// A missing capability is reported before missing 2FA.
    pub const fn check(self, cap: Capability) -> Result<(), AccessRefusal> {
        if !self.holds(cap) {
            Err(AccessRefusal::MissingCapability(cap))
        } else if !self.mfa_ok {
            Err(AccessRefusal::MfaRequired)
        } else {
            Ok(())
        }
    }
}

/// Derives an actor's capabilities (§2.1).
///
/// `roles` must already exclude roles the server no longer has
/// ([`StaffRoles::existing`]). When no configured moderator role exists,
/// Moderate Members grants `moderate` instead, so deleting roles never locks
/// a server out.
pub fn derive_capabilities(
    actor: &ActorFacts,
    roles: &StaffRoles,
    mfa_required: bool,
) -> Capabilities {
    let admin = actor.is_administrator();
    let manage_guild = admin || actor.permissions.contains(Permission::ManageGuild.into());
    let moderate = admin
        || if roles.moderator_role_ids.is_empty() {
            actor
                .permissions
                .contains(Permission::ModerateMembers.into())
        } else {
            actor.holds_any(&roles.moderator_role_ids)
        };
    Capabilities {
        moderate,
        manage_rules: manage_guild || actor.holds_any(&roles.rules_manager_role_ids),
        manage_settings: manage_guild,
        manage_staff_roles: admin,
        mfa_ok: !mfa_required || actor.mfa_enabled,
    }
}

/// The broader "is staff for Discord access" predicate used for staff-only
/// channels, ticket threads, and the honeypot exemption (§2.1 rule 3):
/// owner, Administrator, Moderate Members, or any configured moderator
/// role. It has no 2FA gate and ignores Manage Server and rules managers.
pub fn is_staff_for_access(actor: &ActorFacts, moderator_role_ids: &[RoleId]) -> bool {
    actor.is_administrator()
        || actor
            .permissions
            .contains(Permission::ModerateMembers.into())
        || actor.holds_any(moderator_role_ids)
}

/// Every operation in the operations table (§2.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Operation {
    /// Apply a rule (`/case add`, "Add case", dashboard).
    ApplyRule,
    /// Quack applies the honeypot rule.
    ApplyHoneypotRule,
    /// View a case, list cases, member history, evidence.
    ViewCases,
    /// Edit case context or add evidence.
    EditCase,
    /// Void a case.
    VoidCase,
    /// Retry a failed action (original or undo).
    RetryAction,
    /// Dismiss a failed action.
    DismissAction,
    /// Undo a done timeout or ban by hand.
    UndoAction,
    /// Accept, reject, request information, close, or reopen an appeal.
    DecideAppeal,
    /// Submit an appeal or reply with information.
    SubmitAppeal,
    /// Create, edit, archive, restore, import, or export a rule.
    ManageRules,
    /// Read settings.
    ReadSettings,
    /// Change settings other than the moderator roles.
    ChangeSettings,
    /// Change `moderator_role_ids`.
    ChangeModeratorRoles,
    /// Guided `/setup`, module settings, enable or disable a module.
    Setup,
    /// Read server health.
    ServerHealth,
    /// Read the moderation log or statistics.
    ReadModLog,
    /// Open one's own ticket.
    OpenOwnTicket,
    /// Close a ticket.
    CloseTicket,
    /// Fix a ticket's thread access.
    FixTicketAccess,
}

/// What an operation requires of whoever performs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Requirement {
    /// A staff capability plus the 2FA rule.
    Capability(Capability),
    /// Being the member the case is about (no 2FA rule).
    MemberSelf,
    /// Nothing: Quack itself acts.
    Quack,
    /// Being a member of the server.
    ServerMember,
    /// Being the ticket's member, or `moderate` plus the 2FA rule.
    TicketMemberOrModerator,
}

impl Operation {
    /// What the operation requires (§2.4). Extra checks (member checks,
    /// state) are applied by the operation itself.
    pub const fn requirement(self) -> Requirement {
        use Capability as C;
        match self {
            Self::ApplyRule
            | Self::ViewCases
            | Self::EditCase
            | Self::VoidCase
            | Self::RetryAction
            | Self::DismissAction
            | Self::UndoAction
            | Self::DecideAppeal
            | Self::ReadModLog
            | Self::FixTicketAccess => Requirement::Capability(C::Moderate),
            Self::ApplyHoneypotRule => Requirement::Quack,
            Self::SubmitAppeal => Requirement::MemberSelf,
            Self::ManageRules => Requirement::Capability(C::ManageRules),
            Self::ReadSettings | Self::ChangeSettings | Self::Setup | Self::ServerHealth => {
                Requirement::Capability(C::ManageSettings)
            }
            Self::ChangeModeratorRoles => Requirement::Capability(C::ManageStaffRoles),
            Self::OpenOwnTicket => Requirement::ServerMember,
            Self::CloseTicket => Requirement::TicketMemberOrModerator,
        }
    }
}

#[cfg(test)]
mod tests;
