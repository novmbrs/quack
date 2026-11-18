use time::macros::datetime;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use super::*;
use crate::DomainError;
use crate::ids::{CaseId, MemberId};
use crate::permissions::{BanFacts, DiscordPermission, MemberFacts, Permission, Permissions};

mod props;

const NOW: OffsetDateTime = datetime!(2026-11-17 12:00 UTC);

fn settled(
    outcome: AttemptOutcome,
    changed_outside_case: bool,
    case_valid: bool,
    attempts_left: bool,
) -> ActionEvent {
    ActionEvent::Settled {
        outcome,
        changed_outside_case,
        case_valid,
        attempts_left,
    }
}

fn all_events() -> Vec<ActionEvent> {
    let mut events = vec![
        ActionEvent::CaseVoided,
        ActionEvent::OtherCaseApplies,
        ActionEvent::StartRefused,
        ActionEvent::AttemptStarted,
        ActionEvent::StaffRetry,
        ActionEvent::StaffDismiss,
    ];
    for &outcome in AttemptOutcome::ALL {
        for changed in [false, true] {
            for valid in [true, false] {
                for left in [true, false] {
                    events.push(settled(outcome, changed, valid, left));
                }
            }
        }
    }
    events
}

/// The design's table (§5.3), written out row by row; settle rows in the
/// table's order.
#[allow(clippy::match_same_arms, reason = "one arm per table row")]
fn expected(kind: ActionKind, state: ActionState, event: ActionEvent) -> Option<ActionState> {
    use ActionEvent as E;
    use ActionState as S;
    use AttemptOutcome as O;
    let original = kind.is_original();
    match (state, event) {
        (S::Pending, E::CaseVoided) => original.then_some(S::Cancelled),
        (S::Pending, E::OtherCaseApplies) => (!original).then_some(S::Skipped),
        (S::Pending, E::StartRefused) => Some(S::Failed),
        (S::Pending, E::AttemptStarted) => Some(S::Running),
        (
            S::Running,
            E::Settled {
                outcome,
                changed_outside_case,
                case_valid,
                attempts_left,
            },
        ) => {
            if changed_outside_case {
                // Only an undo's provenance read reports this, as no_change.
                return (!original && outcome == O::NoChange).then_some(S::Skipped);
            }
            Some(match outcome {
                O::Done | O::NoChange => S::Done,
                O::Refused => S::Failed,
                _ if original && !case_valid => S::Cancelled,
                _ if attempts_left => S::Pending,
                _ => S::Failed,
            })
        }
        (S::Failed, E::StaffRetry) => Some(S::Pending),
        (S::Failed, E::StaffDismiss) => Some(S::Failed),
        _ => None,
    }
}

#[test]
fn transition_matches_the_table_for_every_pair() {
    let mut allowed = 0;
    for &kind in ActionKind::ALL {
        for &state in ActionState::ALL {
            for event in all_events() {
                let got = transition(kind, state, event);
                if let Some(to) = expected(kind, state, event) {
                    allowed += 1;
                    assert_eq!(got, Ok(to), "{kind:?} {state:?} {event:?}");
                } else {
                    let err = got.unwrap_err();
                    assert_eq!(
                        (err.machine, err.from, err.event),
                        ("action", state.as_str(), event.name())
                    );
                }
            }
        }
    }
    assert!(allowed > 0);
}

#[test]
fn settle_rows_are_tried_in_order() {
    // A refused attempt on a voided case fails (row 3 before row 4).
    assert_eq!(
        transition(
            ActionKind::Ban,
            ActionState::Running,
            settled(AttemptOutcome::Refused, false, false, true)
        ),
        Ok(ActionState::Failed)
    );
    // A done attempt on a voided case is done (row 1 before row 4).
    assert_eq!(
        transition(
            ActionKind::Ban,
            ActionState::Running,
            settled(AttemptOutcome::Done, false, false, true)
        ),
        Ok(ActionState::Done)
    );
    // An unknown attempt on a voided case is cancelled, budget or not.
    for left in [true, false] {
        assert_eq!(
            transition(
                ActionKind::Timeout,
                ActionState::Running,
                settled(AttemptOutcome::Unknown, false, false, left)
            ),
            Ok(ActionState::Cancelled)
        );
    }
    // An undo on its (voided) case keeps retrying.
    assert_eq!(
        transition(
            ActionKind::Unban,
            ActionState::Running,
            settled(AttemptOutcome::NotSent, false, false, true)
        ),
        Ok(ActionState::Pending)
    );
}

