//! Case cards in every interesting state (§12.4).

use quack_core::action::{ActionFailure, ActionKind, ActionState, Failure};
use quack_core::appeal::AppealState;
use quack_core::case::{CaseSource, ContextValue, Decimal};
use quack_core::dm::{DmFailure, DmState};
use quack_core::permissions::DiscordPermission;
use quack_core::rule::FieldKind;

use crate::action::ActionView;
use crate::case::{CardAction, CaseCard, ContextLine, EvidenceSummary, Imported, Voided};
use crate::dm::DmView;
use crate::tests::fixtures::*;

pub fn av(kind: ActionKind, state: ActionState) -> ActionView {
    let mut a = ActionView::new(kind, state, 12);
    a.timeout_ends_at = Some(later());
    a
}

pub fn card_action(view: ActionView) -> CardAction {
    CardAction {
        id: action_id(),
        view,
        can_retry: false,
        can_dismiss: false,
        can_undo: false,
    }
}

pub fn base_card() -> CaseCard {
    CaseCard {
        case: case_id(),
        number: 12,
        rule: "Spam".to_owned(),
        member: sam(),
        member_id: member_id(),
        source: CaseSource::Discord,
        level_index: 1,
        level_from: 1,
        case_count: 1,
        reason: "Posting spam or unsolicited advertising.".to_owned(),
        action: None,
        undo: None,
        dm: Some(DmView {
            state: DmState::Sent,
            failure: None,
        }),
        evidence: EvidenceSummary::default(),
        context: Vec::new(),
        voided: None,
        replaces: None,
        replaced_by: None,
        appeal: None,
        moderator: Some(mod_ana()),
        created_at: t0(),
        imported: None,
        dashboard_url: URL.to_owned(),
        failures_url: "https://quack.example/guilds/111/failures".to_owned(),
    }
}

pub fn context() -> Vec<ContextLine> {
    vec![
        ContextLine {
            label: "What happened?".to_owned(),
            kind: FieldKind::LongText,
            value: ContextValue::Text("Posted invite links in #general.".to_owned()),
        },
        ContextLine {
            label: "Repeat offence?".to_owned(),
            kind: FieldKind::Boolean,
            value: ContextValue::Boolean(true),
        },
        ContextLine {
            label: "Links posted".to_owned(),
            kind: FieldKind::Number,
            value: ContextValue::Number(Decimal::parse("2.5").unwrap()),
        },
        ContextLine {
            label: "Message link".to_owned(),
            kind: FieldKind::MessageLink,
            value: ContextValue::Text("https://discord.com/channels/1/2/3".to_owned()),
        },
    ]
}

pub fn context_lines() -> Vec<ContextLine> {
    context()
}

pub fn cards() -> Vec<(&'static str, CaseCard)> {
    let mut timeout = base_card();
    timeout.level_index = 2;
    timeout.level_from = 3;
    timeout.case_count = 3;
    timeout.action = Some(card_action(av(ActionKind::Timeout, ActionState::Done)));
    timeout.action.as_mut().unwrap().can_undo = true;
    timeout.dm = Some(DmView {
        state: DmState::Failed,
        failure: Some(DmFailure::DmsClosed),
    });
    timeout.evidence = EvidenceSummary {
        saving: 0,
        saved: 3,
        failed: 0,
    };
    timeout.context = context();
    timeout.appeal = Some(AppealState::Pending);
    timeout.replaced_by = Some(13);

    let mut ban_failed = base_card();
    let mut failed = av(ActionKind::Ban, ActionState::Failed);
    failed.failure = Some(Failure::missing_permission(DiscordPermission::BanMembers));
    failed.attempts = 1;
    ban_failed.action = Some(CardAction {
        can_retry: true,
        can_dismiss: true,
        ..card_action(failed)
    });
    ban_failed.dm = Some(DmView {
        state: DmState::Unknown,
        failure: None,
    });
    ban_failed.evidence = EvidenceSummary {
        saving: 0,
        saved: 2,
        failed: 1,
    };
    ban_failed.appeal = Some(AppealState::NeedsInformation);

    let mut voided = base_card();
    voided.action = Some(card_action(av(ActionKind::Ban, ActionState::Done)));
    let mut unban = av(ActionKind::Unban, ActionState::Pending);
    unban.attempts = 2;
    unban.next_attempt_at = Some(later());
    voided.undo = Some(card_action(unban));
    voided.voided = Some(Voided {
        by: ana(),
        at: later(),
        reason: "Wrong member.".to_owned(),
    });
    voided.replaces = Some(9);
    voided.appeal = Some(AppealState::Closed);
    voided.evidence = EvidenceSummary {
        saving: 1,
        saved: 1,
        failed: 0,
    };

    let mut honeypot = base_card();
    honeypot.source = CaseSource::Honeypot;
    honeypot.moderator = None;
    honeypot.action = Some(card_action(av(ActionKind::Kick, ActionState::Done)));
    honeypot.dm = None;

    let mut imported = base_card();
    imported.source = CaseSource::Imported;
    imported.moderator = None;
    imported.dm = None;
    imported.imported = Some(Imported {
        v4_action: "ban".to_owned(),
        v4_moderator: "Oldtimer#0001".to_owned(),
    });

    let mut maximal = base_card();
    maximal.rule = long(100);
    maximal.reason = long(1_000);
    maximal.context = (0..10)
        .map(|i| ContextLine {
            label: format!("{} {i}", long(98)),
            kind: FieldKind::LongText,
            value: ContextValue::Text(long(4_000)),
        })
        .collect();
    let mut failed_undo = av(ActionKind::RemoveTimeout, ActionState::Failed);
    failed_undo.failure = Some(Failure::new(ActionFailure::RetriesExhausted));
    failed_undo.attempts = 8;
    maximal.action = Some(CardAction {
        can_undo: false,
        ..card_action(av(ActionKind::Timeout, ActionState::Done))
    });
    maximal.undo = Some(CardAction {
        can_retry: true,
        can_dismiss: true,
        ..card_action(failed_undo)
    });
    maximal.voided = Some(Voided {
        by: ana(),
        at: later(),
        reason: long(1_000),
    });
    maximal.evidence = EvidenceSummary {
        saving: 0,
        saved: 49,
        failed: 1,
    };
    maximal.appeal = Some(AppealState::Rejected);
    maximal.replaces = Some(9_007_199_254_740_991);
    maximal.replaced_by = Some(9_007_199_254_740_990);

    let mut hostile = base_card();
    hostile.rule = HOSTILE.to_owned();
    hostile.reason = HOSTILE.to_owned();
    hostile.member = crate::model::Person::new(account(222), HOSTILE);
    hostile.context = vec![ContextLine {
        label: HOSTILE.to_owned(),
        kind: FieldKind::ShortText,
        value: ContextValue::Text(HOSTILE.to_owned()),
    }];

    vec![
        ("warning, DM sent", base_card()),
        (
            "timeout done, DM failed, context, appeal pending, replaced",
            timeout,
        ),
        (
            "ban failed (missing permission), DM unknown, partial evidence",
            ban_failed,
        ),
        ("voided ban, unban retrying, appeal closed", voided),
        ("honeypot kick", honeypot),
        ("imported", imported),
        ("maximal", maximal),
        ("hostile text", hostile),
    ]
}
