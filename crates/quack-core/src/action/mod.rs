//! Actions (§5): what Quack does on Discord for a case, their states, and
//! the transition table every state change goes through.
//!
//! A case has at most one original action (timeout, kick, ban) and at most
//! one undo action per original (remove timeout, unban). `done`,
//! `cancelled`, and `skipped` are terminal; `failed` is terminal unless
//! staff retry it.

mod controls;
mod run;

use crate::InvalidTransition;
use crate::permissions::Permission;

pub use controls::{
    GuardCandidate, StillApplies, VoidEffect, can_dismiss, can_retry, can_undo, on_void,
    undo_guard, undo_needed,
};
pub use run::{
    AttemptContext, AttemptResult, ClaimDecision, ClaimFacts, DiscordRefusal, MAX_ATTEMPTS,
    RETRY_BACKOFF, Settlement, TIMEOUT_END_MARGIN, before_attempt, retry_delay, settle,
    timeout_ends_at, timeout_window_passed,
};

str_enum! {
    /// What an action does on Discord.
    pub enum ActionKind {
        /// Time the member out until the case's fixed end time.
        Timeout = "timeout",
        /// Remove the member from the server.
        Kick = "kick",
        /// Ban the member.
        Ban = "ban",
        /// Undo a timeout.
        RemoveTimeout = "remove_timeout",
        /// Undo a ban.
        Unban = "unban",
    }
}

/// Whether an action is a case's original action or the undo of one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionPurpose {
    /// The level's timeout, kick, or ban.
    Original,
    /// Remove timeout or unban, undoing an original.
    Undo,
}

impl ActionKind {
    /// Whether this kind is an original or an undo.
    pub const fn purpose(self) -> ActionPurpose {
        match self {
            Self::Timeout | Self::Kick | Self::Ban => ActionPurpose::Original,
            Self::RemoveTimeout | Self::Unban => ActionPurpose::Undo,
        }
    }

    /// The undo kind for an original, if it has one (a kick has none).
    pub const fn undo_kind(self) -> Option<Self> {
        match self {
            Self::Timeout => Some(Self::RemoveTimeout),
            Self::Ban => Some(Self::Unban),
            _ => None,
        }
    }

    /// The original kind an undo reverses.
    pub const fn undoes(self) -> Option<Self> {
        match self {
            Self::RemoveTimeout => Some(Self::Timeout),
            Self::Unban => Some(Self::Ban),
            _ => None,
        }
    }

    /// The Discord permission Quack needs for this kind.
    pub const fn permission(self) -> Permission {
        match self {
            Self::Timeout | Self::RemoveTimeout => Permission::ModerateMembers,
            Self::Kick => Permission::KickMembers,
            Self::Ban | Self::Unban => Permission::BanMembers,
        }
    }
}

str_enum! {
    /// An action's stored state (`action_state`).
    pub enum ActionState {
        /// Waiting for its first attempt or its next retry. Outcome: in progress.
        Pending = "pending",
        /// An attempt started and has not settled. Outcome: in progress.
        Running = "running",
        /// Discord confirmed it, or the desired state already held.
        Done = "done",
        /// A definite refusal, or the retry budget ran out.
        Failed = "failed",
        /// An original whose case was voided before it ran.
        Cancelled = "cancelled",
        /// An undo left in place because another valid case still applies.
        Skipped = "skipped",
    }
}

impl ActionState {
    /// Whether nothing can move the action on (`failed` can still be
    /// retried by staff, so it is not terminal here).
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Cancelled | Self::Skipped)
    }
}

str_enum! {
    /// Why an action failed (`action_failure`).
    pub enum ActionFailure {
        /// Quack is not in the server (pre-check, or Unknown Guild).
        QuackNotInServer = "quack_not_in_server",
        /// Quack lacks the permission (pre-check).
        QuackMissingPermission = "quack_missing_permission",
        /// The member's highest role is at or above Quack's (pre-check).
        QuackRoleTooLow = "quack_role_too_low",
        /// The member owns the server (pre-check).
        MemberIsOwner = "member_is_owner",
        /// A timeout's member is not in the server.
        MemberNotInServer = "member_not_in_server",
        /// A timeout's member has Administrator.
        MemberIsAdministrator = "member_is_administrator",
        /// The timeout's fixed end has passed.
        TimeoutWindowPassed = "timeout_window_passed",
        /// Discord refused for another reason.
        DiscordRefused = "discord_refused",
        /// The retry budget ran out on uncertain attempts.
        RetriesExhausted = "retries_exhausted",
    }
}

