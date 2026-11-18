//! §12.6 action sentences and controls, §12.7 DM outcomes, §12.8 DMs.

use quack_core::action::{ActionFailure, ActionKind, ActionSkipReason, ActionState, Failure};
use quack_core::dm::{DmFailure, DmState};
use quack_core::rule::LevelAction;

use super::cards::{av, context_lines};
use super::{Out, Sample, covering, label, msg, sample, text};
use crate::action::{self, ActionView, Detail, Undo};
use crate::dm::{self, AppealDm, AppealDmKind, CaseDm, DmView, RejoinAfter};
use crate::tests::fixtures::*;

/// Every action view worth a sentence: every kind in every state, every
/// failure reason, both skip reasons, no change, retries, and dismissal.
pub fn action_views() -> Vec<(String, ActionView)> {
    let mut v = Vec::new();
    for kind in ActionKind::ALL {
        for state in ActionState::ALL {
            let name = format!("{kind:?} {state:?}");
            match state {
                ActionState::Failed => {
                    for reason in ActionFailure::ALL {
                        let mut a = av(*kind, *state);
                        a.failure = Some(Failure {
                            permission: Some(kind.permission()),
                            ..Failure::new(*reason)
                        });
                        a.attempts = 8;
                        v.push((format!("{name} {reason:?}"), a.clone()));
                        if *reason == ActionFailure::DiscordRefused {
                            a.dismissed = Some((ana(), later()));
                            v.push((format!("{name} {reason:?} dismissed"), a));
                        }
                    }
                }
                ActionState::Skipped => {
                    for reason in ActionSkipReason::ALL {
                        let mut a = av(*kind, *state);
                        a.skip = Some(*reason);
                        a.still_applies_case = Some(7);
                        v.push((format!("{name} {reason:?}"), a));
                    }
                }
                ActionState::Done => {
                    v.push((name.clone(), av(*kind, *state)));
                    let mut a = av(*kind, *state);
                    a.no_change = true;
                    v.push((format!("{name} no change"), a));
                    if *kind == ActionKind::Ban {
                        let mut a = av(*kind, *state);
                        a.ban_delete_message_seconds = Some(86_400);
                        v.push((format!("{name} deleting a day of messages"), a));
                    }
                }
                ActionState::Pending => {
                    v.push((name.clone(), av(*kind, *state)));
                    let mut a = av(*kind, *state);
                    a.attempts = 2;
                    a.next_attempt_at = Some(later());
                    v.push((format!("{name} retry scheduled"), a));
                }
                ActionState::Running | ActionState::Cancelled => v.push((name, av(*kind, *state))),
            }
        }
    }
    v
}

const ACTION_IDS: &[&str] = &[
    "action.retry_scheduled",
    "action.unconfirmed",
    "action.timeout.in_progress",
    "action.timeout.done",
    "action.timeout.done_no_change",
    "action.timeout.failed",
    "action.timeout.cancelled",
    "action.kick.in_progress",
    "action.kick.done",
    "action.kick.done_no_change",
    "action.kick.failed",
    "action.kick.cancelled",
    "action.ban.in_progress",
    "action.ban.done",
    "action.ban.done_delete",
    "action.ban.done_no_change",
    "action.ban.failed",
    "action.ban.cancelled",
    "action.remove_timeout.in_progress",
    "action.remove_timeout.done",
    "action.remove_timeout.done_no_change",
    "action.remove_timeout.failed",
    "action.remove_timeout.skipped",
    "action.remove_timeout.skipped_changed",
    "action.unban.in_progress",
    "action.unban.done",
    "action.unban.done_no_change",
    "action.unban.failed",
    "action.unban.skipped",
    "action.unban.skipped_changed",
    "action.dismissed_suffix",
    "action.failure.quack_not_in_server",
    "action.failure.quack_missing_permission",
    "action.failure.quack_role_too_low",
    "action.failure.member_is_owner",
    "action.failure.member_not_in_server",
    "action.failure.member_is_administrator",
    "action.failure.timeout_window_passed",
    "action.failure.discord_refused",
    "action.failure.retries_exhausted",
];