#[test]
fn terminal_states_accept_nothing_and_kinds_keep_their_end_states() {
    for &kind in ActionKind::ALL {
        for state in [
            ActionState::Done,
            ActionState::Cancelled,
            ActionState::Skipped,
        ] {
            assert!(state.is_terminal());
            for event in all_events() {
                assert!(
                    transition(kind, state, event).is_err(),
                    "{kind:?} {state:?} {event:?}"
                );
            }
        }
        for &state in ActionState::ALL {
            for event in all_events() {
                match transition(kind, state, event) {
                    Ok(ActionState::Skipped) => assert!(!kind.is_original(), "{kind:?}"),
                    Ok(ActionState::Cancelled) => assert!(kind.is_original(), "{kind:?}"),
                    _ => {}
                }
            }
        }
    }
    assert!(!ActionState::Failed.is_terminal());
}

#[test]
fn enum_names_match_the_database() {
    let names = |all: &[&str]| all.join(",");
    assert_eq!(
        names(
            &ActionKind::ALL
                .iter()
                .map(|v| v.as_str())
                .collect::<Vec<_>>()
        ),
        "timeout,kick,ban,remove_timeout,unban"
    );
    assert_eq!(
        names(
            &ActionState::ALL
                .iter()
                .map(|v| v.as_str())
                .collect::<Vec<_>>()
        ),
        "pending,running,done,failed,cancelled,skipped"
    );
    assert_eq!(
        names(
            &ActionFailure::ALL
                .iter()
                .map(|v| v.as_str())
                .collect::<Vec<_>>()
        ),
        "quack_not_in_server,quack_missing_permission,quack_role_too_low,member_is_owner,\
         member_not_in_server,member_is_administrator,timeout_window_passed,discord_refused,\
         retries_exhausted,unconfirmed"
    );
    assert_eq!(
        names(
            &ActionSkipReason::ALL
                .iter()
                .map(|v| v.as_str())
                .collect::<Vec<_>>()
        ),
        "other_case_applies,changed_outside_case"
    );
    assert_eq!(
        names(
            &AttemptOutcome::ALL
                .iter()
                .map(|v| v.as_str())
                .collect::<Vec<_>>()
        ),
        "done,no_change,refused,not_sent,unknown,interrupted"
    );
}

#[test]
fn kinds() {
    assert_eq!(
        ActionKind::Timeout.undo_kind(),
        Some(ActionKind::RemoveTimeout)
    );
    assert_eq!(ActionKind::Ban.undo_kind(), Some(ActionKind::Unban));
    assert_eq!(ActionKind::Kick.undo_kind(), None);
    assert_eq!(ActionKind::Unban.undoes(), Some(ActionKind::Ban));
    assert_eq!(ActionKind::Kick.undoes(), None);
    assert_eq!(ActionKind::RemoveTimeout.purpose(), ActionPurpose::Undo);
    assert_eq!(
        ActionKind::Unban.permission(),
        DiscordPermission::BanMembers
    );
    assert_eq!(
        ActionKind::RemoveTimeout.permission(),
        DiscordPermission::ModerateMembers
    );
    assert!(ActionKind::RemoveTimeout.reads_member());
    assert!(!ActionKind::Unban.reads_member());
}

#[test]
fn may_have_applied_rule() {
    use AttemptOutcome as O;
    assert!(!may_have_applied(&[]));
    assert!(may_have_applied(&[None]));
    for o in [O::Done, O::Unknown, O::Interrupted] {
        assert!(may_have_applied(&[Some(O::NotSent), Some(o)]), "{o:?}");
    }
    assert!(!may_have_applied(&[
        Some(O::NoChange),
        Some(O::Refused),
        Some(O::NotSent)
    ]));
}

#[test]
fn retry_schedule() {
    let expected = [5, 15, 60, 300, 900, 3600, 10_800];
    for (n, secs) in (1..=7).zip(expected) {
        assert_eq!(
            retry_delay(n, None),
            Some(Duration::seconds(secs)),
            "attempt {n}"
        );
    }
    assert_eq!(retry_delay(8, None), None);
    assert_eq!(
        retry_delay(1, Some(Duration::seconds(30))),
        Some(Duration::seconds(30))
    );
    assert_eq!(
        retry_delay(3, Some(Duration::seconds(30))),
        Some(Duration::minutes(1))
    );
    let total: Duration = RETRY_BACKOFF.iter().copied().sum();
    // The design says "≈ 4 h 36 min"; the listed waits add up to 4 h 21 min 20 s.
    assert_eq!(total, Duration::seconds(15_680));
}

