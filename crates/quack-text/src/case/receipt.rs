//! The public receipt (§11.7, §12.4) and the messages around posting one.

use quack_core::ids::CaseId;

use super::card::{CaseCard, field, heading, tone};
use crate::action::{self, ActionView, Detail};
use crate::model::{Block, Msg, Person, Text};
use crate::word;

/// The public receipt's fixed fields (§11.7): nothing staff-only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiptView {
    /// The case.
    pub case: CaseId,
    /// Its number.
    pub number: u64,
    /// The rule's name.
    pub rule: String,
    /// Whether it is voided.
    pub voided: bool,
    /// The member.
    pub member: Person,
    /// The official reason.
    pub reason: String,
    /// The original action; `None` is a warning.
    pub action: Option<ActionView>,
}

impl CaseCard {
    /// The public receipt's projection of this case.
    pub fn receipt(&self) -> ReceiptView {
        ReceiptView {
            case: self.case,
            number: self.number,
            rule: self.rule.clone(),
            voided: self.voided.is_some(),
            member: self.member.clone(),
            reason: self.reason.clone(),
            action: self.action.as_ref().map(|a| a.view.clone()),
        }
    }
}

/// `case.receipt.*`: the public receipt. Only its fixed fields, and only
/// the Open case button.
pub fn receipt(r: &ReceiptView) -> Msg {
    let sentence = r.action.as_ref().map_or_else(action::none, |a| {
        action::sentence(a, &r.member, Detail::Receipt)
    });
    Msg::new(tone(r.action.as_ref(), r.voided))
        .block(heading(r.number, &r.rule, r.voided))
        .para(Text::new().person(&r.member))
        .block(Block::Quote(Text::new().user(&r.reason)))
        .block(field("Action", sentence))
        .buttons(vec![word::open_case_button(r.case)])
}

/// `case.receipt.posted`.
pub fn receipt_posted(number: u64) -> Text {
    Text::fixed(format!(
        "Case #{number} is saved. Quack posted the receipt in this channel."
    ))
}

/// `case.receipt.post_failed`.
pub fn receipt_post_failed(number: u64) -> Text {
    Text::fixed(format!(
        "Case #{number} is saved. Quack couldn't post the receipt in this channel, so here it is privately."
    ))
}

/// `case.receipt.replayed`.
pub fn receipt_replayed(number: u64) -> Text {
    Text::fixed(format!("Case #{number} is already saved. Run "))
        .command(crate::model::Command::CaseView)
        .s(format!(" and enter {number} as the number to see it."))
}

/// `case.saved_unconfirmed`.
pub fn saved_unconfirmed() -> Text {
    Text::fixed(
        "Quack couldn't confirm whether the case was saved. Select Try again. If the case was already saved, Quack will open it.",
    )
}
