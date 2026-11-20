//! Case messages (§12.4, §12.5, §12.6 voiding and context, §12.12 lists):
//! the staff card, the receipts, adding a case, voiding, editing context,
//! and case lists.

mod add;
mod card;
mod edit;
mod list;
mod receipt;
mod void;

use quack_core::case::ContextValue;
use quack_core::rule::FieldKind;

pub use add::{
    AlreadySaved, FormField, PickerRule, context_problem, draft_expired, draft_page_already_saved,
    draft_page_invalid, draft_page_saved, draft_rule_changed, draft_save_failed, form_modal,
    form_title, picker, picker_fill_form, picker_no_rules,
};
pub use card::{CardAction, CaseCard, EvidenceSummary, Imported, Voided, card};
pub use edit::{context_modal, context_saved, context_too_many_fields};
pub use list::{CaseRow, HistoryTotals, ListFilter, ListPage, history, list};
pub use receipt::{
    ReceiptView, receipt, receipt_post_failed, receipt_posted, receipt_posted_reply,
    receipt_replayed, saved_unconfirmed,
};
pub use void::{VoidResult, void_done, void_modal, void_reason_appeal_accepted};

/// Answers longer than this are cut on the card and in DMs.
pub const CONTEXT_CUT: usize = 300;

/// One answered case context field, in the rule version's field order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextLine {
    /// The field's label.
    pub label: String,
    /// The field's kind.
    pub kind: FieldKind,
    /// The answer.
    pub value: ContextValue,
}

impl ContextLine {
    /// Whether the answer is a message link (members see a placeholder
    /// for those, and DMs leave them out).
    pub fn is_link(&self) -> bool {
        self.kind == FieldKind::MessageLink
    }
}

/// An answer as text: booleans as Yes/No, numbers as written, a message
/// link as a link, other text cut to `max` characters.
pub fn context_value(line: &ContextLine, max: usize) -> crate::model::Text {
    let t = crate::model::Text::new();
    match &line.value {
        ContextValue::Text(url) if line.is_link() => t.push(crate::model::Inline::Link {
            text: vec![crate::model::Inline::User(crate::fmt::cut(url, max))],
            url: url.clone(),
        }),
        ContextValue::Text(s) => t.user(crate::fmt::cut(s, max)),
        ContextValue::Boolean(b) => t.s(crate::word::yes_no(*b)),
        ContextValue::Number(n) => t.s(n.to_string()),
    }
}

/// `member.context.link_saved`: what members see instead of a message link.
pub fn member_link_saved() -> crate::model::Text {
    crate::model::Text::fixed("A message saved as evidence")
}

/// `context.details_label` / `case.context.details_label`: the label of the
/// free-text field of a rule with no fields.
pub const DETAILS_LABEL: &str = "Details";