#[test]
fn timeout_end_is_whole_seconds_and_the_window_is_exact() {
    let created = datetime!(2026-11-17 12:00:00.75 UTC);
    let end = timeout_ends_at(created, 60);
    assert_eq!(end, datetime!(2026-11-17 12:01 UTC));
    assert!(!timeout_window_passed(end - Duration::milliseconds(1), end));
    assert!(timeout_window_passed(end, end));
}

fn facts(kind: ActionKind, state: ActionState) -> StartFacts {
    StartFacts {
        kind,
        state,
        next_attempt_at: None,
        member_busy: false,
        case_valid: true,
        case_created_at: NOW,
        dm_waiting: false,
        still_applies: None,
        precheck: Ok(()),
        timeout_ends_at: (kind == ActionKind::Timeout).then_some(NOW + Duration::hours(1)),
        now: NOW,
    }
}

fn still() -> StillApplies {
    StillApplies {
        case_id: CaseId::from_uuid(Uuid::from_u128(9)),
        case_number: 9,
    }
}

#[test]
fn start_order() {
    use StartDecision as D;
    for state in [
        ActionState::Done,
        ActionState::Failed,
        ActionState::Cancelled,
        ActionState::Skipped,
    ] {
        assert_eq!(start(&facts(ActionKind::Ban, state)), D::Finish);
    }
    assert_eq!(
        start(&facts(ActionKind::Ban, ActionState::Running)),
        D::SettleInterrupted
    );
    let pending = facts(ActionKind::Ban, ActionState::Pending);
    assert_eq!(start(&pending), D::Open);

    let later = NOW + Duration::minutes(1);
    let not_due = StartFacts {
        next_attempt_at: Some(later),
        member_busy: true,
        case_valid: false,
        ..pending
    };
    assert_eq!(start(&not_due), D::RetryAt(later));
    let due = StartFacts {
        next_attempt_at: Some(NOW),
        ..pending
    };
    assert_eq!(start(&due), D::Open);

    let busy = StartFacts {
        member_busy: true,
        dm_waiting: true,
        case_valid: false,
        ..pending
    };
    assert_eq!(start(&busy), D::DeferUntil(NOW + MEMBER_BUSY_DEFER));

    let waiting = StartFacts {
        dm_waiting: true,
        case_valid: false,
        now: NOW + Duration::seconds(30),
        ..pending
    };
    assert_eq!(start(&waiting), D::DeferUntil(NOW + Duration::seconds(120)));
    // Never past the 120 seconds.
    let waited = StartFacts {
        now: NOW + Duration::seconds(120),
        ..waiting
    };
    assert_eq!(start(&waited), D::Cancel);
    // Timeouts don't wait for the DM.
    let timeout = StartFacts {
        dm_waiting: true,
        ..facts(ActionKind::Timeout, ActionState::Pending)
    };
    assert_eq!(start(&timeout), D::Open);

    let voided = StartFacts {
        case_valid: false,
        precheck: Err(Failure::new(ActionFailure::QuackRoleTooLow)),
        ..pending
    };
    assert_eq!(start(&voided), D::Cancel);

    let undo = StartFacts {
        case_valid: false,
        still_applies: Some(still()),
        precheck: Err(Failure::new(ActionFailure::QuackRoleTooLow)),
        ..facts(ActionKind::Unban, ActionState::Pending)
    };
    assert_eq!(start(&undo), D::Skip(still()));
    let undo_runs = StartFacts {
        still_applies: None,
        precheck: Ok(()),
        ..undo
    };
    assert_eq!(start(&undo_runs), D::Open);

    let ended = StartFacts {
        timeout_ends_at: Some(NOW),
        precheck: Err(Failure::new(ActionFailure::QuackRoleTooLow)),
        ..facts(ActionKind::Timeout, ActionState::Pending)
    };
    assert_eq!(
        start(&ended),
        D::Fail(Failure::new(ActionFailure::TimeoutWindowPassed))
    );

    let refused = StartFacts {
        precheck: Err(Failure::missing_permission(DiscordPermission::BanMembers)),
        ..pending
    };
    assert_eq!(
        start(&refused),
        D::Fail(Failure::missing_permission(DiscordPermission::BanMembers))
    );
}

