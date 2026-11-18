//! Property tests for the action state machine and job flow.

use proptest::prelude::*;
use time::Duration;
use uuid::Uuid;

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

fn result_strategy() -> impl Strategy<Value = AttemptResult> {
    prop_oneof![
        Just(AttemptResult::Done),
        Just(AttemptResult::Unknown),
        Just(AttemptResult::Interrupted),
        (0_i64..100_000).prop_map(|s| AttemptResult::RateLimited(Duration::seconds(s))),
        (
            400_u16..500,
            prop::option::of(prop::sample::select(vec![
                10_004_u32, 10_007, 10_026, 50_013, 50_001
            ]))
        )
            .prop_map(|(status, code)| AttemptResult::Refused(DiscordRefusal { status, code })),
    ]
}

proptest! {
    #[test]
    fn never_leaves_a_terminal_state(kind in kind_strategy(), events in prop::collection::vec(event_strategy(), 0..40)) {
        let mut state = ActionState::Pending;
        for event in events {
            let before = state;
            if let Ok(next) = transition(kind, state, event) {
                prop_assert!(!before.is_terminal(), "left terminal {before:?} via {event:?}");
                state = next;
            }
        }
    }

    #[test]
    fn never_starts_on_a_voided_case(
        kind in kind_strategy(),
        state in state_strategy(),
        guard in any::<bool>(),
        precheck_ok in any::<bool>(),
        offset in -100_000_i64..100_000,
    ) {
        let facts = ClaimFacts {
            kind,
            state,
            case_valid: false,
            still_applies: guard.then_some(StillApplies { case_id: CaseId::from_uuid(Uuid::nil()), case_number: 1 }),
            precheck: if precheck_ok { Ok(()) } else { Err(Failure::new(ActionFailure::QuackRoleTooLow)) },
            timeout_ends_at: Some(NOW + Duration::seconds(offset)),
            now: NOW,
        };
        let decision = before_attempt(&facts);
        if kind.purpose() == ActionPurpose::Original {
            let started = matches!(decision, ClaimDecision::Attempt { .. });
            prop_assert!(!started, "started {:?} on a voided case", kind);
        }
    }

    #[test]
    fn timeout_end_never_moves(
        created_offset in -1_000_000_i64..1_000_000,
        duration in 1_u32..=2_419_200,
        now_offset in -3_000_000_i64..3_000_000,
    ) {
        let created = NOW + Duration::seconds(created_offset);
        let end = timeout_ends_at(created, duration);
        let facts = ClaimFacts { timeout_ends_at: Some(end), now: NOW + Duration::seconds(now_offset), ..claim(ActionKind::Timeout, ActionState::Pending) };
        match before_attempt(&facts) {
            ClaimDecision::Attempt { timeout_until } => prop_assert_eq!(timeout_until, Some(end)),
            ClaimDecision::Fail(f) => prop_assert_eq!(f.reason, ActionFailure::TimeoutWindowPassed),
            other => prop_assert!(false, "unexpected {other:?}"),
        }
        prop_assert_eq!(end - created, Duration::seconds(i64::from(duration)));
    }

    #[test]
    fn retry_budget_is_respected(kind in kind_strategy(), valid in any::<bool>(), attempt in 1_u32..20, result in result_strategy()) {
        let s = settle(&ctx(kind, valid, attempt), result).unwrap();
        if attempt >= MAX_ATTEMPTS {
            prop_assert_ne!(s.state, ActionState::Pending);
        }
        prop_assert_eq!(s.state == ActionState::Pending, s.retry_in.is_some());
        prop_assert_eq!(s.state == ActionState::Failed, s.failure.is_some());
        if let (AttemptResult::RateLimited(d), Some(wait)) = (result, s.retry_in) {
            prop_assert!(wait >= d);
        }
    }

    #[test]
    fn job_run_never_exceeds_the_budget(results in prop::collection::vec(result_strategy(), 1..30)) {
        // Simulate one job: attempts continue only while the action is pending.
        let mut state = ActionState::Pending;
        let mut attempts = 0;
        for result in results {
            if state != ActionState::Pending {
                break;
            }
            attempts += 1;
            state = settle(&ctx(ActionKind::Ban, true, attempts), result).unwrap().state;
        }
        prop_assert!(attempts <= MAX_ATTEMPTS);
    }
}
