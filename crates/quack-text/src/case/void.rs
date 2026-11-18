//! Voiding a case (§4.7, §12.6).

use quack_core::action::ActionKind;
use quack_core::ids::CaseId;

use crate::actions::{InputId, ModalAction};
use crate::budget;
use crate::model::{Input, InputKind, InputStyle, Modal, Person, Text};

/// `case.void.modal.*`: the void modal. The number is unknown when the
/// button carried only the case id and no read was allowed before the modal.
pub fn void_modal(case: CaseId, number: Option<u64>) -> Modal {
    Modal {
        action: ModalAction::VoidCase(case),
        title: number.map_or_else(|| "Void case".to_owned(), |n| format!("Void case #{n}")),
        inputs: vec![Input {
            id: InputId::Reason,
            label: "Why are you voiding this case?".to_owned(),
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
/// (`case.void.cancelled_action`, `case.void.running_*`,
/// `case.void.done_kick`, `case.void.undo.*`, `case.void.appeal_closed`).
/// An undo checks Discord first (§5.6), so these promise a check, not a
/// result, and point at the case for the outcome.
pub fn void_done(
    number: u64,
    rule: &str,
    member: &Person,
    results: &[VoidResult],
    appeal_closed: bool,
) -> Text {
    let mut t = Text::fixed(format!(
        "Case #{number} is voided. It stays in the history and no longer counts toward "
    ))
    .bold_user(rule)
    .s(".");
    let mut check = false;
    for r in results {
        t = t.s(" ");
        t = match r {
            VoidResult::Cancelled(kind) => t
                .s("Quack won't ")
                .then(match kind {
                    ActionKind::Timeout | ActionKind::RemoveTimeout => {
                        Text::fixed("time out ").person(member)
                    }
                    ActionKind::Kick => Text::fixed("remove ").person(member).s(" from the server"),
                    ActionKind::Ban | ActionKind::Unban => Text::fixed("ban ").person(member),
                })
                .s("."),
            VoidResult::Running(kind) => {
                check = true;
                let what = if matches!(kind, ActionKind::Ban | ActionKind::Unban) {
                    "ban"
                } else {
                    "timeout"
                };
                let undo_listed = results
                    .iter()
                    .any(|r| matches!(r, VoidResult::RemovingTimeout | VoidResult::Unbanning));
                let t = t.s(format!("Quack had already started the {what}."));
                if undo_listed {
                    t
                } else {
                    t.s(" Quack will check whether it can be removed.")
                }
            }
            VoidResult::RunningKick => t
                .s("Quack had already started removing ")
                .person(member)
                .s(" from the server. A kick can't be undone."),
            VoidResult::DoneKick => t
                .s("Quack had already removed ")
                .person(member)
                .s(" from the server. A kick can't be undone."),
            VoidResult::RemovingTimeout => {
                check = true;
                t.s("Quack will check whether it can remove ")
                    .person(member)
                    .s("'s timeout.")
            }
            VoidResult::Unbanning => {
                check = true;
                t.s("Quack will check whether it can unban ")
                    .person(member)
                    .s(".")
            }
        };
    }
    if check {
        t = t.s(format!(" Open case #{number} to see the outcome."));
    }
    if appeal_closed {
        t = t
            .s(" The appeal was closed because the case was voided. Quack will try to send ")
            .person(member)
            .s(" a DM about it.");
    }
    t
}
