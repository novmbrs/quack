//! Staff controls on actions (§12.6): retry, dismiss, and undo.

use quack_core::action::ActionKind;

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

/// `action.retry.done`.
pub fn retry_done(kind: ActionKind, case_number: u64) -> Text {
    Text::fixed(format!(
        "Quack will try the {} for case #{case_number} again. The case shows the result.",
        word::action_noun(kind)
    ))
}

/// `action.dismiss.done`.
pub fn dismiss_done(kind: ActionKind, case_number: u64) -> Text {
    Text::fixed(format!(
        "The failed {} for case #{case_number} is dismissed. The case keeps its failed attempts.",
        word::action_noun(kind)
    ))
}

/// `action.undo.confirm.*`.
pub fn undo_confirm(undo: Undo, member: &Person, case_number: u64) -> Text {
    match undo {
        Undo::RemoveTimeout => Text::fixed("Remove ")
            .person(member)
            .s(format!("'s timeout from case #{case_number}?")),
        Undo::Unban => Text::fixed("Unban ")
            .person(member)
            .s(format!("? Their ban came from case #{case_number}.")),
    }
}

/// `action.undo.button.*`.
pub const fn undo_button(undo: Undo) -> &'static str {
    match undo {
        Undo::RemoveTimeout => "Yes, remove timeout",
        Undo::Unban => "Yes, unban",
    }
}

/// `action.undo.queued.*`.
pub fn undo_queued(undo: Undo, member: &Person, case_number: u64) -> Text {
    let t = match undo {
        Undo::RemoveTimeout => Text::fixed("Quack is removing ")
            .person(member)
            .s("'s timeout."),
        Undo::Unban => Text::fixed("Quack is unbanning ").person(member).s("."),
    };
    t.s(format!(" Case #{case_number} shows the result."))
}
