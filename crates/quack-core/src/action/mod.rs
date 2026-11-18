//! Actions (§5): what Quack does on Discord for a case, their states, and
//! the transition table every state change goes through.
//!
//! A case has at most one original action (timeout, kick, ban) and at most
//! one undo action per original (remove timeout, unban). `done`,
//! `cancelled`, and `skipped` are terminal; `failed` is terminal unless
//! staff retry it. Only originals can be `cancelled` and only undos can be
//! `skipped`.

mod controls;
mod marker;
mod run;

use crate::InvalidTransition;
use crate::permissions::DiscordPermission;

pub use controls::{
    ActionView, Controls, StillApplies, VoidEffect, VoidFacts, controls, on_void, undo_needed,
};
pub use marker::{AUDIT_LOG_REASON_MAX_CHARS, case_marker, parse_case_marker, undo_marker};
pub use run::{
    ActionTarget, AttemptResult, CallOutcome, DiscordRefusal, MAX_ATTEMPTS, MEMBER_BUSY_DEFER,
    Plan, RETRY_BACKOFF, SettleFacts, Settlement, StartDecision, StartFacts, after_call,
    after_read_failure, plan_after_marker, plan_from_ban, plan_from_member, retry_delay, settle,
    start, timeout_ends_at, timeout_window_passed,
};

str_enum! {
    /// What an action does on Discord (`action_kind`).
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

    /// Whether this kind is an original action.
    pub const fn is_original(self) -> bool {
        matches!(self.purpose(), ActionPurpose::Original)
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
    pub const fn permission(self) -> DiscordPermission {
        match self {
            Self::Timeout | Self::RemoveTimeout => DiscordPermission::ModerateMembers,
            Self::Kick => DiscordPermission::KickMembers,
            Self::Ban | Self::Unban => DiscordPermission::BanMembers,
        }
    }

    /// Whether an attempt reads the member first (`GET …/members/{m}`);
    /// otherwise it reads the ban (`GET …/bans/{m}`) (§5.1).
    pub const fn reads_member(self) -> bool {
        matches!(self, Self::Timeout | Self::Kick | Self::RemoveTimeout)
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
        /// An original that will not run (again) because its case was voided.
        Cancelled = "cancelled",
        /// An undo that did not change Discord; `skip_reason` says why.
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
    /// Why an action failed (`action_failure`, §5.5).
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
        /// The timeout's end passed before Discord applied it.
        TimeoutWindowPassed = "timeout_window_passed",
        /// Discord refused for another reason.
        DiscordRefused = "discord_refused",
        /// The round's budget ran out and no attempt may have applied:
        /// Discord definitely did not apply it.
        RetriesExhausted = "retries_exhausted",
        /// The round's budget ran out and some attempt may have applied:
        /// Quack can't tell whether Discord applied it.
        Unconfirmed = "unconfirmed",
    }
}

str_enum! {
    /// Why an undo was `skipped` (`action_skip_reason`, §5.6).
    pub enum ActionSkipReason {
        /// Another valid case's timeout or ban is still in force.
        OtherCaseApplies = "other_case_applies",
        /// Discord shows a timeout or ban this case didn't place.
        ChangedOutsideCase = "changed_outside_case",
    }
}

/// A failure with the details its reason carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Failure {
    /// Why it failed.
    pub reason: ActionFailure,
    /// For `quack_missing_permission`: which permission.
    pub permission: Option<DiscordPermission>,
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
    pub const fn missing_permission(permission: DiscordPermission) -> Self {
        Self {
            permission: Some(permission),
            ..Self::new(ActionFailure::QuackMissingPermission)
        }
    }

    /// Discord refused with this status and code.
    pub const fn discord_refused(status: u16, code: Option<u32>) -> Self {
        Self {
            http_status: Some(status),
            discord_code: code,
            ..Self::new(ActionFailure::DiscordRefused)
        }
    }
}

str_enum! {
    /// How one attempt ended (`attempt_outcome`, §5.5).
    pub enum AttemptOutcome {
        /// Discord confirmed the change.
        Done = "done",
        /// The desired state already held, or the undo found Discord
        /// changed outside this case.
        NoChange = "no_change",
        /// Discord definitely refused.
        Refused = "refused",
        /// A read failed, or Discord rate limited before the change call
        /// was sent: Discord changed nothing.
        NotSent = "not_sent",
        /// The change call's result is unknown (deadline, 5xx, reset).
        Unknown = "unknown",
        /// The next run found the attempt still open.
        Interrupted = "interrupted",
    }
}