fn target(kind: ActionKind) -> ActionTarget {
    ActionTarget {
        kind,
        case_number: 12,
        timeout_ends_at: matches!(kind, ActionKind::Timeout | ActionKind::RemoveTimeout)
            .then_some(NOW + Duration::hours(1)),
        member_joined_at: Some(NOW - Duration::days(1)),
        case_created_at: NOW,
    }
}

fn member() -> MemberFacts {
    MemberFacts {
        id: MemberId::new(5).unwrap(),
        bot: false,
        role_ids: vec![],
        highest_role_position: 1,
        permissions: Permissions::NONE,
        joined_at: Some(NOW - Duration::days(1)),
        timeout_until: None,
        username: "m".into(),
        global_name: None,
        nick: None,
    }
}

fn ban(reason: &str) -> BanFacts {
    BanFacts {
        reason: Some(reason.into()),
    }
}

fn refused_with(reason: ActionFailure) -> Plan {
    Plan::Finish(AttemptResult::Refused {
        failure: Failure::new(reason),
        discord: None,
    })
}

#[test]
fn timeout_reads_first() {
    let t = target(ActionKind::Timeout);
    let end = t.timeout_ends_at.unwrap();
    let plan = |m: Option<&MemberFacts>| plan_from_member(&t, m, NOW).unwrap();
    assert_eq!(plan(None), refused_with(ActionFailure::MemberNotInServer));
    let admin = MemberFacts {
        permissions: Permission::Administrator.into(),
        ..member()
    };
    assert_eq!(
        plan(Some(&admin)),
        refused_with(ActionFailure::MemberIsAdministrator)
    );
    assert_eq!(plan(Some(&member())), Plan::Call);
    for (current, want) in [
        (end - Duration::seconds(1), Plan::Call),
        (end, Plan::Finish(AttemptResult::NoChange)),
        (
            end + Duration::milliseconds(500),
            Plan::Finish(AttemptResult::NoChange),
        ),
        (
            end + Duration::days(1),
            Plan::Finish(AttemptResult::NoChange),
        ),
    ] {
        let timed_out = MemberFacts {
            timeout_until: Some(current),
            ..member()
        };
        assert_eq!(plan(Some(&timed_out)), want, "{current}");
    }
}

#[test]
fn kick_acts_only_on_the_cases_membership() {
    let t = target(ActionKind::Kick);
    let plan = |m: Option<&MemberFacts>| plan_from_member(&t, m, NOW).unwrap();
    assert_eq!(plan(None), Plan::Finish(AttemptResult::NoChange));
    assert_eq!(plan(Some(&member())), Plan::Call);
    let rejoined = MemberFacts {
        joined_at: Some(NOW + Duration::seconds(5)),
        ..member()
    };
    assert_eq!(plan(Some(&rejoined)), Plan::Finish(AttemptResult::NoChange));
    // An imported case's membership is unknown: compare with created_at.
    let imported = ActionTarget {
        member_joined_at: None,
        ..t
    };
    let before = MemberFacts {
        joined_at: Some(NOW - Duration::seconds(1)),
        ..member()
    };
    assert_eq!(
        plan_from_member(&imported, Some(&before), NOW),
        Ok(Plan::Call)
    );
    assert_eq!(
        plan_from_member(&imported, Some(&rejoined), NOW),
        Ok(Plan::Finish(AttemptResult::NoChange))
    );
}

#[test]
fn remove_timeout_checks_provenance() {
    let t = target(ActionKind::RemoveTimeout);
    let end = t.timeout_ends_at.unwrap();
    let with = |until: Option<OffsetDateTime>| MemberFacts {
        timeout_until: until,
        ..member()
    };
    let plan = |m: Option<&MemberFacts>| plan_from_member(&t, m, NOW).unwrap();
    assert_eq!(plan(None), Plan::Finish(AttemptResult::NoChange));
    assert_eq!(
        plan(Some(&with(None))),
        Plan::Finish(AttemptResult::NoChange)
    );
    assert_eq!(
        plan(Some(&with(Some(NOW - Duration::seconds(1))))),
        Plan::Finish(AttemptResult::NoChange)
    );
    assert_eq!(plan(Some(&with(Some(end)))), Plan::Call);
    assert_eq!(
        plan(Some(&with(Some(end + Duration::milliseconds(300))))),
        Plan::Call
    );
    assert_eq!(
        plan(Some(&with(Some(end + Duration::seconds(1))))),
        Plan::Finish(AttemptResult::ChangedOutsideCase)
    );
}

