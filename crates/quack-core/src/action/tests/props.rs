//! Property tests for the action state machine and the attempt flow.

use proptest::prelude::*;
use time::Duration;

use super::*;

fn kind_strategy() -> impl Strategy<Value = ActionKind> {
    prop::sample::select(ActionKind::ALL)
}

fn state_strategy() -> impl Strategy<Value = ActionState> {
    prop::sample::select(ActionState::ALL)
}

fn event_strategy() -> impl Strategy<Value = ActionEvent> {
    prop::sample::select(all_events())
}

fn call_strategy() -> impl Strategy<Value = CallOutcome> {
    prop_oneof![
        Just(CallOutcome::Done),
        Just(CallOutcome::Unknown),
        Just(CallOutcome::NotSent),
        (0_i64..100_000).prop_map(|s| CallOutcome::RetryAfter(Duration::seconds(s))),
        (
            400_u16..500,
            prop::option::of(prop::sample::select(vec![
                10_004_u32, 10_007, 10_026, 50_013, 50_001, 50_035
            ]))
        )
            .prop_map(|(status, code)| CallOutcome::Refused(DiscordRefusal { status, code })),
    ]
}

fn result_strategy() -> impl Strategy<Value = AttemptResult> {
    prop_oneof![
        (kind_strategy(), call_strategy(), -7_200_i64..7_200).prop_map(|(kind, call, offset)| {
            after_call(&target(kind), call, NOW + Duration::seconds(offset))
        }),
        call_strategy().prop_map(after_read_failure),
        Just(AttemptResult::Interrupted),
        Just(AttemptResult::NoChange),
    ]
}

proptest! {
    #[test]
    fn random_events_never_leave_the_table(
        kind in kind_strategy(),
        events in prop::collection::vec(event_strategy(), 0..60),
    ) {
        let mut state = ActionState::Pending;
        for event in events {
            let before = state;
            if let Ok(next) = transition(kind, state, event) {
                prop_assert!(!before.is_terminal(), "left terminal {before:?} via {event:?}");
                prop_assert_eq!(Some(next), expected(kind, before, event));
                if before == ActionState::Failed && next != ActionState::Failed {
                    prop_assert_eq!(event, ActionEvent::StaffRetry, "left failed without a staff retry");
                }
                if next == ActionState::Skipped {
                    prop_assert!(!kind.is_original());
                }
                if next == ActionState::Cancelled {
                    prop_assert!(kind.is_original());
                }
                state = next;
            }
        }
    }

    #[test]
    fn never_starts_an_original_on_a_voided_case(
        kind in kind_strategy(),
        state in state_strategy(),
        guard in any::<bool>(),
        precheck_ok in any::<bool>(),
        busy in any::<bool>(),
        dm_waiting in any::<bool>(),
        offset in -100_000_i64..100_000,
    ) {
        let facts = StartFacts {
            kind,
            state,
            next_attempt_at: None,
            member_busy: busy,
            case_valid: false,
            case_created_at: NOW + Duration::seconds(offset),
            dm_waiting,
            still_applies: guard.then_some(still()),
            precheck: if precheck_ok { Ok(()) } else { Err(Failure::new(ActionFailure::QuackRoleTooLow)) },
            timeout_ends_at: Some(NOW + Duration::seconds(offset)),
            now: NOW,
        };
        if kind.is_original() {
            prop_assert_ne!(start(&facts), StartDecision::Open, "started {:?} on a voided case", kind);
        }
    }

    #[test]
    fn timeout_end_never_moves_and_is_whole_seconds(
        created_ms in -1_000_000_000_i64..1_000_000_000,
        duration in 1_u32..=2_419_200,
        now_offset in -3_000_000_i64..3_000_000,
    ) {
        let created = NOW + Duration::milliseconds(created_ms);
        let end = timeout_ends_at(created, duration).unwrap();
        prop_assert_eq!(end.nanosecond(), 0);
        prop_assert!(end <= created + Duration::seconds(i64::from(duration)));
        prop_assert!(end > created + Duration::seconds(i64::from(duration) - 1));
        let now = NOW + Duration::seconds(now_offset);
        let facts = StartFacts { timeout_ends_at: Some(end), now, ..super::facts(ActionKind::Timeout, ActionState::Pending) };
        match start(&facts) {
            StartDecision::Open => prop_assert!(now < end),
            StartDecision::Fail(f) => {
                prop_assert_eq!(f.reason, ActionFailure::TimeoutWindowPassed);
                prop_assert!(now >= end);
            }
            other => prop_assert!(false, "unexpected {other:?}"),
        }
    }

    #[test]
    fn settle_respects_the_round_budget(
        kind in kind_strategy(),
        valid in any::<bool>(),
        attempt in 1_u32..20,
        result in result_strategy(),
    ) {
        let facts = settle_facts(kind, valid, attempt);
        let Ok(s) = settle(&facts, result) else {
            // Only an original can't take an undo's provenance answer.
            prop_assert!(result == AttemptResult::ChangedOutsideCase && kind.is_original());
            return Ok(());
        };
        if attempt >= MAX_ATTEMPTS {
            prop_assert_ne!(s.state, ActionState::Pending);
        }
        prop_assert_eq!(s.state == ActionState::Pending, s.next_attempt_at.is_some());
        prop_assert_eq!(s.state == ActionState::Failed, s.failure.is_some());
        prop_assert_eq!(s.state == ActionState::Skipped, s.skip_reason.is_some());
        if s.state == ActionState::Cancelled {
            prop_assert!(kind.is_original() && !valid);
        }
        if let (AttemptResult::NotSent { retry_after: Some(d) }, Some(at)) = (result, s.next_attempt_at) {
            prop_assert!(at - NOW >= d);
        }
        if let Some(f) = s.failure
            && f.reason == ActionFailure::RetriesExhausted
        {
            prop_assert!(!s.outcome.may_have_applied());
        }
    }

    #[test]
    fn a_round_never_exceeds_its_budget(results in prop::collection::vec(result_strategy(), 1..30)) {
        // Simulate one round: attempts continue only while the action is pending.
        let mut state = ActionState::Pending;
        let mut outcomes = Vec::new();
        for result in results {
            if state != ActionState::Pending {
                break;
            }
            let attempt = u32::try_from(outcomes.len()).unwrap() + 1;
            let facts = SettleFacts { earlier: &outcomes, ..settle_facts(ActionKind::Ban, true, attempt) };
            let s = settle(&facts, result).unwrap();
            if s.state == ActionState::Failed && s.failure.is_some_and(|f| f.reason == ActionFailure::Unconfirmed) {
                let round: Vec<_> = outcomes.iter().copied().chain([s.outcome]).map(Some).collect();
                prop_assert!(may_have_applied(&round));
            }
            outcomes.push(s.outcome);
            state = s.state;
        }
        prop_assert!(outcomes.len() <= MAX_ATTEMPTS as usize);
    }
}
