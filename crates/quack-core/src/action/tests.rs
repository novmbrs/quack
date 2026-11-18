use time::macros::datetime;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use super::*;
use crate::DomainError;
use crate::ids::CaseId;

const NOW: OffsetDateTime = datetime!(2026-11-17 12:00 UTC);

fn finished(outcome: AttemptOutcome, case_valid: bool, attempts_left: bool) -> ActionEvent {
    ActionEvent::AttemptFinished {
        outcome,
        case_valid,
        attempts_left,
    }
}

fn all_events() -> Vec<ActionEvent> {
    let mut events = vec![
        ActionEvent::AttemptStarted,
        ActionEvent::CaseVoided,
        ActionEvent::UndoSkipped,
        ActionEvent::PrecheckFailed,
        ActionEvent::StaffRetry,
        ActionEvent::StaffDismiss,
    ];
    for &outcome in AttemptOutcome::ALL {
        for valid in [true, false] {
            for left in [true, false] {
                events.push(finished(outcome, valid, left));
            }
        }
    }
    events
}

/// The design's table (§5.3), written out independently of `transition`.
#[allow(clippy::match_same_arms, reason = "one arm per table row")]
fn expected(kind: ActionKind, state: ActionState, event: ActionEvent) -> Option<ActionState> {
    use ActionEvent as E;
    use ActionState as S;
    let original = kind.purpose() == ActionPurpose::Original;
    match (state, event) {
        (S::Pending, E::AttemptStarted) => Some(S::Running),
        (S::Pending, E::CaseVoided) => original.then_some(S::Cancelled),
        (S::Pending, E::UndoSkipped) => (!original).then_some(S::Skipped),
        (S::Pending, E::PrecheckFailed) => Some(S::Failed),
        (
            S::Running,
            E::AttemptFinished {
                outcome: AttemptOutcome::Done | AttemptOutcome::NoChange,
                ..
            },
        ) => Some(S::Done),
        (
            S::Running,
            E::AttemptFinished {
                case_valid: false, ..
            },
        ) if original => Some(S::Cancelled),
        (
            S::Running,
            E::AttemptFinished {
                outcome: AttemptOutcome::Refused,
                ..
            },
        ) => Some(S::Failed),
        (
            S::Running,
            E::AttemptFinished {
                attempts_left: true,
                ..
            },
        ) => Some(S::Pending),
        (
            S::Running,
            E::AttemptFinished {
                attempts_left: false,
                ..
            },
        ) => Some(S::Failed),
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
fn terminal_states_accept_nothing() {
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
    }
    assert!(!ActionState::Failed.is_terminal());
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
        crate::permissions::Permission::BanMembers
    );
}