#[test]
fn ban_and_unban_read_the_ban() {
    let b = target(ActionKind::Ban);
    assert_eq!(plan_from_ban(&b, None), Ok(Plan::Call));
    assert_eq!(
        plan_from_ban(&b, Some(&ban("Quack case #12: Spam"))),
        Ok(Plan::Finish(AttemptResult::Done))
    );
    for other in ["Quack case #11: Spam", "spammer", ""] {
        assert_eq!(
            plan_from_ban(&b, Some(&ban(other))),
            Ok(Plan::Finish(AttemptResult::NoChange)),
            "{other}"
        );
    }
    assert_eq!(
        plan_from_ban(&b, Some(&BanFacts { reason: None })),
        Ok(Plan::Finish(AttemptResult::NoChange))
    );

    let u = target(ActionKind::Unban);
    assert_eq!(
        plan_from_ban(&u, None),
        Ok(Plan::Finish(AttemptResult::NoChange))
    );
    assert_eq!(
        plan_from_ban(&u, Some(&ban("Quack case #7: Spam"))),
        Ok(Plan::CheckMarker(7))
    );
    assert_eq!(
        plan_from_ban(&u, Some(&ban("manual"))),
        Ok(Plan::Finish(AttemptResult::ChangedOutsideCase))
    );
    assert_eq!(plan_after_marker(true), Plan::Call);
    assert_eq!(
        plan_after_marker(false),
        Plan::Finish(AttemptResult::ChangedOutsideCase)
    );

    assert!(plan_from_ban(&target(ActionKind::Kick), None).is_err());
    assert!(plan_from_member(&u, None, NOW).is_err());
}

#[test]
fn classifying_calls_and_reads() {
    use AttemptResult as R;
    let refusal = |status, code| DiscordRefusal { status, code };
    let refused = |status, code| CallOutcome::Refused(refusal(status, code));
    let at = |kind, outcome| after_call(&target(kind), outcome, NOW);
    assert_eq!(at(ActionKind::Ban, CallOutcome::Done), R::Done);
    assert_eq!(
        at(ActionKind::Kick, refused(404, Some(10_007))),
        R::NoChange
    );
    assert_eq!(
        at(ActionKind::Unban, refused(404, Some(10_026))),
        R::NoChange
    );
    assert_eq!(
        at(ActionKind::Timeout, refused(404, Some(10_007))),
        R::Refused {
            failure: Failure::new(ActionFailure::MemberNotInServer),
            discord: Some(refusal(404, Some(10_007)))
        }
    );
    assert_eq!(
        at(ActionKind::Ban, refused(404, Some(10_004))),
        R::Refused {
            failure: Failure::new(ActionFailure::QuackNotInServer),
            discord: Some(refusal(404, Some(10_004)))
        }
    );
    let stale = at(ActionKind::Ban, refused(403, Some(50_013)));
    assert_eq!(
        stale,
        R::Refused {
            failure: Failure::discord_refused(403, Some(50_013)),
            discord: Some(refusal(403, Some(50_013)))
        }
    );
    assert_eq!(stale.discord(), Some(refusal(403, Some(50_013))));
    // A refused timeout after its end is timeout_window_passed.
    let late = after_call(
        &target(ActionKind::Timeout),
        refused(400, Some(50_035)),
        NOW + Duration::hours(1),
    );
    assert!(matches!(
        late,
        R::Refused { failure, .. } if failure.reason == ActionFailure::TimeoutWindowPassed
    ));
    assert_eq!(
        at(ActionKind::Ban, CallOutcome::RetryAfter(Duration::SECOND)),
        R::NotSent {
            retry_after: Some(Duration::SECOND)
        }
    );
    assert_eq!(
        at(ActionKind::Ban, CallOutcome::NotSent),
        R::NotSent { retry_after: None }
    );
    assert_eq!(at(ActionKind::Ban, CallOutcome::Unknown), R::Unknown);

    assert_eq!(
        after_read_failure(CallOutcome::Unknown),
        R::NotSent { retry_after: None }
    );
    assert_eq!(
        after_read_failure(CallOutcome::RetryAfter(Duration::SECOND)),
        R::NotSent {
            retry_after: Some(Duration::SECOND)
        }
    );
    assert!(matches!(
        after_read_failure(refused(404, Some(10_004))),
        R::Refused { failure, .. } if failure.reason == ActionFailure::QuackNotInServer
    ));
    assert!(matches!(
        after_read_failure(refused(403, Some(50_001))),
        R::Refused { failure, .. } if failure.reason == ActionFailure::DiscordRefused
    ));

    for (result, outcome) in [
        (R::Done, AttemptOutcome::Done),
        (R::NoChange, AttemptOutcome::NoChange),
        (R::ChangedOutsideCase, AttemptOutcome::NoChange),
        (R::NotSent { retry_after: None }, AttemptOutcome::NotSent),
        (R::Unknown, AttemptOutcome::Unknown),
        (R::Interrupted, AttemptOutcome::Interrupted),
    ] {
        assert_eq!(result.outcome(), outcome);
    }
}

