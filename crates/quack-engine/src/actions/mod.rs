//! Actions (§5): the `run_action` job handler, its finalize, and the staff
//! controls (retry, dismiss, undo).

mod act;
mod controls;
mod run;
mod settle;

use quack_core::action::{
    ActionKind, ActionSkipReason, AttemptOutcome, Failure, case_marker, undo_marker,
};
use quack_core::ids::ActionId;
use quack_core::modlog::{
    ActionDoneDetail, ActionFailedDetail, ModLogEntry, ModLogLinks, UndoSkippedDetail,
};
use quack_db::actions::Action;
use quack_db::cases::Case;
use quack_db::{DbError, Tx};

use crate::modlog::{self, By};

pub(crate) use controls::view;
pub use controls::{ActionError, ControlDone, dismiss, retry, undo};
pub use run::{finalize, run};

/// The `run_action` payload.
#[derive(Clone, Copy, Debug, serde::Deserialize)]
struct Payload {
    action_id: ActionId,
}

/// The `X-Audit-Log-Reason` for an action (§5.1): the case marker for
/// originals, `Quack case #<n> voided` for undos.
fn audit_reason(kind: ActionKind, case_number: u64, rule_name: &str) -> String {
    if kind.is_original() {
        case_marker(case_number, rule_name)
    } else {
        undo_marker(case_number)
    }
}

fn links(action: &Action) -> ModLogLinks {
    ModLogLinks {
        case_id: Some(action.case),
        member_id: Some(action.member),
        action_id: Some(action.id),
        ..ModLogLinks::default()
    }
}

/// Logs `action_done`.
async fn log_done(
    tx: &mut Tx,
    case: &Case,
    action: &Action,
    attempts: u32,
    no_change: bool,
) -> Result<(), DbError> {
    let entry = ModLogEntry::ActionDone(ActionDoneDetail {
        case_number: case.number,
        kind: action.kind,
        attempts,
        no_change,
    });
    modlog::write(tx, case.guild, entry, By::QUACK, links(action)).await?;
    Ok(())
}

/// Logs `action_failed`.
async fn log_failed(
    tx: &mut Tx,
    case: &Case,
    action: &Action,
    failure: &Failure,
    attempts: u32,
) -> Result<(), DbError> {
    let entry = ModLogEntry::ActionFailed(ActionFailedDetail {
        case_number: case.number,
        kind: action.kind,
        reason: failure.reason,
        permission: failure.permission,
        http_status: failure.http_status,
        discord_code: failure.discord_code,
        attempts,
    });
    modlog::write(tx, case.guild, entry, By::QUACK, links(action)).await?;
    Ok(())
}

/// Logs `action_undo_skipped`.
async fn log_skipped(
    tx: &mut Tx,
    case: &Case,
    action: &Action,
    reason: ActionSkipReason,
    still_applies: Option<u64>,
) -> Result<(), DbError> {
    let entry = ModLogEntry::ActionUndoSkipped(UndoSkippedDetail {
        case_number: case.number,
        kind: action.kind,
        skip_reason: reason,
        still_applies_case_number: still_applies,
    });
    modlog::write(tx, case.guild, entry, By::QUACK, links(action)).await?;
    Ok(())
}

/// The outcomes of the current round's attempts, oldest first, with an
/// open attempt as `None`.
async fn round(tx: &mut Tx, action: &Action) -> Result<Vec<Option<AttemptOutcome>>, DbError> {
    let all = quack_db::actions::attempts(tx, action.id).await?;
    let take = usize::try_from(action.attempts).unwrap_or(usize::MAX);
    let skip = all.len().saturating_sub(take);
    Ok(all.into_iter().skip(skip).map(|a| a.outcome).collect())
}