/// A failure with the details its reason carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Failure {
    /// Why it failed.
    pub reason: ActionFailure,
    /// For `quack_missing_permission`: which permission.
    pub permission: Option<Permission>,
    /// For `discord_refused`: Discord's HTTP status.
    pub http_status: Option<u16>,
    /// For `discord_refused`: Discord's JSON error code, when it sent one.
    pub discord_code: Option<u32>,
}

impl Failure {
    /// A failure with no details.
    pub const fn new(reason: ActionFailure) -> Self {
        Self {
            reason,
            permission: None,
            http_status: None,
            discord_code: None,
        }
    }

    /// Quack lacks `permission`.
    pub const fn missing_permission(permission: Permission) -> Self {
        Self {
            permission: Some(permission),
            ..Self::new(ActionFailure::QuackMissingPermission)
        }
    }
}

str_enum! {
    /// How one attempt ended (`attempt_outcome`).
    pub enum AttemptOutcome {
        /// Discord confirmed the change.
        Done = "done",
        /// Discord said the desired state already held.
        NoChange = "no_change",
        /// Discord definitely refused.
        Refused = "refused",
        /// Rate limited before anything happened.
        RateLimited = "rate_limited",
        /// The result is unknown (timeout, 5xx, connection reset).
        Unknown = "unknown",
        /// The worker crashed mid-attempt.
        Interrupted = "interrupted",
    }
}

/// An event in the action transition table (§5.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionEvent {
    /// The job claimed it, every check passed, and the attempt is recorded.
    AttemptStarted,
    /// The case is voided before the action started (on claim or by §4.7).
    CaseVoided,
    /// The undo guard found another valid case that still applies.
    UndoSkipped,
    /// The Quack-side pre-check refused, or a timeout's window passed.
    PrecheckFailed,
    /// An attempt finished.
    AttemptFinished {
        /// How it ended.
        outcome: AttemptOutcome,
        /// Whether the case is still valid.
        case_valid: bool,
        /// Whether the job's retry budget allows another attempt.
        attempts_left: bool,
    },
    /// Staff retried a failed action.
    StaffRetry,
    /// Staff dismissed a failed action.
    StaffDismiss,
}

impl ActionEvent {
    /// The event's name, for errors and logs.
    pub const fn name(self) -> &'static str {
        match self {
            Self::AttemptStarted => "attempt_started",
            Self::CaseVoided => "case_voided",
            Self::UndoSkipped => "undo_skipped",
            Self::PrecheckFailed => "precheck_failed",
            Self::AttemptFinished { .. } => "attempt_finished",
            Self::StaffRetry => "staff_retry",
            Self::StaffDismiss => "staff_dismiss",
        }
    }
}

/// Applies one event to an action of `kind` in `state` (§5.3). Any pair
/// the table does not list is an [`InvalidTransition`].
pub fn transition(
    kind: ActionKind,
    state: ActionState,
    event: ActionEvent,
) -> Result<ActionState, InvalidTransition> {
    use ActionEvent as E;
    use ActionState as S;
    let original = kind.purpose() == ActionPurpose::Original;
    let next = match (state, event) {
        (S::Pending, E::AttemptStarted) => S::Running,
        (S::Pending, E::CaseVoided) if original => S::Cancelled,
        (S::Pending, E::UndoSkipped) if !original => S::Skipped,
        (S::Failed, E::StaffRetry) => S::Pending,
        (S::Pending, E::PrecheckFailed) | (S::Failed, E::StaffDismiss) => S::Failed,
        (
            S::Running,
            E::AttemptFinished {
                outcome,
                case_valid,
                attempts_left,
            },
        ) => match outcome {
            AttemptOutcome::Done | AttemptOutcome::NoChange => S::Done,
            _ if original && !case_valid => S::Cancelled,
            AttemptOutcome::Refused => S::Failed,
            _ if attempts_left => S::Pending,
            _ => S::Failed,
        },
        _ => {
            return Err(InvalidTransition {
                machine: "action",
                from: state.as_str(),
                event: event.name(),
            });
        }
    };
    Ok(next)
}

#[cfg(test)]
mod tests;