const MEMBER_ACTION_IDS: &[&str] = &[
    "member.action.timeout.in_progress",
    "member.action.timeout.done",
    "member.action.unconfirmed",
    "member.action.timeout.not_applied",
    "member.action.kick.in_progress",
    "member.action.kick.done",
    "member.action.kick.not_applied",
    "member.action.ban.in_progress",
    "member.action.ban.done",
    "member.action.ban.not_applied",
    "member.action.remove_timeout.done",
    "member.action.remove_timeout.in_progress",
    "member.action.remove_timeout.skipped",
    "member.action.remove_timeout.skipped_changed",
    "member.action.remove_timeout.not_applied",
    "member.action.unban.done",
    "member.action.unban.in_progress",
    "member.action.unban.skipped",
    "member.action.unban.skipped_changed",
    "member.action.unban.not_applied",
];

pub fn dm_views() -> Vec<(String, Option<DmView>)> {
    let mut v = vec![("level sends no DM".to_owned(), None)];
    for state in DmState::ALL {
        if *state == DmState::Failed {
            for f in DmFailure::ALL {
                v.push((
                    format!("{state:?} {f:?}"),
                    Some(DmView {
                        state: *state,
                        failure: Some(*f),
                    }),
                ));
            }
        } else {
            v.push((
                format!("{state:?}"),
                Some(DmView {
                    state: *state,
                    failure: None,
                }),
            ));
        }
    }
    v
}

pub fn samples() -> Vec<Sample> {
    let mut v = Vec::new();
    v.push(text("action.none", "", action::none()));
    v.push(text("member.action.none", "", action::member_none()));
    for (name, a) in action_views() {
        v.push(covering(
            "action.sentence",
            ACTION_IDS,
            name.clone(),
            Out::Text(action::sentence(&a, &sam(), Detail::Full)),
        ));
        v.push(sample(
            "action.sentence.receipt",
            name.clone(),
            Out::Text(action::sentence(&a, &sam(), Detail::Receipt)),
        ));
        v.push(covering(
            "member.action",
            MEMBER_ACTION_IDS,
            name,
            Out::Text(action::member_sentence(&a)),
        ));
    }
    let mut no_end = ActionView::new(ActionKind::Timeout, ActionState::Running, 12);
    no_end.timeout_ends_at = None;
    v.push(sample(
        "action.sentence",
        "Timeout Running without an end time",
        Out::Text(action::sentence(&no_end, &sam(), Detail::Full)),
    ));
    for kind in ActionKind::ALL {
        v.push(text(
            "action.retry.done",
            format!("{kind:?}"),
            action::retry_done(*kind, 12),
        ));
        v.push(text(
            "action.dismiss.done",
            format!("{kind:?}"),
            action::dismiss_done(*kind, 12),
        ));
    }
    for (undo, confirm, button, queued) in [
        (
            Undo::RemoveTimeout,
            "action.undo.confirm.remove_timeout",
            "action.undo.button.remove_timeout",
            "action.undo.queued.remove_timeout",
        ),
        (
            Undo::Unban,
            "action.undo.confirm.unban",
            "action.undo.button.unban",
            "action.undo.queued.unban",
        ),
    ] {
        v.push(text(confirm, "", action::undo_confirm(undo, &sam(), 12)));
        v.push(label(button, action::undo_button(undo), 80));
        v.push(text(queued, "", action::undo_queued(undo, &sam(), 12)));
    }

    for (name, d) in dm_views() {
        v.push(covering(
            "dm.state",
            &[
                "dm.state.in_progress",
                "dm.state.sent",
                "dm.state.failed",
                "dm.state.unknown",
                "dm.state.not_needed.level",
                "dm.state.not_needed.voided",
                "dm.failure.dms_closed",
                "dm.failure.not_in_shared_server",
                "dm.failure.discord_unavailable",
                "dm.failure.discord_refused",
            ],
            name.clone(),
            Out::Text(dm::sentence(d.as_ref(), &sam())),
        ));
        v.push(covering(
            "member.dm",
            &[
                "member.dm.in_progress",
                "member.dm.sent",
                "member.dm.failed",
                "member.dm.unknown",
                "member.dm.not_needed",
            ],
            name,
            Out::Text(dm::member_sentence(d.as_ref())),
        ));
    }
    v.extend(dm_samples());
    v
}

