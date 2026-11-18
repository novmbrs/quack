//! Staff controls on actions (§12.6): retry, dismiss, and undo.

use quack_core::action::ActionKind;

use crate::fmt;
use crate::model::{Person, Text};
use crate::word;

/// The two undo actions, so undo texts can't be asked for a kick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Undo {
    /// Remove a timeout.
    RemoveTimeout,
    /// Unban.
    Unban,
}

/// The undo for an original action, if it has one (a kick has none).
pub const fn undo_from_original(kind: ActionKind) -> Option<Undo> {
    match kind {
        ActionKind::Timeout => Some(Undo::RemoveTimeout),
        ActionKind::Ban => Some(Undo::Unban),
        ActionKind::Kick | ActionKind::RemoveTimeout | ActionKind::Unban => None,
    }
}

/// "remove the timeout", "ban the member": an action as the thing Quack
/// will try, without naming the member.
const fn try_to(kind: ActionKind) -> &'static str {
    match kind {
        ActionKind::Timeout => "time out the member",
        ActionKind::Kick => "remove the member from the server",
        ActionKind::Ban => "ban the member",
        ActionKind::RemoveTimeout => "remove the timeout",
        ActionKind::Unban => "unban the member",
    }
}

/// "the ban", "removing the timeout": what a retry tries again.
pub(crate) const fn retry_noun(kind: ActionKind) -> &'static str {
    match kind {
        ActionKind::Timeout => "the timeout",
        ActionKind::Kick => "the kick",
        ActionKind::Ban => "the ban",
        ActionKind::RemoveTimeout => "removing the timeout",
        ActionKind::Unban => "unbanning the member",
    }
}

/// "the ban failure", "the failed attempt to unban the member".
pub(crate) fn failure_noun(kind: ActionKind) -> String {
    match kind {
        ActionKind::Timeout | ActionKind::Kick | ActionKind::Ban => {
            format!("the {} failure", word::action(kind))
        }
        ActionKind::RemoveTimeout | ActionKind::Unban => {
            format!("the failed attempt to {}", try_to(kind))
        }
    }
}

/// `action.retry.done`.
pub fn retry_done(kind: ActionKind, case_number: u64) -> Text {
    Text::fixed(format!(
        "Quack will try again to {} for case #{case_number}. Open the case to see the outcome.",
        try_to(kind)
    ))
}

/// `action.dismiss.done`.
pub fn dismiss_done(kind: ActionKind, case_number: u64) -> Text {
    Text::fixed(fmt::upper_first(&format!(
        "{} for case #{case_number} was dismissed. The case keeps its history.",
        failure_noun(kind)
    )))
}

/// `action.undo.confirm.*`.
pub fn undo_confirm(undo: Undo, member: &Person, case_number: u64) -> Text {
    match undo {
        Undo::RemoveTimeout => Text::fixed("Remove ")
            .person(member)
            .s(format!("'s timeout for case #{case_number}?")),
        Undo::Unban => Text::fixed("Unban ")
            .person(member)
            .s(format!(" for case #{case_number}?")),
    }
}

/// `action.undo.button.*`.
pub const fn undo_button(undo: Undo) -> &'static str {
    match undo {
        Undo::RemoveTimeout => "Yes, remove timeout",
        Undo::Unban => "Yes, unban",
    }
}

/// `action.undo.queued.*`: the undo checks Discord before it changes
/// anything (§5.6), so this promises a check, not a result.
pub fn undo_queued(undo: Undo, member: &Person, case_number: u64) -> Text {
    let t = match undo {
        Undo::RemoveTimeout => Text::fixed("Quack will check whether it can remove ")
            .person(member)
            .s("'s timeout."),
        Undo::Unban => Text::fixed("Quack will check whether it can unban ")
            .person(member)
            .s("."),
    };
    t.s(format!(" Open case #{case_number} to see the outcome."))
}
