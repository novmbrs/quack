//! Voiding a case (§4.7, §12.6).

use quack_core::action::ActionKind;
use quack_core::ids::CaseId;

use crate::actions::{InputId, ModalAction};
use crate::budget;
use crate::model::{Input, InputKind, InputStyle, Modal, Person, Text};
use crate::word;

/// `case.void.modal.*`: the void modal. The number is unknown when the
/// button carried only the case id and no read was allowed before the modal.
pub fn void_modal(case: CaseId, number: Option<u64>) -> Modal {
    Modal {
        action: ModalAction::VoidCase(case),
        title: number.map_or_else(|| "Void case".to_owned(), |n| format!("Void case #{n}")),
        inputs: vec![Input {
            id: InputId::Reason,
            label: "Why is this case wrong?".to_owned(),
            description: Some("The member can see this reason.".to_owned()),
            required: true,
            kind: InputKind::Text {
                style: InputStyle::Paragraph,
                placeholder: None,
                min_length: 1,
                max_length: u16::try_from(quack_core::limits::VOID_REASON.max)
                    .unwrap_or(u16::try_from(budget::INPUT_MAX_LENGTH).unwrap_or(u16::MAX)),
                value: None,
            },
        }],
    }
}

/// `case.void.reason.appeal_accepted`: the void reason an accepted appeal
/// records.
pub fn void_reason_appeal_accepted() -> &'static str {
    "Appeal accepted."
}

/// What a void did to the case's action, appeal, and timeout or ban.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VoidResult {
    /// The original action hadn't started; it never will.
    Cancelled(ActionKind),
    /// The original's attempt was in flight; it is undone if it went
    /// through (timeout or ban).
    Running(ActionKind),
    /// A kick was in flight; it can't be undone.
    RunningKick,
    /// The kick was done; it can't be undone.
    DoneKick,
    /// Quack is removing the timeout.
    RemovingTimeout,
    /// Quack is unbanning.
    Unbanning,
}

/// `case.void.done` followed by one sentence for each thing the void did
/// (`case.void.undo.*`, `case.void.cancelled_action`,
/// `case.void.running_*`, `case.void.done_kick`, `case.void.appeal_closed`).
pub fn void_done(
    number: u64,
    rule: &str,
    member: &Person,
    results: &[VoidResult],
    appeal_closed: bool,
) -> Text {
    let mut t = Text::fixed(format!(
        "Case #{number} is voided. It stays in the history but no longer counts toward "
    ))
    .bold_user(rule)
    .s(".");
    for r in results {
        t = t.s(" ");
        t = match r {
            VoidResult::Cancelled(kind) => {
                t.s(format!("Quack won't run the {}.", word::action_noun(*kind)))
            }
            VoidResult::Running(kind) => t.s(format!(
                "Quack had already started the {}. If it went through, Quack will undo it.",
                word::action_noun(*kind)
            )),
            VoidResult::RunningKick => t
                .s("Quack had already started the kick, so ")
                .person(member)
                .s(" may still be removed. A kick can't be undone."),
            VoidResult::DoneKick => t
                .person(member)
                .s(" was already removed from the server. A kick can't be undone."),
            VoidResult::RemovingTimeout => {
                t.s("Quack is removing ").person(member).s("'s timeout.")
            }
            VoidResult::Unbanning => t.s("Quack is unbanning ").person(member).s("."),
        };
    }
    if appeal_closed {
        t = t
            .s(" The open appeal is closed, and ")
            .person(member)
            .s(" gets a DM about it.");
    }
    t
}
