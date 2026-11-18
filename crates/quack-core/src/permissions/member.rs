//! Member checks (§2.3) and the Quack-side action pre-check (§5.4 step 6).

use super::{DiscordPermission, Permission, Permissions};
use crate::action::{ActionFailure, ActionKind, Failure};
use crate::ids::{AccountId, MemberId};

/// Role facts of someone who is in the server.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PresentMember {
    /// Position of their highest role (@everyone is 0).
    pub highest_role_position: u32,
    /// Their computed server-level permissions.
    pub permissions: Permissions,
}

impl PresentMember {
    fn is_administrator(self) -> bool {
        self.permissions.contains(Permission::Administrator.into())
    }
}

/// Quack's own facts in the server: `None` in [`ApplyFacts::quack`] means
/// Quack is not in the server.
pub type QuackFacts = PresentMember;

/// Who is acting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActorPresence {
    /// Quack itself (honeypot cases, action jobs).
    Quack,
    /// A staff member, with their role facts when they are in the server.
    Staff {
        /// Their account.
        id: AccountId,
        /// `None` when they are not in the server.
        member: Option<PresentMember>,
    },
}

/// The member a case is about. When they are not in the server only their
/// account facts are known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemberSubject {
    /// Their account.
    pub id: MemberId,
    /// Whether the account is a bot (this includes Quack).
    pub bot: bool,
    /// Their role facts, or `None` when they are not in the server.
    pub present: Option<PresentMember>,
}

/// Everything the member checks need, gathered from Discord before any
/// write (gateway cache first, REST fallback).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ApplyFacts {
    /// The server owner.
    pub owner_id: AccountId,
    /// Quack's facts, or `None` when Quack is not in the server.
    pub quack: Option<QuackFacts>,
    /// Who is acting.
    pub actor: ActorPresence,
    /// The member the case is about.
    pub member: MemberSubject,
}

/// Why a member check refused. The first failing check wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemberRefusal {
    /// Check 1: Quack is not in the server.
    QuackNotInServer,
    /// Check 2: the actor is not in the server.
    ActorNotInServer,
    /// Check 3: the member is not in the server.
    MemberNotInServer,
    /// Check 4: the member is the actor.
    MemberIsSelf,
    /// Check 5: the member is a bot account.
    MemberIsBot,
    /// Check 6: the member owns the server.
    MemberIsOwner,
    /// Check 7: the member's highest role is at or above the actor's.
    MemberRoleTooHigh,
    /// Check 8: the member's highest role is at or above Quack's.
    QuackRoleTooLow,
    /// Check 9: Quack lacks the permission the action needs.
    QuackMissingPermission(DiscordPermission),
    /// Check 10: Discord never times out a member with Administrator.
    MemberIsAdministrator,
}

str_enum! {
    /// The machine code of a [`MemberRefusal`] (API `member_refused` codes).
    pub enum MemberRefusalCode {
        /// See [`MemberRefusal::QuackNotInServer`].
        QuackNotInServer = "quack_not_in_server",
        /// See [`MemberRefusal::ActorNotInServer`].
        ActorNotInServer = "actor_not_in_server",
        /// See [`MemberRefusal::MemberNotInServer`].
        MemberNotInServer = "member_not_in_server",
        /// See [`MemberRefusal::MemberIsSelf`].
        MemberIsSelf = "member_is_self",
        /// See [`MemberRefusal::MemberIsBot`].
        MemberIsBot = "member_is_bot",
        /// See [`MemberRefusal::MemberIsOwner`].
        MemberIsOwner = "member_is_owner",
        /// See [`MemberRefusal::MemberRoleTooHigh`].
        MemberRoleTooHigh = "member_role_too_high",
        /// See [`MemberRefusal::QuackRoleTooLow`].
        QuackRoleTooLow = "quack_role_too_low",
        /// See [`MemberRefusal::QuackMissingPermission`].
        QuackMissingPermission = "quack_missing_permission",
        /// See [`MemberRefusal::MemberIsAdministrator`].
        MemberIsAdministrator = "member_is_administrator",
    }
}

impl MemberRefusal {
    /// The refusal's machine code.
    pub const fn code(self) -> MemberRefusalCode {
        use MemberRefusalCode as C;
        match self {
            Self::QuackNotInServer => C::QuackNotInServer,
            Self::ActorNotInServer => C::ActorNotInServer,
            Self::MemberNotInServer => C::MemberNotInServer,
            Self::MemberIsSelf => C::MemberIsSelf,
            Self::MemberIsBot => C::MemberIsBot,
            Self::MemberIsOwner => C::MemberIsOwner,
            Self::MemberRoleTooHigh => C::MemberRoleTooHigh,
            Self::QuackRoleTooLow => C::QuackRoleTooLow,
            Self::QuackMissingPermission(_) => C::QuackMissingPermission,
            Self::MemberIsAdministrator => C::MemberIsAdministrator,
        }
    }

    /// Whether Quack's own access caused it, so an administrator must fix
    /// it and it is recorded as `case_create_refused`.
    pub const fn is_quack_access(self) -> bool {
        matches!(
            self,
            Self::QuackNotInServer | Self::QuackRoleTooLow | Self::QuackMissingPermission(_)
        )
    }

    /// The permission Quack lacks, for check 9.
    pub const fn permission(self) -> Option<DiscordPermission> {
        match self {
            Self::QuackMissingPermission(p) => Some(p),
            _ => None,
        }
    }
}