fn settle_facts(kind: ActionKind, case_valid: bool, attempt: u32) -> SettleFacts<'static> {
    SettleFacts {
        kind,
        case_valid,
        attempt,
        earlier: &[],
        now: NOW,
    }
}

#[test]
fn settle_cases() {
    let s = settle(&settle_facts(ActionKind::Ban, true, 1), AttemptResult::Done).unwrap();
    assert_eq!(
        (s.state, s.outcome, s.no_change, s.failure),
        (ActionState::Done, AttemptOutcome::Done, false, None)
    );
    let s = settle(
        &settle_facts(ActionKind::Kick, true, 1),
        AttemptResult::NoChange,
    )
    .unwrap();
    assert_eq!((s.state, s.no_change), (ActionState::Done, true));

    let s = settle(
        &settle_facts(ActionKind::RemoveTimeout, false, 1),
        AttemptResult::ChangedOutsideCase,
    )
    .unwrap();
    assert_eq!(
        (s.state, s.outcome, s.skip_reason, s.no_change),
        (
            ActionState::Skipped,
            AttemptOutcome::NoChange,
            Some(ActionSkipReason::ChangedOutsideCase),
            false
        )
    );
    assert!(
        settle(
            &settle_facts(ActionKind::Ban, true, 1),
            AttemptResult::ChangedOutsideCase
        )
        .is_err()
    );

    let s = settle(
        &settle_facts(ActionKind::Ban, true, 2),
        AttemptResult::Unknown,
    )
    .unwrap();
    assert_eq!(
        (s.state, s.next_attempt_at),
        (ActionState::Pending, Some(NOW + Duration::seconds(15)))
    );
    let s = settle(
        &settle_facts(ActionKind::Ban, true, 1),
        AttemptResult::NotSent {
            retry_after: Some(Duration::minutes(2)),
        },
    )
    .unwrap();
    assert_eq!(s.next_attempt_at, Some(NOW + Duration::minutes(2)));

    // Exhausted: unconfirmed when any attempt of the round may have applied.
    let only_not_sent = SettleFacts {
        earlier: &[AttemptOutcome::NotSent; 7],
        ..settle_facts(ActionKind::Ban, true, 8)
    };
    let s = settle(&only_not_sent, AttemptResult::NotSent { retry_after: None }).unwrap();
    assert_eq!(
        (s.state, s.failure.map(|f| f.reason), s.next_attempt_at),
        (
            ActionState::Failed,
            Some(ActionFailure::RetriesExhausted),
            None
        )
    );
    let s = settle(&only_not_sent, AttemptResult::Interrupted).unwrap();
    assert_eq!(
        s.failure.map(|f| f.reason),
        Some(ActionFailure::Unconfirmed)
    );
    let earlier_unknown = SettleFacts {
        earlier: &[AttemptOutcome::Unknown],
        ..settle_facts(ActionKind::Ban, true, 8)
    };
    let s = settle(
        &earlier_unknown,
        AttemptResult::NotSent { retry_after: None },
    )
    .unwrap();
    assert_eq!(
        s.failure.map(|f| f.reason),
        Some(ActionFailure::Unconfirmed)
    );

    // Voided original: cancelled unless done or refused.
    let s = settle(
        &settle_facts(ActionKind::Timeout, false, 1),
        AttemptResult::Unknown,
    )
    .unwrap();
    assert_eq!((s.state, s.failure), (ActionState::Cancelled, None));
    let refusal = Failure::discord_refused(403, Some(50_013));
    let s = settle(
        &settle_facts(ActionKind::Timeout, false, 1),
        AttemptResult::Refused {
            failure: refusal,
            discord: None,
        },
    )
    .unwrap();
    assert_eq!((s.state, s.failure), (ActionState::Failed, Some(refusal)));
}

