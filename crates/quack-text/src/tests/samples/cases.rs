//! §12.4 case card and receipt, §12.5 adding a case, §12.6 voiding,
//! context, and evidence, §12.12 lists.

use quack_core::action::{ActionFailure, ActionKind, ActionState, Failure};
use quack_core::case::{ContextProblem, ContextProblemCode, ContextValue};
use quack_core::rule::{FieldKey, FieldKind};

use super::{Out, Sample, covering, label, modal, msg, sample, text};
use crate::actions::DraftPage;
use crate::case::{self, AlreadySaved, FormField, PickerRule, VoidResult};
use crate::tests::fixtures::*;

const CARD_IDS: &[&str] = &[
    "case.card.heading",
    "case.card.heading_voided",
    "case.card.member",
    "case.card.member_imported",
    "case.card.reason",
    "case.card.action",
    "case.card.undo",
    "case.card.dm",
    "case.card.evidence.saving",
    "case.card.evidence.saved",
    "case.card.evidence.partial",
    "case.card.context_heading",
    "case.card.context_line",
    "case.card.voided",
    "case.card.void_reason",
    "case.card.replaces",
    "case.card.replaced_by",
    "case.card.appeal.pending",
    "case.card.appeal.needs_information",
    "case.card.appeal.accepted",
    "case.card.appeal.rejected",
    "case.card.appeal.closed",
    "case.card.controls.retry_dismiss",
    "case.card.controls.retry",
    "case.card.controls.dismiss",
    "case.card.trimmed",
    "case.card.imported_action",
    "case.card.footer",
    "case.card.footer_honeypot",
    "case.card.footer_imported",
    "case.button.edit_context",
    "case.button.add_evidence",
    "case.button.view_evidence",
    "case.button.history",
    "case.button.dashboard",
    "case.button.void",
    "case.button.retry",
    "case.button.dismiss",
    "case.button.remove_timeout",
    "case.button.unban",
];

const RECEIPT_IDS: &[&str] = &[
    "case.receipt.heading",
    "case.receipt.heading_voided",
    "case.receipt.member",
    "case.receipt.reason",
    "case.receipt.action",
    "case.receipt.staff_hint",
];

fn form_fields() -> Vec<FormField> {
    let f = |key: &str, label: &str, kind, required, value| FormField {
        key: FieldKey(key.to_owned()),
        label: label.to_owned(),
        kind,
        required,
        value,
    };
    vec![
        f("what", "What happened?", FieldKind::LongText, true, None),
        f(
            "short",
            &long(100),
            FieldKind::ShortText,
            false,
            Some(ContextValue::Text(long(600))),
        ),
        f(
            "repeat",
            "Repeat offence?",
            FieldKind::Boolean,
            false,
            Some(ContextValue::Boolean(false)),
        ),
        f("count", "Links posted", FieldKind::Number, false, None),
        f(
            "message",
            "Message link",
            FieldKind::MessageLink,
            true,
            None,
        ),
    ]
}

