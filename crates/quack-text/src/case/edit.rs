//! Editing a case's context (§4.9, §12.6).

use quack_core::ids::CaseId;

use super::add::{FormField, field_input};
use crate::actions::ModalAction;
use crate::budget;
use crate::fmt;
use crate::model::{Modal, Msg, Text, Tone};
use crate::word;

/// `case.context.modal.title` with the case's fields (at most 5; more go
/// to the dashboard, see [`context_too_many_fields`]).
pub fn context_modal(case: CaseId, number: u64, revision: u32, fields: &[FormField]) -> Modal {
    Modal {
        action: ModalAction::EditContext { case, revision },
        title: fmt::cut(
            &format!("Edit context · case #{number}"),
            budget::MODAL_TITLE,
        ),
        inputs: fields
            .iter()
            .take(budget::MODAL_INPUTS)
            .map(field_input)
            .collect(),
    }
}

/// `case.context.saved`, or `case.context.saved_new_evidence` when new
/// message links became evidence.
pub fn context_saved(number: u64, new_evidence: u32) -> Text {
    let t = Text::fixed(format!("The context of case #{number} is saved."));
    match new_evidence {
        0 => t,
        1 => t.s(" Quack is saving 1 new evidence item."),
        n => t.s(format!(
            " Quack is saving {} new evidence items.",
            fmt::count(n)
        )),
    }
}

/// `case.context.too_many_fields`, with a link to the case on the dashboard.
pub fn context_too_many_fields(number: u64, dashboard_url: &str) -> Msg {
    Msg::text(
        Tone::Neutral,
        Text::fixed(format!(
            "Case #{number} has more context fields than fit in a Discord form. Edit it on the dashboard."
        )),
    )
    .buttons(vec![word::dashboard_button(dashboard_url)])
}