fn view(kind: ActionKind, state: ActionState) -> ActionView {
    ActionView {
        kind,
        state,
        dismissed: false,
        failure: (state == ActionState::Failed).then_some(ActionFailure::DiscordRefused),
        timeout_ends_at: matches!(kind, ActionKind::Timeout | ActionKind::RemoveTimeout)
            .then_some(NOW + Duration::hours(1)),
        has_undo: false,
    }
}

#[test]
fn staff_controls() {
    let c = controls(&view(ActionKind::Ban, ActionState::Failed), true, NOW);
    assert_eq!((c.retry, c.dismiss), (Ok(()), Ok(())));
    assert_eq!(c.undo, Err(DomainError::ActionNotUndoable));
    assert!(c.can_retry() && c.can_dismiss() && !c.can_undo());

    let dismissed = ActionView {
        dismissed: true,
        ..view(ActionKind::Ban, ActionState::Failed)
    };
    let c = controls(&dismissed, true, NOW);
    assert_eq!(c.retry, Err(DomainError::ActionNotRetryable));
    assert_eq!(c.dismiss, Err(DomainError::ActionNotDismissable));

    for state in [
        ActionState::Pending,
        ActionState::Running,
        ActionState::Cancelled,
    ] {
        let c = controls(&view(ActionKind::Ban, state), true, NOW);
        assert_eq!(c.retry, Err(DomainError::ActionNotRetryable));
        assert_eq!(c.dismiss, Err(DomainError::ActionNotDismissable));
        assert_eq!(c.undo, Err(DomainError::ActionNotUndoable));
    }

    assert_eq!(
        controls(&view(ActionKind::Ban, ActionState::Failed), false, NOW).retry,
        Err(DomainError::CaseVoided)
    );
    // Undos can always be retried: their case is voided by definition.
    assert_eq!(
        controls(&view(ActionKind::Unban, ActionState::Failed), false, NOW).retry,
        Ok(())
    );
    let window = ActionView {
        failure: Some(ActionFailure::TimeoutWindowPassed),
        ..view(ActionKind::Timeout, ActionState::Failed)
    };
    assert_eq!(
        controls(&window, true, NOW).retry,
        Err(DomainError::TimeoutWindowPassed)
    );
    let ended = view(ActionKind::Timeout, ActionState::Failed);
    assert_eq!(
        controls(&ended, true, NOW + Duration::hours(1)).retry,
        Err(DomainError::TimeoutWindowPassed)
    );
    assert_eq!(controls(&ended, true, NOW).retry, Ok(()));

    assert_eq!(
        controls(&view(ActionKind::Ban, ActionState::Done), true, NOW).undo,
        Ok(ActionKind::Unban)
    );
    assert_eq!(
        controls(&view(ActionKind::Timeout, ActionState::Done), true, NOW).undo,
        Ok(ActionKind::RemoveTimeout)
    );
    assert_eq!(
        controls(
            &view(ActionKind::Timeout, ActionState::Done),
            true,
            NOW + Duration::hours(2)
        )
        .undo,
        Err(DomainError::ActionNotUndoable)
    );
    let undone = ActionView {
        has_undo: true,
        ..view(ActionKind::Ban, ActionState::Done)
    };
    assert_eq!(
        controls(&undone, true, NOW).undo,
        Err(DomainError::ActionNotUndoable)
    );
    for kind in [
        ActionKind::Kick,
        ActionKind::Unban,
        ActionKind::RemoveTimeout,
    ] {
        assert_eq!(
            controls(&view(kind, ActionState::Done), true, NOW).undo,
            Err(DomainError::ActionNotUndoable)
        );
    }
}

fn void_facts(
    kind: ActionKind,
    state: ActionState,
    attempts: &'static [Option<AttemptOutcome>],
) -> VoidFacts<'static> {
    VoidFacts {
        kind,
        state,
        has_undo: false,
        attempts,
        timeout_ends_at: (kind == ActionKind::Timeout).then_some(NOW + Duration::hours(1)),
        now: NOW,
    }
}

