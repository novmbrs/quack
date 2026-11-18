//! Go execution and attempt states → action states (§19.7) and the
//! in-flight policy (§19.9): nothing whose outcome is uncertain is ever left
//! for a worker to replay.

use quack_core::action::{ActionFailure, ActionKind, AttemptOutcome, may_have_applied};
use serde_json::Value;

use crate::convert::has_class;

/// Go error classes that show Discord (or Quack, before calling it)
/// definitely refused an attempt. Anything else that failed is `unknown`.
const REFUSED: [&str; 11] = [
    "permission_or_hierarchy_denied",
    "permission_denied",
    "unknown_member_or_resource",
    "validation_failed",
    "discord_failure",
    "invalid_action_config",
    "unsupported_action",
    "guild_not_found",
    "reversal_authorization_unavailable",
    "reversal_permission_denied",
    "reversal_ownership_conflict",
];

/// Go error classes of an attempt that never reached Discord.
const NOT_SENT: [&str; 5] = [
    "rate_limited",
    "discord_unavailable",
    "guild_lookup_failed",
    "reversal_provenance_unavailable",
    "case_voided",
];

/// An attempt's outcome from its Go status, error code, and response. An
/// attempt that never finished (`open`) may have applied unless it
/// succeeded; a failure counts as unsent or refused only for a known code
/// that shows it (an allowlist), so an unrecognized one is `unknown`.
pub(crate) fn attempt_outcome(
    status: &str,
    code: &str,
    response: &Value,
    open: bool,
) -> AttemptOutcome {
    let any = |classes: &[&str]| classes.iter().any(|c| has_class(code, c));
    match status {
        "succeeded" if response.get("reversal_noop").and_then(Value::as_bool) == Some(true) => {
            AttemptOutcome::NoChange
        }
        "succeeded" => AttemptOutcome::Done,
        _ if open => AttemptOutcome::Interrupted,
        "failed" if any(&NOT_SENT) => AttemptOutcome::NotSent,
        "failed" if any(&REFUSED) => AttemptOutcome::Refused,
        "failed" if code == "lease_expired" => AttemptOutcome::Interrupted,
        // `running` never finished: the process died mid-attempt.
        "running" => AttemptOutcome::Interrupted,
        _ => AttemptOutcome::Unknown,
    }
}

/// What the importer decides for one execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum State {
    /// Runs after cutover (a `run_action` job is inserted).
    Pending,
    /// Done; `no_change` when Discord already had the desired state.
    Done { no_change: bool },
    /// An original that will not run.
    Cancelled,
    /// Failed, with the reason and (for Discord refusals) the HTTP status.
    Failed {
        reason: ActionFailure,
        http: Option<i16>,
    },
    /// An undo the Go system could not show was its own effect to undo.
    Skipped,
}

/// The facts the state depends on.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Facts<'a> {
    pub status: &'a str,
    pub last_error_code: &'a str,
    pub kind: ActionKind,
    pub undo: bool,
    pub case_voided: bool,
    /// Every attempt's outcome.
    pub attempts: &'a [AttemptOutcome],
    /// Whether the last successful attempt reported `reversal_noop`.
    pub no_change: bool,
}

/// The new state and, for unfinished Go work, which §19.9 row applied.
pub(crate) fn state(f: &Facts<'_>) -> (State, Option<&'static str>) {
    let uncertain = may_have_applied(&f.attempts.iter().copied().map(Some).collect::<Vec<_>>());
    let unconfirmed = State::Failed {
        reason: ActionFailure::Unconfirmed,
        http: None,
    };
    match f.status {
        "succeeded" => (
            State::Done {
                no_change: f.no_change,
            },
            None,
        ),
        "cancelled" if !f.undo && !uncertain => (State::Cancelled, None),
        // An undo is never cancelled (§4.7), and a cancelled original that
        // may have applied must be looked at: staff decide.
        "cancelled" => (never_ran(uncertain), Some("cancelled_needs_review")),
        "failed" => (failed(f, uncertain), None),
        "pending" | "retrying" if uncertain => {
            (unconfirmed, Some("pending_after_uncertain_attempt"))
        }
        "pending" | "retrying" if f.case_voided && !f.undo => {
            (State::Cancelled, Some("pending_on_voided_case_cancelled"))
        }
        "pending" | "retrying" => (State::Pending, Some("pending_action_resumed")),
        "running" => (unconfirmed, Some("running_action_unconfirmed")),
        _ => (unconfirmed, Some("unknown_status_unconfirmed")),
    }
}

/// `failed` with `retries_exhausted` unless an attempt may have applied.
const fn never_ran(uncertain: bool) -> State {
    State::Failed {
        reason: if uncertain {
            ActionFailure::Unconfirmed
        } else {
            ActionFailure::RetriesExhausted
        },
        http: None,
    }
}

