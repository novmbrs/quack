//! Every interaction → one small handler (§11.1, §11.5).
//!
//! The router itself does no I/O. Each handler acknowledges first through
//! [`Pending`], then authorizes and calls the engine through the
//! [`crate::ack::Acked`] handle.

use quack_discord::custom_id::{ComponentId, CustomId, Legacy};
use quack_discord::interaction::{FieldValue, Request, SelectChoice, Source, UnknownReason};
use quack_telemetry::labels::Reason;
use quack_text::actions::InputId;
use quack_text::appeal::ModalDecision;
use quack_text::error;

use crate::ack::{Failure, Finish, Pending};
use crate::handlers::{
    add_menu, appeals, case_add, case_edit, case_list, case_view, case_void, controls, drafts,
    evidence, evidence_view, help, member_appeal, module, rules, setup,
};
use crate::modules::is_module_id;

/// `interaction.expired_button`.
pub(crate) fn expired() -> Failure {
    Failure::refused(error::expired_button(), Reason::Invalid)
}

/// Routes one interaction to its handler.
pub(crate) async fn route(p: Pending<'_>) -> Finish {
    let ix = p.ix;
    match &ix.request {
        Request::Help { topic } => help::run(p, *topic).await,
        Request::Autocomplete { command, query } => {
            case_add::autocomplete(p, *command, query).await
        }
        Request::CaseAdd {
            member,
            rule,
            private,
            file,
        } => case_add::run(p, member, rule, *private, file.as_ref()).await,
        Request::CaseEvidence {
            number,
            links,
            files,
            note,
        } => evidence::command(p, *number, links, files, note.as_deref()).await,
        Request::CaseView { number } => case_view::command(p, *number).await,
        Request::CaseList { member, rule } => {
            case_list::command(p, member.as_ref(), rule.as_ref()).await
        }
        Request::CaseVoid { number, reason } => case_void::command(p, *number, reason).await,
        Request::CaseHistory { member } => case_list::history_command(p, member).await,
        Request::Appeals => appeals::command(p).await,
        Request::Rules => rules::command(p).await,
        Request::Setup => setup::command(p).await,
        Request::AddCaseFromMessage { message } => add_menu::from_message(p, message).await,
        Request::AddCaseForMember { member } => add_menu::for_member(p, member).await,
        Request::AddAsEvidence { message } => evidence::from_message(p, message).await,
        Request::Component { id, values, source } => component(p, *id, values, *source).await,
        Request::ModalSubmit { id, fields, source } => modal(p, *id, fields, *source).await,
        // A command Quack no longer registers (Go-era ones linger in
        // clients until the sync reaches them) reads like an old button.
        Request::Unknown(UnknownReason::UnknownCommand) => {
            p.fail(Failure::refused(error::legacy_button(), Reason::Invalid))
                .await
        }
        // An autocomplete Quack couldn't read still needs choices (none).
        Request::Unknown(UnknownReason::Malformed { autocomplete: true }) => p.choices(&[]).await,
        Request::Unknown(_) => p.fail(expired()).await,
    }
}

/// The current id a received id stands for, or the answer for one that
/// names nothing current.
async fn current(
    p: Pending<'_>,
    id: ComponentId,
    source: Source,
) -> Result<(Pending<'_>, CustomId), Finish> {
    match id {
        ComponentId::Legacy(Legacy::Mapped(CustomId::OpenCase(case))) => {
            Err(case_view::open_legacy(p, case, source).await)
        }
        ComponentId::Current(id) | ComponentId::Legacy(Legacy::Mapped(id)) => Ok((p, id)),
        ComponentId::Legacy(Legacy::CaseOfAction(action)) => {
            Err(case_view::of_action(p, action, source).await)
        }
        ComponentId::Expired => Err(p.fail(expired()).await),
        ComponentId::Unrecognized => Err(p
            .fail(Failure::refused(error::legacy_button(), Reason::Invalid))
            .await),
    }
}