#[test]
fn refusal_classification() {
    use AttemptOutcome as O;
    let refused = |status, code| AttemptResult::Refused(DiscordRefusal { status, code });
    let reason = |kind, r: AttemptResult| r.outcome(kind);
    assert_eq!(
        reason(ActionKind::Kick, refused(404, Some(10_007))),
        (O::NoChange, None)
    );
    assert_eq!(
        reason(ActionKind::RemoveTimeout, refused(404, Some(10_007))),
        (O::NoChange, None)
    );
    assert_eq!(
        reason(ActionKind::Unban, refused(404, Some(10_026))),
        (O::NoChange, None)
    );
    assert_eq!(
        reason(ActionKind::Timeout, refused(404, Some(10_007))),
        (
            O::Refused,
            Some(Failure::new(ActionFailure::MemberNotInServer))
        )
    );
    assert_eq!(
        reason(ActionKind::Ban, refused(404, Some(10_004))),
        (
            O::Refused,
            Some(Failure::new(ActionFailure::QuackNotInServer))
        )
    );
    let (outcome, failure) = reason(ActionKind::Ban, refused(403, Some(50_013)));
    assert_eq!(outcome, O::Refused);
    let failure = failure.unwrap();
    assert_eq!(
        (failure.reason, failure.http_status, failure.discord_code),
        (ActionFailure::DiscordRefused, Some(403), Some(50_013))
    );
    assert_eq!(
        reason(ActionKind::Ban, AttemptResult::Unknown).0,
        O::Unknown
    );
    assert_eq!(
        reason(ActionKind::Ban, AttemptResult::Interrupted).0,
        O::Interrupted
    );
    assert_eq!(
        reason(
            ActionKind::Ban,
            AttemptResult::RateLimited(Duration::SECOND)
        )
        .0,
        O::RateLimited
    );
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
fn timeout_end_and_window() {
    let end = timeout_ends_at(NOW, 3600);
    assert_eq!(end, NOW + Duration::hours(1));
    assert!(!timeout_window_passed(end - Duration::seconds(6), end));
    assert!(timeout_window_passed(end - Duration::seconds(5), end));
    assert!(timeout_window_passed(end, end));
}

fn ctx(kind: ActionKind, case_valid: bool, attempt: u32) -> AttemptContext {
    AttemptContext {
        kind,
        case_valid,
        attempt,
        timeout_ends_at: Some(NOW + Duration::hours(1)),
        now: NOW,
    }
}

#[test]
fn settle_cases() {
    let s = settle(&ctx(ActionKind::Ban, true, 1), AttemptResult::Done).unwrap();
    assert_eq!(
        (s.state, s.outcome, s.no_change, s.queue_undo),
        (ActionState::Done, AttemptOutcome::Done, false, None)
    );

    let s = settle(&ctx(ActionKind::Ban, false, 1), AttemptResult::Done).unwrap();
    assert_eq!(
        (s.state, s.queue_undo),
        (ActionState::Done, Some(ActionKind::Unban))
    );

    let mut ended = ctx(ActionKind::Timeout, false, 1);
    ended.timeout_ends_at = Some(NOW);
    assert_eq!(
        settle(&ended, AttemptResult::Done).unwrap().queue_undo,
        None
    );
    assert_eq!(
        settle(&ctx(ActionKind::Kick, false, 1), AttemptResult::Done)
            .unwrap()
            .queue_undo,
        None
    );

    let s = settle(
        &ctx(ActionKind::Unban, true, 1),
        AttemptResult::Refused(DiscordRefusal {
            status: 404,
            code: Some(10_026),
        }),
    )
    .unwrap();
    assert_eq!(
        (s.state, s.outcome, s.no_change),
        (ActionState::Done, AttemptOutcome::NoChange, true)
    );

    let s = settle(&ctx(ActionKind::Kick, true, 2), AttemptResult::Unknown).unwrap();
    assert_eq!(
        (s.state, s.retry_in, s.failure),
        (ActionState::Pending, Some(Duration::seconds(15)), None)
    );

    let s = settle(&ctx(ActionKind::Kick, true, 8), AttemptResult::Unknown).unwrap();
    assert_eq!((s.state, s.retry_in), (ActionState::Failed, None));
    assert_eq!(
        s.failure.map(|f| f.reason),
        Some(ActionFailure::RetriesExhausted)
    );

    let s = settle(
        &ctx(ActionKind::Kick, false, 1),
        AttemptResult::Refused(DiscordRefusal {
            status: 403,
            code: None,
        }),
    )
    .unwrap();
    assert_eq!((s.state, s.failure), (ActionState::Cancelled, None));

    // Undo actions are never cancelled by a void.
    let s = settle(&ctx(ActionKind::Unban, false, 1), AttemptResult::Unknown).unwrap();
    assert_eq!(s.state, ActionState::Pending);
}

fn claim(kind: ActionKind, state: ActionState) -> ClaimFacts {
    ClaimFacts {
        kind,
        state,
        case_valid: true,
        still_applies: None,
        precheck: Ok(()),
        timeout_ends_at: (kind == ActionKind::Timeout).then_some(NOW + Duration::hours(1)),
        now: NOW,
    }
}

#[test]
fn before_attempt_order() {
    let case = CaseId::from_uuid(Uuid::from_u128(9));
    let still = StillApplies {
        case_id: case,
        case_number: 4,
    };
    let refused = Failure::new(ActionFailure::QuackRoleTooLow);

    assert_eq!(
        before_attempt(&claim(ActionKind::Ban, ActionState::Done)),
        ClaimDecision::AlreadySettled
    );
    assert_eq!(
        before_attempt(&claim(ActionKind::Ban, ActionState::Failed)),
        ClaimDecision::AlreadySettled
    );
    assert_eq!(
        before_attempt(&claim(ActionKind::Ban, ActionState::Running)),
        ClaimDecision::Interrupted
    );
    let voided = ClaimFacts {
        case_valid: false,
        precheck: Err(refused),
        ..claim(ActionKind::Ban, ActionState::Pending)
    };
    assert_eq!(before_attempt(&voided), ClaimDecision::Cancel);
    let undo = ClaimFacts {
        case_valid: false,
        still_applies: Some(still),
        precheck: Err(refused),
        ..claim(ActionKind::Unban, ActionState::Pending)
    };
    assert_eq!(before_attempt(&undo), ClaimDecision::Skip(still));
    let undo = ClaimFacts {
        case_valid: false,
        precheck: Err(refused),
        ..claim(ActionKind::Unban, ActionState::Pending)
    };
    assert_eq!(before_attempt(&undo), ClaimDecision::Fail(refused));
    let late = ClaimFacts {
        timeout_ends_at: Some(NOW + Duration::seconds(5)),
        ..claim(ActionKind::Timeout, ActionState::Pending)
    };
    assert_eq!(
        before_attempt(&late),
        ClaimDecision::Fail(Failure::new(ActionFailure::TimeoutWindowPassed))
    );
    assert_eq!(
        before_attempt(&claim(ActionKind::Timeout, ActionState::Pending)),
        ClaimDecision::Attempt {
            timeout_until: Some(NOW + Duration::hours(1))
        }
    );
    assert_eq!(
        before_attempt(&claim(ActionKind::Kick, ActionState::Pending)),
        ClaimDecision::Attempt {
            timeout_until: None
        }
    );
}

#[test]
fn staff_controls() {
    use ActionKind as K;
    use ActionState as S;
    assert_eq!(
        can_retry(
            K::Ban,
            S::Failed,
            false,
            true,
            Some(ActionFailure::DiscordRefused)
        ),
        Ok(())
    );
    assert_eq!(
        can_retry(K::Ban, S::Done, false, true, None),
        Err(DomainError::ActionNotRetryable)
    );
    assert_eq!(
        can_retry(K::Ban, S::Failed, true, true, None),
        Err(DomainError::ActionNotRetryable)
    );
    assert_eq!(
        can_retry(K::Ban, S::Failed, false, false, None),
        Err(DomainError::CaseVoided)
    );
    assert_eq!(
        can_retry(
            K::Timeout,
            S::Failed,
            false,
            true,
            Some(ActionFailure::TimeoutWindowPassed)
        ),
        Err(DomainError::TimeoutWindowPassed)
    );
    assert_eq!(can_retry(K::Unban, S::Failed, false, false, None), Ok(()));

    assert_eq!(can_dismiss(S::Failed, false), Ok(()));
    assert_eq!(
        can_dismiss(S::Failed, true),
        Err(DomainError::ActionNotDismissable)
    );
    assert_eq!(
        can_dismiss(S::Pending, false),
        Err(DomainError::ActionNotDismissable)
    );

    let future = Some(NOW + Duration::minutes(1));
    assert_eq!(
        can_undo(K::Timeout, S::Done, false, future, NOW),
        Ok(K::RemoveTimeout)
    );
    assert_eq!(
        can_undo(K::Timeout, S::Done, false, Some(NOW), NOW),
        Err(DomainError::ActionNotUndoable)
    );
    assert_eq!(can_undo(K::Ban, S::Done, false, None, NOW), Ok(K::Unban));
    assert_eq!(
        can_undo(K::Ban, S::Done, true, None, NOW),
        Err(DomainError::ActionNotUndoable)
    );
    assert_eq!(
        can_undo(K::Ban, S::Running, false, None, NOW),
        Err(DomainError::ActionNotUndoable)
    );
    assert_eq!(
        can_undo(K::Kick, S::Done, false, None, NOW),
        Err(DomainError::ActionNotUndoable)
    );
}

#[test]
fn void_effects() {
    use ActionKind as K;
    use ActionState as S;
    let future = Some(NOW + Duration::minutes(1));
    let cases = [
        (K::Ban, S::Pending, false, None, VoidEffect::Cancel),
        (K::Ban, S::Running, false, None, VoidEffect::LeaveRunning),
        (
            K::Ban,
            S::Done,
            false,
            None,
            VoidEffect::QueueUndo(K::Unban),
        ),
        (K::Ban, S::Done, true, None, VoidEffect::Nothing),
        (
            K::Timeout,
            S::Done,
            false,
            future,
            VoidEffect::QueueUndo(K::RemoveTimeout),
        ),
        (K::Timeout, S::Done, false, Some(NOW), VoidEffect::Nothing),
        (K::Kick, S::Done, false, None, VoidEffect::Nothing),
        (K::Ban, S::Failed, false, None, VoidEffect::Nothing),
        (K::Ban, S::Cancelled, false, None, VoidEffect::Nothing),
        (K::Unban, S::Pending, false, None, VoidEffect::Nothing),
        (
            K::RemoveTimeout,
            S::Running,
            false,
            None,
            VoidEffect::Nothing,
        ),
    ];
    for (kind, state, has_undo, end, effect) in cases {
        assert_eq!(
            on_void(kind, state, has_undo, end, NOW),
            effect,
            "{kind:?} {state:?}"
        );
    }
}

fn candidate(n: u64, kind: ActionKind, state: ActionState) -> GuardCandidate {
    GuardCandidate {
        case_id: CaseId::from_uuid(Uuid::from_u128(u128::from(n))),
        case_number: n,
        case_valid: true,
        kind,
        state,
        timeout_ends_at: (kind == ActionKind::Timeout).then_some(NOW + Duration::hours(1)),
        undone: false,
    }
}

#[test]
fn undo_guard_matches_the_query() {
    let own = CaseId::from_uuid(Uuid::from_u128(1));
    let ban = |n, s| candidate(n, ActionKind::Ban, s);
    assert_eq!(undo_guard(ActionKind::Unban, own, &[], NOW), None);
    let found = undo_guard(
        ActionKind::Unban,
        own,
        &[
            ban(2, ActionState::Done),
            ban(5, ActionState::Pending),
            ban(3, ActionState::Running),
        ],
        NOW,
    );
    assert_eq!(found.map(|s| s.case_number), Some(5));
    let ignored = [
        ban(1, ActionState::Done),   // own case
        ban(6, ActionState::Failed), // not in force
        ban(7, ActionState::Cancelled),
        GuardCandidate {
            case_valid: false,
            ..ban(8, ActionState::Done)
        },
        GuardCandidate {
            undone: true,
            ..ban(9, ActionState::Done)
        },
        candidate(10, ActionKind::Timeout, ActionState::Done), // other kind
    ];
    assert_eq!(undo_guard(ActionKind::Unban, own, &ignored, NOW), None);
    let ended = GuardCandidate {
        timeout_ends_at: Some(NOW),
        ..candidate(11, ActionKind::Timeout, ActionState::Done)
    };
    assert_eq!(
        undo_guard(ActionKind::RemoveTimeout, own, &[ended], NOW),
        None
    );
    let live = candidate(12, ActionKind::Timeout, ActionState::Done);
    assert_eq!(
        undo_guard(ActionKind::RemoveTimeout, own, &[live], NOW).map(|s| s.case_number),
        Some(12)
    );
    assert_eq!(
        undo_guard(ActionKind::Ban, own, &[ban(2, ActionState::Done)], NOW),
        None
    );
}

mod props;