fn case_dm() -> CaseDm {
    CaseDm {
        case: case_id(),
        number: 12,
        created_at: t0(),
        server: "Duck Pond".to_owned(),
        rule: "Spam".to_owned(),
        reason: "Posting spam or unsolicited advertising.".to_owned(),
        intro: Some("Please read the **rules** in <#1>. @everyone".to_owned()),
        context: context_lines(),
        action: None,
        timeout_ends_at: None,
        appealable: true,
        footer: Some("Questions? Open a ticket.".to_owned()),
        dashboard_url: format!("{URL}/appeal"),
    }
}

const DM_CASE_IDS: &[&str] = &[
    "dm.case.heading",
    "dm.case.rule",
    "dm.case.reason",
    "dm.case.intro",
    "dm.case.details_heading",
    "dm.case.detail_line",
    "dm.case.outcome.warning",
    "dm.case.outcome.timeout",
    "dm.case.outcome.kick",
    "dm.case.outcome.ban",
    "dm.case.appeal",
    "dm.case.not_appealable",
    "dm.case.footer_text",
    "dm.case.subtext",
    "dm.case.button.appeal",
    "dm.case.button.dashboard",
];

fn dm_samples() -> Vec<Sample> {
    let mut v = Vec::new();
    v.push(covering(
        "dm.case",
        DM_CASE_IDS,
        "warning, appealable, intro and footer",
        Out::Msg(dm::case(&case_dm())),
    ));
    v.push(sample(
        "dm.case",
        "timeout",
        Out::Msg(dm::case(&CaseDm {
            action: Some(LevelAction::Timeout {
                duration_seconds: 3_600,
            }),
            timeout_ends_at: Some(later()),
            context: vec![],
            intro: None,
            footer: None,
            ..case_dm()
        })),
    ));
    v.push(sample(
        "dm.case",
        "kick, not appealable",
        Out::Msg(dm::case(&CaseDm {
            action: Some(LevelAction::Kick),
            appealable: false,
            ..case_dm()
        })),
    ));
    v.push(sample(
        "dm.case",
        "ban",
        Out::Msg(dm::case(&CaseDm {
            action: Some(LevelAction::Ban {
                delete_message_seconds: 86_400,
            }),
            ..case_dm()
        })),
    ));
    v.push(sample(
        "dm.case",
        "maximal",
        Out::Msg(dm::case(&CaseDm {
            server: long(100),
            rule: long(100),
            reason: long(1_000),
            intro: Some(long(1_000)),
            footer: Some(long(500)),
            context: (0..10)
                .map(|i| crate::case::ContextLine {
                    label: format!("{} {i}", long(98)),
                    ..context_lines()[0].clone()
                })
                .map(|mut l| {
                    l.value = quack_core::case::ContextValue::Text(long(4_000));
                    l
                })
                .collect(),
            ..case_dm()
        })),
    ));
    let appeal = |kind| AppealDm {
        kind,
        server: "Duck Pond".to_owned(),
        number: 12,
        reason: "We reviewed it.".to_owned(),
        at: later(),
        dashboard_url: format!("{URL}/appeal"),
    };
    for (id, kind) in [
        (
            "dm.appeal.information_requested",
            AppealDmKind::InformationRequested,
        ),
        ("dm.appeal.reopened", AppealDmKind::Reopened),
        ("dm.appeal.accepted", AppealDmKind::Accepted(None)),
        (
            "dm.appeal.accepted.rejoin_ban",
            AppealDmKind::Accepted(Some((
                RejoinAfter::Ban,
                "https://discord.gg/abc123".to_owned(),
            ))),
        ),
        (
            "dm.appeal.accepted.rejoin_kick",
            AppealDmKind::Accepted(Some((
                RejoinAfter::Kick,
                "https://discord.gg/abc123".to_owned(),
            ))),
        ),
        ("dm.appeal.rejected", AppealDmKind::Rejected),
        ("dm.appeal.closed", AppealDmKind::Closed),
        (
            "dm.appeal.closed_case_voided",
            AppealDmKind::ClosedCaseVoided,
        ),
    ] {
        v.push(covering(
            id,
            &["dm.appeal.button", "dm.appeal.button.reply"],
            "",
            Out::Msg(dm::appeal(&appeal(kind))),
        ));
    }
    v.push(msg(
        "dm.appeal.rejected",
        "maximal",
        dm::appeal(&AppealDm {
            server: long(100),
            reason: long(2_000),
            ..appeal(AppealDmKind::Rejected)
        }),
    ));
    v
}
