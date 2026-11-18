//! Staff controls (§5.8), the undo guard (§5.6), and what a void does to
//! each action (§5.9).

use time::OffsetDateTime;

use super::{ActionFailure, ActionKind, ActionPurpose, ActionState};
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

/// Whether staff may retry an action (§5.8). Member checks for originals
/// run separately.
pub fn can_retry(
    kind: ActionKind,
    state: ActionState,
    dismissed: bool,
    case_valid: bool,
    failure: Option<ActionFailure>,
) -> Result<(), DomainError> {
    if state != ActionState::Failed || dismissed {
        return Err(DomainError::ActionNotRetryable);
    }
    if kind.purpose() == ActionPurpose::Original {
        if !case_valid {
            return Err(DomainError::CaseVoided);
        }
        if failure == Some(ActionFailure::TimeoutWindowPassed) {
            return Err(DomainError::TimeoutWindowPassed);
        }
    }
    Ok(())
}

/// Whether staff may dismiss an action (§5.8).
pub fn can_dismiss(state: ActionState, dismissed: bool) -> Result<(), DomainError> {
    if state == ActionState::Failed && !dismissed {
        Ok(())
    } else {
        Err(DomainError::ActionNotDismissable)
    }
}

/// Whether staff may undo an original by hand, and with which undo kind
/// (§5.8): the original is a done timeout or ban with no undo yet, and a
/// timeout's end is still in the future.
pub fn can_undo(
    kind: ActionKind,
    state: ActionState,
    has_undo: bool,
    timeout_ends_at: Option<OffsetDateTime>,
    now: OffsetDateTime,
) -> Result<ActionKind, DomainError> {
    match kind.undo_kind() {
        Some(undo)
            if state == ActionState::Done
                && !has_undo
                && undo_needed(kind, timeout_ends_at, now) =>
        {
            Ok(undo)
        }
        _ => Err(DomainError::ActionNotUndoable),
    }
}

/// What voiding a case does to one of its actions (§5.9).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoidEffect {
    /// Nothing changes.
    Nothing,
    /// The pending original becomes `cancelled` and never starts.
    Cancel,
    /// The running original is left to finish; its settle checks validity.
    LeaveRunning,
    /// Insert an undo action of this kind and its job.
    QueueUndo(ActionKind),
}

/// Decides what a void does to an action of `kind` in `state`. Undo
/// actions are never cancelled by a void.
pub fn on_void(
    kind: ActionKind,
    state: ActionState,
    has_undo: bool,
    timeout_ends_at: Option<OffsetDateTime>,
    now: OffsetDateTime,
) -> VoidEffect {
    if kind.purpose() == ActionPurpose::Undo {
        return VoidEffect::Nothing;
    }
    match state {
        ActionState::Pending => VoidEffect::Cancel,
        ActionState::Running => VoidEffect::LeaveRunning,
        ActionState::Done if !has_undo && undo_needed(kind, timeout_ends_at, now) => kind
            .undo_kind()
            .map_or(VoidEffect::Nothing, VoidEffect::QueueUndo),
        _ => VoidEffect::Nothing,
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

/// One of the member's other actions, as the undo guard query reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GuardCandidate {
    /// The action's case.
    pub case_id: CaseId,
    /// That case's number.
    pub case_number: u64,
    /// Whether that case is valid.
    pub case_valid: bool,
    /// The action's kind.
    pub kind: ActionKind,
    /// The action's state.
    pub state: ActionState,
    /// For a timeout, its fixed end.
    pub timeout_ends_at: Option<OffsetDateTime>,
    /// Whether an undo of it is already done.
    pub undone: bool,
}

/// The undo guard (§5.6): before an undo runs, finds the newest other valid
/// case whose action of the same original kind is still in force for the
/// member. `Some` means the undo is skipped. Mirrors the design's SQL.
pub fn undo_guard(
    undo: ActionKind,
    own_case: CaseId,
    candidates: &[GuardCandidate],
    now: OffsetDateTime,
) -> Option<StillApplies> {
    let original = undo.undoes()?;
    candidates
        .iter()
        .filter(|c| {
            c.kind == original
                && c.case_id != own_case
                && c.case_valid
                && matches!(
                    c.state,
                    ActionState::Pending | ActionState::Running | ActionState::Done
                )
                && (c.kind != ActionKind::Timeout || c.timeout_ends_at.is_some_and(|end| end > now))
                && !c.undone
        })
        .max_by_key(|c| c.case_number)
        .map(|c| StillApplies {
            case_id: c.case_id,
            case_number: c.case_number,
        })
}