#[test]
fn void_effects_follow_the_table() {
    use ActionKind::{Ban, Kick, RemoveTimeout, Timeout, Unban};
    use ActionState::{Cancelled, Done, Failed, Pending, Running, Skipped};
    use AttemptOutcome as O;
    let effect = |cancel, undo| VoidEffect {
        cancel,
        queue_undo: undo,
    };
    let cases: &[(
        ActionKind,
        ActionState,
        &'static [Option<AttemptOutcome>],
        VoidEffect,
    )] = &[
        (Ban, Pending, &[], effect(true, None)),
        (Ban, Pending, &[Some(O::NotSent)], effect(true, None)),
        (Ban, Pending, &[Some(O::Unknown)], effect(true, Some(Unban))),
        (
            Timeout,
            Pending,
            &[Some(O::Interrupted)],
            effect(true, Some(RemoveTimeout)),
        ),
        (Kick, Pending, &[Some(O::Unknown)], effect(true, None)),
        (Ban, Running, &[None], effect(false, Some(Unban))),
        (
            Timeout,
            Running,
            &[Some(O::NotSent), None],
            effect(false, Some(RemoveTimeout)),
        ),
        (Kick, Running, &[None], effect(false, None)),
        (Ban, Done, &[Some(O::Done)], effect(false, Some(Unban))),
        (Ban, Done, &[Some(O::NoChange)], effect(false, None)),
        (
            Timeout,
            Done,
            &[Some(O::Done)],
            effect(false, Some(RemoveTimeout)),
        ),
        (Kick, Done, &[Some(O::Done)], effect(false, None)),
        (Ban, Failed, &[Some(O::Refused)], effect(false, None)),
        (
            Ban,
            Failed,
            &[Some(O::Unknown), Some(O::Refused)],
            effect(false, Some(Unban)),
        ),
        (Ban, Cancelled, &[Some(O::Unknown)], effect(false, None)),
        (Unban, Pending, &[], effect(false, None)),
        (Unban, Failed, &[Some(O::Unknown)], effect(false, None)),
        (RemoveTimeout, Skipped, &[], effect(false, None)),
    ];
    for &(kind, state, attempts, want) in cases {
        assert_eq!(
            on_void(&void_facts(kind, state, attempts)),
            want,
            "{kind:?} {state:?} {attempts:?}"
        );
    }
    // A timeout that already ended needs no undo.
    let ended = VoidFacts {
        now: NOW + Duration::hours(1),
        ..void_facts(Timeout, Done, &[Some(O::Done)])
    };
    assert_eq!(on_void(&ended), effect(false, None));
    // Never a second undo.
    let has_undo = VoidFacts {
        has_undo: true,
        ..void_facts(Ban, Done, &[Some(O::Done)])
    };
    assert_eq!(on_void(&has_undo), effect(false, None));
}

fn candidate(n: u64, kind: ActionKind, state: ActionState) -> GuardCandidate {
    GuardCandidate {
        case_id: CaseId::from_uuid(Uuid::from_u128(u128::from(n))),
        case_number: n,
        case_valid: true,
        kind,
        state,
        timeout_ends_at: (kind == ActionKind::Timeout).then_some(NOW + Duration::hours(1)),
    }
}

#[test]
fn undo_guard_matches_the_query() {
    let own = CaseId::from_uuid(Uuid::from_u128(1));
    let guard = |undo, c: &[GuardCandidate]| undo_guard(undo, own, c, NOW).map(|s| s.case_number);
    let done_ban = candidate(5, ActionKind::Ban, ActionState::Done);
    assert_eq!(guard(ActionKind::Unban, &[done_ban]), Some(5));
    // Only done actions count.
    for state in [
        ActionState::Pending,
        ActionState::Running,
        ActionState::Failed,
        ActionState::Cancelled,
    ] {
        assert_eq!(
            guard(ActionKind::Unban, &[candidate(5, ActionKind::Ban, state)]),
            None,
            "{state:?}"
        );
    }
    let voided = GuardCandidate {
        case_valid: false,
        ..done_ban
    };
    assert_eq!(guard(ActionKind::Unban, &[voided]), None);
    let same_case = GuardCandidate {
        case_id: own,
        ..done_ban
    };
    assert_eq!(guard(ActionKind::Unban, &[same_case]), None);
    assert_eq!(guard(ActionKind::RemoveTimeout, &[done_ban]), None);
    let ended = GuardCandidate {
        timeout_ends_at: Some(NOW),
        ..candidate(6, ActionKind::Timeout, ActionState::Done)
    };
    assert_eq!(guard(ActionKind::RemoveTimeout, &[ended]), None);
    let live = candidate(7, ActionKind::Timeout, ActionState::Done);
    assert_eq!(guard(ActionKind::RemoveTimeout, &[ended, live]), Some(7));
    // The newest case wins.
    let older = candidate(3, ActionKind::Ban, ActionState::Done);
    assert_eq!(guard(ActionKind::Unban, &[older, done_ban]), Some(5));
    // Originals have no guard.
    assert_eq!(guard(ActionKind::Ban, &[done_ban]), None);
}