/// Which operation the checks are for; each runs a different subset (§2.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckPurpose {
    /// A moderator applies a rule: checks 1–8, the member must be present.
    ApplyRule,
    /// Quack applies the honeypot rule: checks 1, 3, 5, 6, 8.
    ApplyHoneypotRule,
    /// A moderator retries a failed action (§5.8). An original: checks 2,
    /// 4, 6, 7, and 3 for a timeout. A remove timeout: check 7 when the
    /// member is in the server. An unban: none (the member is usually
    /// absent). Quack's own checks run later in the action job.
    RetryAction(ActionKind),
    /// A moderator asks for an undo of this kind by hand (§5.8): a remove
    /// timeout runs check 7 when the member is in the server; an unban runs
    /// none.
    RequestUndo(ActionKind),
}

impl CheckPurpose {
    /// Whether member check `check` (1–8) runs for this purpose.
    const fn runs(self, check: u8) -> bool {
        match self {
            Self::ApplyRule => true,
            Self::ApplyHoneypotRule => matches!(check, 1 | 3 | 5 | 6 | 8),
            Self::RetryAction(kind) if kind.is_original() => {
                matches!(check, 2 | 4 | 6 | 7)
                    || (check == 3 && matches!(kind, ActionKind::Timeout))
            }
            Self::RetryAction(kind) | Self::RequestUndo(kind) => {
                check == 7 && matches!(kind, ActionKind::RemoveTimeout)
            }
        }
    }

    /// Whether any member check runs, so the caller needs Discord's facts
    /// at all (an unban's retry or request needs none).
    pub const fn needs_facts(self) -> bool {
        let mut check = 1;
        while check <= 8 {
            if self.runs(check) {
                return true;
            }
            check += 1;
        }
        false
    }
}

/// Runs member checks 1–8 for `purpose`, in order; the first failure wins.
///
/// Checks 9 and 10 depend on the chosen level and run afterwards through
/// [`check_action_permission`]. Checks 4–8 use only account facts and skip
/// role positions when the member is not in the server.
pub fn check_member(facts: &ApplyFacts, purpose: CheckPurpose) -> Result<(), MemberRefusal> {
    let runs = |check: u8| purpose.runs(check);
    let member = facts.member;
    let (actor_id, actor_member) = match facts.actor {
        ActorPresence::Quack => (None, None),
        ActorPresence::Staff { id, member } => (Some(id), member),
    };
    if runs(1) && facts.quack.is_none() {
        return Err(MemberRefusal::QuackNotInServer);
    }
    if runs(2) && actor_id.is_some() && actor_member.is_none() {
        return Err(MemberRefusal::ActorNotInServer);
    }
    if runs(3) && member.present.is_none() {
        return Err(MemberRefusal::MemberNotInServer);
    }
    if runs(4) && actor_id == Some(member.id.account()) {
        return Err(MemberRefusal::MemberIsSelf);
    }
    if runs(5) && member.bot {
        return Err(MemberRefusal::MemberIsBot);
    }
    if runs(6) && member.id.account() == facts.owner_id {
        return Err(MemberRefusal::MemberIsOwner);
    }
    if runs(7)
        && actor_id != Some(facts.owner_id)
        && let (Some(actor), Some(target)) = (actor_member, member.present)
        && target.highest_role_position >= actor.highest_role_position
    {
        return Err(MemberRefusal::MemberRoleTooHigh);
    }
    if runs(8)
        && let (Some(quack), Some(target)) = (facts.quack, member.present)
        && target.highest_role_position >= quack.highest_role_position
    {
        return Err(MemberRefusal::QuackRoleTooLow);
    }
    Ok(())
}

/// Member checks 9 and 10, run once the level is chosen and it has an
/// action: Quack holds the action's permission (Administrator satisfies
/// all), and a timeout's member does not have Administrator.
pub fn check_action_permission(facts: &ApplyFacts, kind: ActionKind) -> Result<(), MemberRefusal> {
    let quack = facts.quack.ok_or(MemberRefusal::QuackNotInServer)?;
    let needed = kind.permission();
    if !quack.permissions.allows(needed.permission()) {
        return Err(MemberRefusal::QuackMissingPermission(needed));
    }
    if kind == ActionKind::Timeout
        && facts
            .member
            .present
            .is_some_and(PresentMember::is_administrator)
    {
        return Err(MemberRefusal::MemberIsAdministrator);
    }
    Ok(())
}

/// The Quack-side pre-check an action's start runs before calling Discord
/// (§5.4 step 1.8). A refusal fails the action without calling Discord.
///
/// Order: Quack in the server, Quack holds the permission, the member is
/// not the owner, and, when the member is in the server, their highest
/// role is below Quack's (not checked for `unban`). Whether a timeout's
/// member is present or an administrator comes from the attempt's own read
/// (§5.1).
pub fn action_precheck(facts: &ApplyFacts, kind: ActionKind) -> Result<(), Failure> {
    let Some(quack) = facts.quack else {
        return Err(Failure::new(ActionFailure::QuackNotInServer));
    };
    let needed = kind.permission();
    if !quack.permissions.allows(needed.permission()) {
        return Err(Failure::missing_permission(needed));
    }
    if facts.member.id.account() == facts.owner_id {
        return Err(Failure::new(ActionFailure::MemberIsOwner));
    }
    if kind != ActionKind::Unban
        && let Some(member) = facts.member.present
        && member.highest_role_position >= quack.highest_role_position
    {
        return Err(Failure::new(ActionFailure::QuackRoleTooLow));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
