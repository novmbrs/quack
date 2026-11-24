//! §12.9 appeals and the member timeline.

use quack_core::appeal::{AppealEventKind, AppealState};

use super::{Out, Sample, covering, msg, sample, text};
use crate::appeal::{self, AppealPost, AppealRow, ModalDecision};
use crate::model::Actor;
use crate::tests::fixtures::*;
use crate::timeline;

fn post() -> AppealPost {
    AppealPost {
        appeal: appeal_id(),
        version: 3,
        number: 12,
        rule: "Spam".to_owned(),
        member: sam(),
        submitted_at: t0(),
        state: AppealState::Pending,
        decided: None,
        closed_by_void: false,
        statement: "I posted that link by mistake.".to_owned(),
        information: None,
        decision_reason: None,
        dashboard_url: URL.to_owned(),
    }
}

const POST_IDS: &[&str] = &[
    "appeal.post.heading",
    "appeal.post.from",
    "appeal.post.state.pending",
    "appeal.post.state.needs_information",
    "appeal.post.state.accepted",
    "appeal.post.state.rejected",
    "appeal.post.state.closed",
    "appeal.post.state.closed_case_voided",
    "appeal.post.statement_heading",
    "appeal.post.statement",
    "appeal.post.statement_cut",
    "appeal.post.information_heading",
    "appeal.post.reason_heading",
    "appeal.button.accept",
    "appeal.button.reject",
    "appeal.button.open",
    "appeal.post.trimmed",
];

pub fn samples() -> Vec<Sample> {
    let mut v = Vec::new();
    v.push(covering(
        "appeal.form",
        &[
            "appeal.form.title",
            "appeal.form.label",
            "appeal.form.description",
        ],
        "",
        Out::Modal(appeal::form(case_id())),
    ));
    v.push(text("appeal.submitted", "", appeal::submitted(12)));
    v.push(text("appeal.submit.exists", "", appeal::submit_exists(12)));
    v.push(text(
        "appeal.submit.not_allowed",
        "",
        appeal::submit_not_allowed(12),
    ));
    v.push(text("appeal.submit.voided", "", appeal::submit_voided(12)));
    v.push(text(
        "appeal.submit.not_yours",
        "",
        appeal::submit_not_yours(),
    ));
    v.push(text(
        "appeal.information.saved",
        "",
        appeal::information_saved(12),
    ));
    v.push(text(
        "appeal.information.not_requested",
        "",
        appeal::information_not_requested(),
    ));
    v.push(text(
        "appeal.information.outdated",
        "",
        appeal::information_outdated(),
    ));

    let decided = Some((ana(), later()));
    let posts = vec![
        ("pending", post()),
        (
            "needs information",
            AppealPost {
                state: AppealState::NeedsInformation,
                decided: decided.clone(),
                decision_reason: Some("Which link?".to_owned()),
                ..post()
            },
        ),
        (
            "accepted",
            AppealPost {
                state: AppealState::Accepted,
                decided: decided.clone(),
                information: Some("It was the wrong link.".to_owned()),
                decision_reason: Some(
                    "We reviewed your appeal. The case no longer counts.".to_owned(),
                ),
                ..post()
            },
        ),
        (
            "rejected",
            AppealPost {
                state: AppealState::Rejected,
                decided: decided.clone(),
                decision_reason: Some("We reviewed your appeal. The case still counts.".to_owned()),
                ..post()
            },
        ),
        (
            "closed",
            AppealPost {
                state: AppealState::Closed,
                decided: decided.clone(),
                decision_reason: Some("Closing this.".to_owned()),
                ..post()
            },
        ),
        (
            "closed by void",
            AppealPost {
                state: AppealState::Closed,
                decided: Some((Actor::Quack, later())),
                closed_by_void: true,
                decision_reason: Some("Closed because the case was voided.".to_owned()),
                ..post()
            },
        ),
        (
            "maximal",
            AppealPost {
                rule: long(100),
                statement: long(4_000),
                information: Some(long(4_000)),
                decision_reason: Some(long(2_000)),
                state: AppealState::Rejected,
                decided,
                ..post()
            },
        ),
    ];
    for (name, p) in posts {
        v.push(covering(
            "appeal.post",
            POST_IDS,
            name,
            Out::Msg(appeal::post(&p)),
        ));
    }
    v.push(msg(
        "appeal.post.moved",
        "",
        appeal::post_moved(&channel("appeals")),
    ));
    v.push(covering(
        "appeal.modal.accept",
        &[
            "appeal.modal.accept.title",
            "appeal.modal.reason.label",
            "appeal.modal.reason.description",
            "appeal.reason.default.accept",
        ],
        "",
        Out::Modal(appeal::decision_modal(
            ModalDecision::Accept,
            appeal_id(),
            3,
        )),
    ));
    v.push(covering(
        "appeal.modal.reject",
        &["appeal.modal.reject.title", "appeal.reason.default.reject"],
        "",
        Out::Modal(appeal::decision_modal(
            ModalDecision::Reject,
            appeal_id(),
            3,
        )),
    ));
    v.push(text(
        "appeal.decided.accepted",
        "",
        appeal::decided(ModalDecision::Accept, 12, &sam()),
    ));
    v.push(text(
        "appeal.decided.rejected",
        "",
        appeal::decided(ModalDecision::Reject, 12, &sam()),
    ));
    v.push(text("appeal.closed_by_void", "", appeal::closed_by_void()));
    for kind in AppealEventKind::ALL {
        for (who, actor) in [("staff", ana()), ("Quack", Actor::Quack)] {
            v.push(text(
                "appeal.event",
                format!("{kind:?} by {who}, staff view"),
                appeal::event(*kind, Some(&actor), &sam(), false),
            ));
            v.push(text(
                "appeal.event",
                format!("{kind:?} by {who}, member view"),
                appeal::event(*kind, Some(&actor), &sam(), true),
            ));
        }
    }
    let row = AppealRow {
        appeal: appeal_id(),
        number: 12,
        member: sam(),
        rule: "Spam".to_owned(),
        submitted_at: t0(),
    };
    v.push(covering(
        "appeals",
        &["appeals.heading", "appeals.row", "appeals.button.open"],
        "one",
        Out::Msg(appeal::list(std::slice::from_ref(&row), 1, 1)),
    ));
    let rows: Vec<AppealRow> = (0..10)
        .map(|i| AppealRow {
            appeal: quack_core::ids::AppealId::from_uuid(uuid::Uuid::from_u128(
                0x0193_0000_0000_7000_8000_0000_0000_0100 + u128::from(i),
            )),
            number: 9_007_199_254_740_991 - i,
            rule: long(100),
            ..row.clone()
        })
        .collect();
    v.push(sample(
        "appeals",
        "maximal page",
        Out::Msg(appeal::list(&rows, 2, 3)),
    ));
    v.push(covering(
        "appeals.empty",
        &[],
        "",
        Out::Msg(appeal::list(&[], 1, 1)),
    ));
    v.push(text("timeline.case_added", "", timeline::case_added(12)));
    v.push(text(
        "timeline.case_voided",
        "",
        timeline::case_voided("Wrong member."),
    ));
    v
}