/// A Go `failed` execution, by its `last_error_code` (§19.7 table). After an
/// attempt that may have applied, any failure is `unconfirmed`.
fn failed(f: &Facts<'_>, uncertain: bool) -> State {
    let code = f.last_error_code;
    let refused = |status: i16| State::Failed {
        reason: ActionFailure::DiscordRefused,
        http: Some(status),
    };
    let any = |classes: &[&str]| classes.iter().any(|c| has_class(code, c));
    let state = if any(&[
        "reversal_ownership_conflict",
        "reversal_provenance_unavailable",
    ]) && f.undo
    {
        return State::Skipped;
    } else if code == "case_voided" && !f.undo && !uncertain {
        return State::Cancelled;
    } else if any(&[
        "permission_or_hierarchy_denied",
        "reversal_authorization_unavailable",
        "reversal_permission_denied",
    ]) {
        refused(403)
    } else if any(&["unknown_member_or_resource"]) {
        if f.kind == ActionKind::Timeout {
            State::Failed {
                reason: ActionFailure::MemberNotInServer,
                http: None,
            }
        } else {
            refused(404)
        }
    } else if any(&[
        "validation_failed",
        "discord_failure",
        "invalid_action_config",
        "unsupported_action",
    ]) {
        refused(400)
    } else if any(&["guild_not_found"]) {
        State::Failed {
            reason: ActionFailure::QuackNotInServer,
            http: None,
        }
    } else if any(&["rate_limited", "discord_unavailable", "guild_lookup_failed"])
        || code == "case_voided"
    {
        never_ran(false)
    } else {
        // lease_expired_review_required, network and server errors, and
        // anything unrecognized: the outcome is unknown.
        State::Failed {
            reason: ActionFailure::Unconfirmed,
            http: None,
        }
    };
    if uncertain {
        State::Failed {
            reason: ActionFailure::Unconfirmed,
            http: None,
        }
    } else {
        state
    }
}