/// Buttons and selects (§11.5).
async fn component(
    p: Pending<'_>,
    id: ComponentId,
    values: &[SelectChoice],
    source: Source,
) -> Finish {
    let (p, id) = match current(p, id, source).await {
        Ok(found) => found,
        Err(finish) => return finish,
    };
    if is_module_id(&id) {
        return module::component(p, id, values, &[], source).await;
    }
    match id {
        CustomId::OpenCase(case) => case_view::open(p, case, source).await,
        CustomId::CasePicker => case_view::picked(p, values, source).await,
        CustomId::EditContext(case) => case_edit::open_context(p, case).await,
        CustomId::AddEvidence(case) => evidence::open_modal(p, case).await,
        CustomId::ViewEvidence { case, item } => evidence_view::view(p, case, item, source).await,
        CustomId::EvidenceSurrounding(e) => evidence_view::surrounding(p, e, source).await,
        CustomId::EvidenceVisibility { evidence, visible } => {
            evidence_view::visibility(p, evidence, visible, source).await
        }
        CustomId::EditEvidenceNote(e) => evidence_view::open_note(p, e).await,
        CustomId::RemoveEvidence(e) => evidence_view::ask_remove(p, e, source).await,
        CustomId::ConfirmRemoveEvidence(e) => evidence_view::remove(p, e, source).await,
        CustomId::AddAsEvidence { channel, message } => {
            evidence::picked(p, channel, message, values, source).await
        }
        CustomId::AddAsEvidenceByNumber { channel, message } => {
            evidence::open_number(p, channel, message).await
        }
        CustomId::VoidCase(case) => case_void::open(p, case).await,
        CustomId::History { member, page } => {
            case_list::history_page(p, member, page, source).await
        }
        CustomId::CaseList { page, member, rule } => {
            case_list::page(p, page, member, rule, source).await
        }
        CustomId::RetryAction(action) => controls::retry(p, action, source).await,
        CustomId::DismissAction(action) => controls::dismiss(p, action, source).await,
        CustomId::UndoAction(action) => controls::ask_undo(p, action, source).await,
        CustomId::ConfirmUndo(action) => controls::undo(p, action, source).await,
        CustomId::RulePicker(draft) => add_menu::picked(p, draft, values, source).await,
        CustomId::RulePickerPage { draft, page } => add_menu::page(p, draft, page, source).await,
        CustomId::DraftContinue(page) => drafts::open_page(p, page).await,
        CustomId::DraftRetryPage(page) => drafts::retry_page(p, page).await,
        CustomId::DraftSaveAgain(draft) => drafts::save_again(p, draft, source).await,
        CustomId::Appeal(case) => member_appeal::open(p, case).await,
        CustomId::AcceptAppeal { appeal, version } => {
            appeals::open_decision(p, ModalDecision::Accept, appeal, version).await
        }
        CustomId::RejectAppeal { appeal, version } => {
            appeals::open_decision(p, ModalDecision::Reject, appeal, version).await
        }
        CustomId::OpenAppeal(appeal) => appeals::open(p, appeal, source).await,
        CustomId::AppealsPage(page) => appeals::page(p, page, source).await,
        CustomId::RulesPage(page) => rules::page(p, page, source).await,
        CustomId::Setup(area, op) => setup::control(p, area, op, values, source).await,
        // A modal's id on a button: nothing current renders that.
        CustomId::EditContextModal { .. }
        | CustomId::EvidenceNoteModal { .. }
        | CustomId::AddAsEvidenceModal { .. }
        | CustomId::AddEvidenceModal(_)
        | CustomId::VoidCaseModal(_)
        | CustomId::DraftPageModal(_)
        | CustomId::AppealModal(_)
        | CustomId::AcceptAppealModal { .. }
        | CustomId::RejectAppealModal { .. }
        | CustomId::SetupInviteModal
        | CustomId::SetupWarningModal
        | CustomId::OpenTicket
        | CustomId::CloseTicket(_)
        | CustomId::FixTicketAccess(_) => p.fail(expired()).await,
    }
}

/// Modal submits (§11.5).
async fn modal(
    p: Pending<'_>,
    id: ComponentId,
    fields: &[(InputId, FieldValue)],
    source: Source,
) -> Finish {
    let (p, id) = match current(p, id, source).await {
        Ok(found) => found,
        Err(finish) => return finish,
    };
    if is_module_id(&id) {
        return module::component(p, id, &[], fields, source).await;
    }
    match id {
        CustomId::EditContextModal { case, revision } => {
            case_edit::save_context(p, case, revision, fields, source).await
        }
        CustomId::AddEvidenceModal(case) => evidence::submit_modal(p, case, fields, source).await,
        CustomId::EvidenceNoteModal { evidence, revision } => {
            evidence_view::save_note(p, evidence, revision, fields, source).await
        }
        CustomId::AddAsEvidenceModal { channel, message } => {
            evidence::submit_number(p, channel, message, fields, source).await
        }
        CustomId::VoidCaseModal(case) => case_void::submit(p, case, fields, source).await,
        CustomId::DraftPageModal(page) => drafts::submit(p, page, fields, source).await,
        CustomId::AppealModal(case) => member_appeal::submit(p, case, fields, source).await,
        CustomId::AcceptAppealModal { appeal, version } => {
            appeals::decide(p, ModalDecision::Accept, appeal, version, fields, source).await
        }
        CustomId::RejectAppealModal { appeal, version } => {
            appeals::decide(p, ModalDecision::Reject, appeal, version, fields, source).await
        }
        CustomId::SetupInviteModal => setup::save_invite(p, fields, source).await,
        CustomId::SetupWarningModal => setup::save_warning(p, fields, source).await,
        _ => p.fail(expired()).await,
    }
}

/// The text a modal sent for `input` (empty when absent).
pub(crate) fn text_of(fields: &[(InputId, FieldValue)], input: &InputId) -> String {
    fields
        .iter()
        .find(|(id, _)| id == input)
        .and_then(|(_, v)| match v {
            FieldValue::Text(t) => Some(t.clone()),
            FieldValue::YesNo(_) | FieldValue::Files(_) => None,
        })
        .unwrap_or_default()
}
