//! Staff controls (§5.8), the undo guard (§5.6), and what a void does to
//! each action (§4.7, §5.9).

use time::OffsetDateTime;

use super::{ActionFailure, ActionKind, ActionState, AttemptOutcome, may_have_applied};
use crate::DomainError;
use crate::ids::CaseId;

/// Whether an original of `kind` still has something to undo: a ban
/// always, a timeout while its end is in the future, a kick never.
pub fn undo_needed(
    kind: ActionKind,
    timeout_ends_at: Option<OffsetDateTime>,
    now: OffsetDateTime,
) -> bool {
    match kind {
        ActionKind::Ban => true,
        ActionKind::Timeout => timeout_ends_at.is_some_and(|end| end > now),
        _ => false,
    }
}

/// An action as the staff controls see it, read inside the write's
/// transaction after locking the case and the action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActionView {
    /// The action's kind.
    pub kind: ActionKind,
    /// Its state.
    pub state: ActionState,
    /// Whether staff dismissed it.
    pub dismissed: bool,
    /// Why it failed, when it did.
    pub failure: Option<ActionFailure>,
    /// For a timeout (or its removal): the case's fixed end.
    pub timeout_ends_at: Option<OffsetDateTime>,
    /// For an original: whether an undo of it exists.
    pub has_undo: bool,
    /// For a done original timeout or ban: the undo guard's answer (§5.6)
    /// for its undo, read like the undo's own start would. `Some` means
    /// the undo would be skipped, so it is not offered.
    pub undo_guard: Option<StillApplies>,
}

/// Which controls an action offers, each with the refusal a write gets
/// when it is not allowed (§5.8). Buttons are rendered for the `Ok` ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Controls {
    /// Retry: a new round (`pending` + job).
    pub retry: Result<(), DomainError>,
    /// Dismiss: leaves the failed-actions list; history is kept.
    pub dismiss: Result<(), DomainError>,
    /// Undo: the undo kind to queue.
    pub undo: Result<ActionKind, DomainError>,
}

impl Controls {
    /// Whether the Retry button shows.
    pub const fn can_retry(&self) -> bool {
        self.retry.is_ok()
    }

    /// Whether the Dismiss button shows.
    pub const fn can_dismiss(&self) -> bool {
        self.dismiss.is_ok()
    }

    /// Whether the Undo button shows.
    pub const fn can_undo(&self) -> bool {
        self.undo.is_ok()
    }
}

/// The one eligibility function for staff controls (§5.8): card and
/// log-channel buttons are rendered from it, and every write re-evaluates
/// it inside its transaction. Actor checks (fresh member facts) run
/// separately.
///
/// - Retry: `failed` and not dismissed. An original also needs a valid case
///   (else `case_voided`), a reason other than `timeout_window_passed`, and
///   for a timeout an end in the future (else `timeout_window_passed`). An
///   undo can always be retried (its case is voided by definition).
/// - Dismiss: `failed` and not dismissed.
/// - Undo: a `done` timeout or ban with no undo yet, that the undo guard
///   would not skip (no other valid case's action of that kind is in
///   force); a timeout's end must be in the future. What is offered is
///   what runs: the undo's provenance check accepts its own case's ban
///   marker even while the case is valid.
pub fn controls(action: &ActionView, case_valid: bool, now: OffsetDateTime) -> Controls {
    let failed_open = action.state == ActionState::Failed && !action.dismissed;
    let retry = if !failed_open {
        Err(DomainError::ActionNotRetryable)
    } else if !action.kind.is_original() {
        Ok(())
    } else if !case_valid {
        Err(DomainError::CaseVoided)
    } else if action.failure == Some(ActionFailure::TimeoutWindowPassed)
        || (action.kind == ActionKind::Timeout
            && action.timeout_ends_at.is_none_or(|end| end <= now))
    {
        Err(DomainError::TimeoutWindowPassed)
    } else {
        Ok(())
    };
    let dismiss = if failed_open {
        Ok(())
    } else {
        Err(DomainError::ActionNotDismissable)
    };
    let undo = match action.kind.undo_kind() {
        Some(undo)
            if action.state == ActionState::Done
                && !action.has_undo
                && action.undo_guard.is_none()
                && undo_needed(action.kind, action.timeout_ends_at, now) =>
        {
            Ok(undo)
        }
        _ => Err(DomainError::ActionNotUndoable),
    };
    Controls {
        retry,
        dismiss,
        undo,
    }
}

/// What the void transaction knows about one of the case's actions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VoidFacts<'a> {
    /// The action's kind.
    pub kind: ActionKind,
    /// Its state.
    pub state: ActionState,
    /// For an original: whether an undo of it exists.
    pub has_undo: bool,
    /// Every attempt of the action (all rounds); `None` while open.
    pub attempts: &'a [Option<AttemptOutcome>],
    /// For a timeout: its fixed end.
    pub timeout_ends_at: Option<OffsetDateTime>,
    /// The current time.
    pub now: OffsetDateTime,
}

/// What a void does to one action (§4.7 step 2.4–2.5, §5.9).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VoidEffect {
    /// A `pending` original becomes `cancelled`. A `running` one is left to
    /// settle (then `done`, or `cancelled`).
    pub cancel: bool,
    /// Insert an undo action of this kind and its job.
    pub queue_undo: Option<ActionKind>,
}

/// Decides what voiding the case does to one of its actions. Undo actions
/// are never touched. An original timeout or ban gets its undo queued when
/// none exists, at least one attempt may have applied (§5.2), and, for a
/// timeout, its end is still in the future; a `cancelled` original never
/// does.
///
/// A `done` ban gets its undo even when every attempt was `no_change`: the
/// ban it found may be one another case placed whose own undo was skipped
/// because this case still applied, and the unban's provenance check
/// (§5.6) decides whether that ban is Quack's to remove. A `no_change`
/// timeout gets none: the timeout it found ends at or after this case's
/// own end, so it was not this case's, and it expires on its own.
pub fn on_void(facts: &VoidFacts<'_>) -> VoidEffect {
    if !facts.kind.is_original() {
        return VoidEffect {
            cancel: false,
            queue_undo: None,
        };
    }
    // A done timeout or ban gets its undo even when it was `no_change`
    // (§5.9): the case that now owns what Discord shows (another case's
    // equal timeout, an earlier ban) may be this one, and the undo's guard
    // and provenance check decide whether anything is removed.
    let reached_discord = may_have_applied(facts.attempts) || facts.state == ActionState::Done;
    let wants_undo = facts.state != ActionState::Cancelled
        && !facts.has_undo
        && reached_discord
        && undo_needed(facts.kind, facts.timeout_ends_at, facts.now);
    VoidEffect {
        cancel: facts.state == ActionState::Pending,
        queue_undo: if wants_undo {
            facts.kind.undo_kind()
        } else {
            None
        },
    }
}

/// The case whose action keeps an undo from running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StillApplies {
    /// That case.
    pub case_id: CaseId,
    /// Its number, for "left in place because case #N still applies".
    pub case_number: u64,
}