/// Go action types → kinds; `send_dm` (skipped) and unknown types are `None`.
pub(crate) fn kind(action_type: &str) -> Option<ActionKind> {
    Some(match action_type {
        "timeout_user" => ActionKind::Timeout,
        "kick_user" => ActionKind::Kick,
        "ban_user" => ActionKind::Ban,
        "remove_timeout" => ActionKind::RemoveTimeout,
        "unban_user" => ActionKind::Unban,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use AttemptOutcome as A;
    use proptest::prelude::*;
    use serde_json::json;

    fn facts<'a>(status: &'a str, code: &'a str, attempts: &'a [A]) -> Facts<'a> {
        Facts {
            status,
            last_error_code: code,
            kind: ActionKind::Ban,
            undo: false,
            case_voided: false,
            attempts,
            no_change: false,
        }
    }

    fn attempt_outcome(status: &str, code: &str, response: &Value) -> A {
        super::attempt_outcome(status, code, response, false)
    }

    #[test]
    fn uncertain_attempts_are_never_read_as_safe() {
        let none = json!({});
        // An open attempt may have applied, whatever its status says.
        assert_eq!(
            super::attempt_outcome("failed", "ban_rate_limited", &none, true),
            A::Interrupted
        );
        assert_eq!(
            super::attempt_outcome("failed", "", &none, true),
            A::Interrupted
        );
        assert_eq!(
            super::attempt_outcome("succeeded", "", &none, true),
            A::Done
        );
        // A failure with no code or an unrecognized one is unknown, not a
        // refusal: a pending execution after it isn't resumed.
        assert_eq!(attempt_outcome("failed", "", &none), A::Unknown);
        assert_eq!(
            attempt_outcome("failed", "ban_something_new", &none),
            A::Unknown
        );
        let unknown = [attempt_outcome("failed", "", &none)];
        assert_eq!(
            state(&facts("retrying", "", &unknown)).0,
            State::Failed {
                reason: ActionFailure::Unconfirmed,
                http: None
            }
        );
    }

    #[test]
    fn attempt_outcomes() {
        let none = json!({});
        assert_eq!(attempt_outcome("succeeded", "", &none), A::Done);
        assert_eq!(
            attempt_outcome("succeeded", "", &json!({"reversal_noop": true})),
            A::NoChange
        );
        assert_eq!(
            attempt_outcome("failed", "ban_rate_limited", &none),
            A::NotSent
        );
        assert_eq!(
            attempt_outcome("failed", "ban_network_error", &none),
            A::Unknown
        );
        assert_eq!(
            attempt_outcome("failed", "lease_expired", &none),
            A::Interrupted
        );
        assert_eq!(
            attempt_outcome("failed", "ban_permission_or_hierarchy_denied", &none),
            A::Refused
        );
        assert_eq!(attempt_outcome("running", "", &none), A::Interrupted);
    }

    #[test]
    fn failed_codes() {
        let s = |code: &str| state(&facts("failed", code, &[A::Refused])).0;
        assert_eq!(
            s("ban_permission_or_hierarchy_denied"),
            State::Failed {
                reason: ActionFailure::DiscordRefused,
                http: Some(403)
            }
        );
        assert_eq!(
            s("ban_unknown_member_or_resource"),
            State::Failed {
                reason: ActionFailure::DiscordRefused,
                http: Some(404)
            }
        );
        assert_eq!(
            s("ban_validation_failed"),
            State::Failed {
                reason: ActionFailure::DiscordRefused,
                http: Some(400)
            }
        );
        assert_eq!(
            s("guild_not_found"),
            State::Failed {
                reason: ActionFailure::QuackNotInServer,
                http: None
            }
        );
        assert_eq!(
            s("ban_rate_limited"),
            State::Failed {
                reason: ActionFailure::RetriesExhausted,
                http: None
            }
        );
        assert_eq!(
            s("lease_expired_review_required"),
            State::Failed {
                reason: ActionFailure::Unconfirmed,
                http: None
            }
        );
        assert_eq!(s("case_voided"), State::Cancelled);
        assert_eq!(
            s("something_new"),
            State::Failed {
                reason: ActionFailure::Unconfirmed,
                http: None
            }
        );
        let mut t = facts(
            "failed",
            "timeout_unknown_member_or_resource",
            &[A::Refused],
        );
        t.kind = ActionKind::Timeout;
        assert_eq!(
            state(&t).0,
            State::Failed {
                reason: ActionFailure::MemberNotInServer,
                http: None
            }
        );
        let mut u = facts("failed", "reversal_ownership_conflict", &[A::Refused]);
        u.undo = true;
        assert_eq!(state(&u).0, State::Skipped);
        u.last_error_code = "case_voided";
        assert_eq!(
            state(&u).0,
            State::Failed {
                reason: ActionFailure::RetriesExhausted,
                http: None
            }
        );
        // After an attempt that may have applied, a refusal is unconfirmed.
        assert_eq!(
            state(&facts(
                "failed",
                "ban_permission_or_hierarchy_denied",
                &[A::Unknown, A::Refused]
            ))
            .0,
            State::Failed {
                reason: ActionFailure::Unconfirmed,
                http: None
            }
        );
    }

    #[test]
    fn in_flight_rows() {
        assert_eq!(
            state(&facts("pending", "", &[])),
            (State::Pending, Some("pending_action_resumed"))
        );
        assert_eq!(
            state(&facts("retrying", "", &[A::NotSent])).0,
            State::Pending
        );
        assert_eq!(
            state(&facts("retrying", "", &[A::Unknown])).0,
            State::Failed {
                reason: ActionFailure::Unconfirmed,
                http: None
            }
        );
        assert_eq!(
            state(&facts("running", "", &[A::Interrupted])).0,
            State::Failed {
                reason: ActionFailure::Unconfirmed,
                http: None
            }
        );
        let mut v = facts("pending", "", &[]);
        v.case_voided = true;
        assert_eq!(state(&v).0, State::Cancelled);
        v.undo = true;
        assert_eq!(state(&v).0, State::Pending);
    }

    fn outcome() -> impl Strategy<Value = A> {
        proptest::sample::select(A::ALL.to_vec())
    }

    proptest! {
        /// The importer never leaves work a worker would replay when an
        /// attempt may already have reached Discord, and never cancels an
        /// undo or skips an original.
        #[test]
        fn nothing_uncertain_is_replayed(
            status in proptest::sample::select(vec!["pending", "retrying", "running", "failed", "succeeded", "cancelled", "weird"]),
            code in proptest::sample::select(vec!["", "ban_rate_limited", "case_voided", "reversal_ownership_conflict", "x_network_error", "guild_not_found"]),
            attempts in proptest::collection::vec(outcome(), 0..5),
            undo: bool,
            voided: bool,
        ) {
            let f = Facts { status, last_error_code: code, kind: ActionKind::Ban, undo, case_voided: voided, attempts: &attempts, no_change: false };
            let (s, _) = state(&f);
            let uncertain = attempts.iter().any(|a| a.may_have_applied());
            if s == State::Pending {
                prop_assert!(!uncertain);
                prop_assert!(matches!(status, "pending" | "retrying"));
                prop_assert!(undo || !voided);
            }
            if undo { prop_assert_ne!(s, State::Cancelled); } else { prop_assert_ne!(s, State::Skipped); }
            if status == "running" {
                let unconfirmed = matches!(s, State::Failed { reason: ActionFailure::Unconfirmed, .. });
                prop_assert!(unconfirmed);
            }
        }
    }
}