pub fn samples() -> Vec<Sample> {
    let mut v = Vec::new();
    for (name, c) in super::cards::cards() {
        v.push(covering(
            "case.card",
            CARD_IDS,
            name,
            Out::Msg(case::card(&c)),
        ));
        v.push(covering(
            "case.receipt",
            RECEIPT_IDS,
            name,
            Out::Msg(case::receipt(&c.receipt())),
        ));
    }
    let mut unconfirmed = super::cards::base_card();
    let mut a = super::cards::av(ActionKind::Ban, ActionState::Failed);
    a.failure = Some(Failure::new(ActionFailure::Unconfirmed));
    unconfirmed.action = Some(super::cards::card_action(a));
    v.push(sample(
        "case.receipt",
        "ban unconfirmed",
        Out::Msg(case::receipt(&unconfirmed.receipt())),
    ));
    v.push(sample(
        "case.card",
        "ban unconfirmed",
        Out::Msg(case::card(&unconfirmed)),
    ));
    for (name, retry, dismiss) in [
        ("ban unconfirmed, only Retry allowed", true, false),
        (
            "ban unconfirmed on a voided case, only Dismiss allowed",
            false,
            true,
        ),
    ] {
        let mut c = unconfirmed.clone();
        if let Some(a) = c.action.as_mut() {
            a.can_retry = retry;
            a.can_dismiss = dismiss;
        }
        v.push(sample("case.card", name, Out::Msg(case::card(&c))));
    }
    v.push(text("case.receipt.posted", "", case::receipt_posted(12)));
    v.push(text(
        "case.receipt.post_failed",
        "",
        case::receipt_post_failed(12),
    ));
    v.push(text(
        "case.receipt.replayed",
        "",
        case::receipt_replayed(12),
    ));
    v.push(text(
        "case.saved_unconfirmed",
        "",
        case::saved_unconfirmed(),
    ));

    let rules: Vec<PickerRule> = (0..25)
        .map(|i| PickerRule {
            id: rule_id(),
            name: format!("{} {i}", long(100)),
            description: long(500),
        })
        .collect();
    let one = [PickerRule {
        id: rule_id(),
        name: "Spam".to_owned(),
        description: "Unsolicited ads.".to_owned(),
    }];
    v.push(covering(
        "case.picker.prompt",
        &["case.picker.placeholder"],
        "one rule",
        Out::Msg(case::picker(&sam(), case_id(), &one, 1, 1)),
    ));
    v.push(sample(
        "case.picker.prompt",
        "maximal page 2 of 3",
        Out::Msg(case::picker(&sam(), case_id(), &rules, 2, 3)),
    ));
    v.push(text("case.picker.no_rules", "", case::picker_no_rules()));
    let page = DraftPage {
        draft: case_id(),
        rule: rule_id(),
        version: 3,
        page: 1,
    };
    v.push(covering(
        "case.picker.fill_form",
        &["case.draft.button.fill_form"],
        "",
        Out::Msg(case::picker_fill_form("Spam", &sam(), page)),
    ));
    v.push(label("case.form.title", case::form_title("Spam", 1, 2), 45));
    v.push(sample(
        "case.form.title",
        "one page",
        Out::Str(case::form_title("Spam", 1, 1), 45),
    ));
    v.push(sample(
        "case.form.title",
        "long rule name",
        Out::Str(case::form_title(&long(100), 2, 2), 45),
    ));
    v.push(covering(
        "case.form",
        &[
            "case.form.boolean_placeholder",
            "case.form.number_placeholder",
            "case.form.link_placeholder",
        ],
        "every field kind",
        Out::Modal(case::form_modal("Spam", page, 2, &form_fields())),
    ));
    v.push(modal(
        "case.form",
        "rule with no fields",
        case::form_modal(
            "Spam",
            page,
            1,
            &[FormField {
                key: FieldKey("details".to_owned()),
                label: case::DETAILS_LABEL.to_owned(),
                kind: FieldKind::LongText,
                required: false,
                value: None,
            }],
        ),
    ));
    let next = DraftPage { page: 2, ..page };
    v.push(covering(
        "case.draft.page_saved",
        &["case.draft.button.continue"],
        "",
        Out::Msg(case::draft_page_saved(1, 2, next)),
    ));
    // The real context problems, as the engine passes them.
    let problems = [
        case::context_problem(
            &ContextProblem {
                key: "what_happened".to_owned(),
                code: ContextProblemCode::Required,
            },
            Some("What happened?"),
            Some(FieldKind::LongText),
        ),
        case::context_problem(
            &ContextProblem {
                key: "links_posted".to_owned(),
                code: ContextProblemCode::NotANumber,
            },
            Some("Links posted"),
            Some(FieldKind::Number),
        ),
    ];
    v.push(covering(
        "case.draft.page_invalid",
        &["case.draft.button.try_again"],
        "",
        Out::Msg(case::draft_page_invalid(&problems, page)),
    ));
    v.push(msg(
        "case.draft.page_already_saved",
        "next page",
        case::draft_page_already_saved(AlreadySaved::Continue(next)),
    ));
    v.push(msg(
        "case.draft.page_already_saved",
        "case already added",
        case::draft_page_already_saved(AlreadySaved::Case(case_id())),
    ));
    v.push(text("case.draft.expired", "", case::draft_expired()));
    v.push(text(
        "case.draft.rule_changed",
        "",
        case::draft_rule_changed("Spam"),
    ));
    v.push(msg(
        "case.draft.save_failed",
        "internal error",
        case::draft_save_failed(crate::error::internal(), case_id()),
    ));
    for (id, code, kind) in [
        (
            "context.required",
            ContextProblemCode::Required,
            FieldKind::LongText,
        ),
        (
            "context.too_long",
            ContextProblemCode::TooLong,
            FieldKind::ShortText,
        ),
        (
            "context.too_long",
            ContextProblemCode::TooLong,
            FieldKind::LongText,
        ),
        (
            "context.not_text",
            ContextProblemCode::NotText,
            FieldKind::ShortText,
        ),
        (
            "context.not_yes_no",
            ContextProblemCode::NotYesNo,
            FieldKind::Boolean,
        ),
        (
            "context.not_a_number",
            ContextProblemCode::NotANumber,
            FieldKind::Number,
        ),
        (
            "context.number_out_of_range",
            ContextProblemCode::NumberOutOfRange,
            FieldKind::Number,
        ),
        (
            "context.not_a_message_link",
            ContextProblemCode::NotAMessageLink,
            FieldKind::MessageLink,
        ),
        (
            "context.other_server",
            ContextProblemCode::OtherServer,
            FieldKind::MessageLink,
        ),
        (
            "context.unknown_field",
            ContextProblemCode::UnknownField,
            FieldKind::ShortText,
        ),
        (
            "context.duplicate_field",
            ContextProblemCode::DuplicateField,
            FieldKind::ShortText,
        ),
    ] {
        let p = ContextProblem {
            key: "what_happened".to_owned(),
            code,
        };
        v.push(text(
            id,
            format!("{kind:?}"),
            case::context_problem(&p, Some("What happened?"), Some(kind)),
        ));
    }
    v.push(label("context.details_label", case::DETAILS_LABEL, 45));
    v.push(label("case.context.details_label", case::DETAILS_LABEL, 45));
    v.push(text(
        "member.context.link_saved",
        "",
        case::member_link_saved(),
    ));

    v.push(covering(
        "case.void.modal",
        &[
            "case.void.modal.title",
            "case.void.modal.label",
            "case.void.modal.description",
        ],
        "number known",
        Out::Modal(case::void_modal(case_id(), Some(12))),
    ));
    v.push(modal(
        "case.void.modal",
        "number unknown",
        case::void_modal(case_id(), None),
    ));
    v.push(label(
        "case.void.reason.appeal_accepted",
        case::void_reason_appeal_accepted(),
        1_000,
    ));
    let void_variants: Vec<(&str, Vec<VoidResult>, bool)> = vec![
        ("warning", vec![], false),
        (
            "timeout cancelled",
            vec![VoidResult::Cancelled(ActionKind::Timeout)],
            false,
        ),
        (
            "ban running",
            vec![VoidResult::Running(ActionKind::Ban), VoidResult::Unbanning],
            false,
        ),
        ("kick running", vec![VoidResult::RunningKick], false),
        ("kick done", vec![VoidResult::DoneKick], false),
        (
            "timeout undone, appeal closed",
            vec![VoidResult::RemovingTimeout],
            true,
        ),
        ("ban undone", vec![VoidResult::Unbanning], false),
    ];
    for (name, results, appeal) in void_variants {
        v.push(covering(
            "case.void.done",
            &[
                "case.void.undo.remove_timeout",
                "case.void.undo.unban",
                "case.void.cancelled_action",
                "case.void.check_outcome",
                "case.void.running_action",
                "case.void.running_kick",
                "case.void.done_kick",
                "case.void.appeal_closed",
            ],
            name,
            Out::Text(case::void_done(12, "Spam", &sam(), &results, appeal)),
        ));
    }

    v.push(modal(
        "case.context.modal.title",
        "",
        case::context_modal(case_id(), 12, 4, &form_fields()),
    ));
    v.push(text("case.context.saved", "", case::context_saved(12, 0)));
    v.push(text(
        "case.context.saved_new_evidence",
        "one",
        case::context_saved(12, 1),
    ));
    v.push(text(
        "case.context.saved_new_evidence",
        "other",
        case::context_saved(12, 3),
    ));
    v.push(msg(
        "case.context.too_many_fields",
        "",
        case::context_too_many_fields(12, URL),
    ));

    v
}
