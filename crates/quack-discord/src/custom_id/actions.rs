//! `quack_text::actions` → custom ids, plus the ids of modal inputs and the
//! values of select options.

use quack_core::rule::FieldKey;
use quack_text::actions::{ButtonAction, InputId, ModalAction, SelectAction, SelectValue};

use super::CustomId;

impl From<&ButtonAction> for CustomId {
    fn from(action: &ButtonAction) -> Self {
        use ButtonAction as B;
        match *action {
            B::OpenCase(c) => Self::OpenCase(c),
            B::EditContext(c) => Self::EditContext(c),
            B::AddEvidence(c) => Self::AddEvidence(c),
            B::ViewEvidence { case, item } => Self::ViewEvidence { case, item },
            B::VoidCase(c) => Self::VoidCase(c),
            B::History { member, page } => Self::History { member, page },
            B::CaseListPage { page, member, rule } => Self::CaseList { page, member, rule },
            B::RetryAction(a) => Self::RetryAction(a),
            B::DismissAction(a) => Self::DismissAction(a),
            B::UndoAction(a) => Self::UndoAction(a),
            B::ConfirmUndo(a) => Self::ConfirmUndo(a),
            B::RulePickerPage { draft, page } => Self::RulePickerPage { draft, page },
            B::DraftContinue(p) => Self::DraftContinue(p),
            B::DraftRetryPage(p) => Self::DraftRetryPage(p),
            B::DraftSaveAgain(d) => Self::DraftSaveAgain(d),
            B::Appeal(c) => Self::Appeal(c),
            B::AcceptAppeal { appeal, version } => Self::AcceptAppeal { appeal, version },
            B::RejectAppeal { appeal, version } => Self::RejectAppeal { appeal, version },
            B::OpenAppeal(a) => Self::OpenAppeal(a),
            B::AppealsPage(p) => Self::AppealsPage(p),
            B::RulesPage(p) => Self::RulesPage(p),
            B::Setup(area, op) => Self::Setup(area, op),
            B::OpenTicket => Self::OpenTicket,
            B::CloseTicket(t) => Self::CloseTicket(t),
            B::FixTicketAccess(t) => Self::FixTicketAccess(t),
        }
    }
}

impl From<SelectAction> for CustomId {
    fn from(action: SelectAction) -> Self {
        match action {
            SelectAction::OpenCase => Self::CasePicker,
            SelectAction::PickRule(draft) => Self::RulePicker(draft),
            SelectAction::Setup(area, op) => Self::Setup(area, op),
        }
    }
}

impl From<&ModalAction> for CustomId {
    fn from(action: &ModalAction) -> Self {
        use ModalAction as M;
        match *action {
            M::EditContext { case, revision } => Self::EditContextModal { case, revision },
            M::AddEvidence(c) => Self::AddEvidenceModal(c),
            M::VoidCase(c) => Self::VoidCaseModal(c),
            M::DraftPage(p) => Self::DraftPageModal(p),
            M::Appeal(c) => Self::AppealModal(c),
            M::AcceptAppeal { appeal, version } => Self::AcceptAppealModal { appeal, version },
            M::RejectAppeal { appeal, version } => Self::RejectAppealModal { appeal, version },
            M::SetupInvite => Self::SetupInviteModal,
            M::SetupWarning => Self::SetupWarningModal,
        }
    }
}

/// The custom id of a modal input: `f:<field key>`, or a fixed name.
pub fn encode_input_id(id: &InputId) -> String {
    match id {
        InputId::Field(key) => format!("f:{}", key.as_str()),
        InputId::Reason => "reason".to_owned(),
        InputId::Links => "links".to_owned(),
        InputId::Statement => "statement".to_owned(),
        InputId::Invite => "invite".to_owned(),
        InputId::Warning => "warning".to_owned(),
    }
}

/// Reads a modal input's custom id; `None` for anything Quack didn't write.
pub fn decode_input_id(raw: &str) -> Option<InputId> {
    Some(match raw {
        "reason" => InputId::Reason,
        "links" => InputId::Links,
        "statement" => InputId::Statement,
        "invite" => InputId::Invite,
        "warning" => InputId::Warning,
        other => {
            let key = FieldKey(other.strip_prefix("f:")?.to_owned());
            if !key.is_valid() {
                return None;
            }
            InputId::Field(key)
        }
    })
}

/// The value of a select option: a case or rule as `u22`, or `yes`/`no`.
/// Which one a value is follows from the select's custom id.
pub fn encode_select_value(value: SelectValue) -> String {
    match value {
        SelectValue::Case(c) => c.to_u22(),
        SelectValue::Rule(r) => r.to_u22(),
        SelectValue::Yes => "yes".to_owned(),
        SelectValue::No => "no".to_owned(),
    }
}