impl AttemptOutcome {
    /// Whether an attempt that ended this way may have changed Discord:
    /// `done`, `unknown`, or `interrupted`.
    pub const fn may_have_applied(self) -> bool {
        matches!(self, Self::Done | Self::Unknown | Self::Interrupted)
    }
}

/// Whether any of these attempts may have applied (§5.2): an attempt that
/// is still open (`None`, its `finished_at` is NULL) or that ended `done`,
/// `unknown`, or `interrupted`. The void (§4.7) and the exhausted-budget
/// failure reason (§5.5) use this one rule.
pub fn may_have_applied(attempts: &[Option<AttemptOutcome>]) -> bool {
    attempts
        .iter()
        .any(|a| a.is_none_or(AttemptOutcome::may_have_applied))
}

/// An event in the action transition table (§5.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionEvent {
    /// The original's case is voided: at start, or by the void itself.
    CaseVoided,
    /// Start: the undo guard found another valid case's action in force.
    OtherCaseApplies,
    /// Start: the Quack-side pre-check refused, or the timeout end passed.
    StartRefused,
    /// Start: every check passed; the attempt row is open.
    AttemptStarted,
    /// Settle: the open attempt finished.
    Settled {
        /// How it ended.
        outcome: AttemptOutcome,
        /// The undo found the timeout or ban changed outside this case.
        changed_outside_case: bool,
        /// Whether the case is still valid (read under the case lock).
        case_valid: bool,
        /// Whether the round's budget allows another attempt.
        attempts_left: bool,
    },
    /// Staff retried a failed action: the write clears every failure field
    /// (`failure_reason`, `failure_permission`, `failure_http_status`,
    /// `failure_discord_code`) and starts a new round (`attempts = 0`).
    StaffRetry,
    /// Staff dismissed a failed action.
    StaffDismiss,
}

impl ActionEvent {
    /// The event's name, for errors and logs.
    pub const fn name(self) -> &'static str {
        match self {
            Self::CaseVoided => "case_voided",
            Self::OtherCaseApplies => "other_case_applies",
            Self::StartRefused => "start_refused",
            Self::AttemptStarted => "attempt_started",
            Self::Settled { .. } => "settled",
            Self::StaffRetry => "staff_retry",
            Self::StaffDismiss => "staff_dismiss",
        }
    }
}

/// Applies one event to an action of `kind` in `state` (§5.3). Settle rows
/// are tried in the table's order. Any pair the table does not list is an
/// [`InvalidTransition`].
pub fn transition(
    kind: ActionKind,
    state: ActionState,
    event: ActionEvent,
) -> Result<ActionState, InvalidTransition> {
    use ActionEvent as E;
    use ActionState as S;
    use AttemptOutcome as O;
    let original = kind.is_original();
    let invalid = InvalidTransition {
        machine: "action",
        from: state.as_str(),
        event: event.name(),
    };
    let next = match (state, event) {
        (S::Pending, E::CaseVoided) if original => S::Cancelled,
        (S::Pending, E::OtherCaseApplies) if !original => S::Skipped,
        (S::Pending, E::StartRefused) | (S::Failed, E::StaffDismiss) => S::Failed,
        (S::Pending, E::AttemptStarted) => S::Running,
        (
            S::Running,
            E::Settled {
                outcome,
                changed_outside_case,
                case_valid,
                attempts_left,
            },
        ) => match outcome {
            O::Done | O::NoChange if !changed_outside_case => S::Done,
            O::NoChange if !original => S::Skipped,
            _ if changed_outside_case => return Err(invalid),
            O::Refused => S::Failed,
            _ if original && !case_valid => S::Cancelled,
            _ if attempts_left => S::Pending,
            _ => S::Failed,
        },
        (S::Failed, E::StaffRetry) => S::Pending,
        _ => return Err(invalid),
    };
    Ok(next)
}

#[cfg(test)]
mod tests;
