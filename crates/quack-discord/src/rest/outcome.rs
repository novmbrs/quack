//! How an HTTP answer maps to the typed outcome of one Discord call
//! (`quack_core::discord::Outcome`).

use std::time::Duration;

pub use quack_core::discord::{NotSentCause, Outcome, Refusal, RefusalKind, UnknownCause};
use quack_telemetry::labels::RestOutcome;
use serde::Deserialize;

/// The outcome class recorded on metrics and spans.
pub(crate) trait OutcomeClass {
    /// The class of this outcome.
    fn class(&self) -> RestOutcome;
}

impl<T> OutcomeClass for Outcome<T> {
    fn class(&self) -> RestOutcome {
        match self {
            Outcome::Done(_) => RestOutcome::Done,
            Outcome::Refused(_) => RestOutcome::Refused,
            Outcome::RetryAfter(_) => RestOutcome::RateLimited,
            Outcome::NotSent(_) => RestOutcome::NotSent,
            Outcome::Unknown(_) => RestOutcome::Unknown,
        }
    }
}

/// Discord's JSON error body (`{"code": 50013, "message": …}`), or a 429
/// body (`{"retry_after": 1.5, "global": false}`).
#[derive(Deserialize)]
struct ErrorBody {
    code: Option<u32>,
    retry_after: Option<f64>,
}

/// The shortest delay a 429 is reported with, so a `retry_after` of zero
/// never becomes a busy loop.
const MIN_RETRY_AFTER: Duration = Duration::from_millis(50);

/// The delay reported for a 429 that says nothing usable.
const DEFAULT_RETRY_AFTER: Duration = Duration::from_secs(1);

fn seconds(value: f64) -> Option<Duration> {
    (value.is_finite() && (0.0..86_400.0).contains(&value)).then(|| Duration::from_secs_f64(value))
}

/// Classifies a non-success answer. `retry_after_header` is the raw
/// `Retry-After` header, used when a 429 body has no `retry_after`.
pub(crate) fn classify_failure<T>(
    status: u16,
    body: &[u8],
    retry_after_header: Option<&str>,
) -> Outcome<T> {
    let parsed: Option<ErrorBody> = serde_json::from_slice(body).ok();
    match status {
        429 => {
            let delay = parsed
                .as_ref()
                .and_then(|b| b.retry_after)
                .and_then(seconds)
                .or_else(|| {
                    retry_after_header
                        .and_then(|h| h.trim().parse::<f64>().ok())
                        .and_then(seconds)
                })
                .unwrap_or(DEFAULT_RETRY_AFTER);
            Outcome::RetryAfter(delay.max(MIN_RETRY_AFTER))
        }
        // A 4xx is Discord's definite refusal; 304 is its documented "no
        // action was taken".
        304 | 400..=499 => Outcome::Refused(Refusal::new(status, parsed.and_then(|b| b.code))),
        // Anything else (a 5xx, a redirect from an intermediary, a status
        // nobody documents) says nothing about whether the request took
        // effect.
        _ => Outcome::Unknown(UnknownCause::ServerError(status)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refused(status: u16, body: &str) -> RefusalKind {
        match classify_failure::<()>(status, body.as_bytes(), None) {
            Outcome::Refused(r) => r.kind,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn every_known_code_maps_to_its_kind() {
        let cases = [
            (403, 50013, RefusalKind::MissingPermissions),
            (403, 50001, RefusalKind::MissingAccess),
            (403, 50007, RefusalKind::CannotDm),
            (400, 50278, RefusalKind::NoMutualGuilds),
            (404, 10004, RefusalKind::UnknownGuild),
            (404, 10007, RefusalKind::UnknownMember),
            (404, 10013, RefusalKind::UnknownUser),
            (404, 10026, RefusalKind::UnknownBan),
            (404, 10008, RefusalKind::UnknownMessage),
            (404, 10003, RefusalKind::UnknownChannel),
            (413, 40005, RefusalKind::TooLarge),
            (400, 40005, RefusalKind::TooLarge),
        ];
        for (status, code, kind) in cases {
            let body = format!(r#"{{"code": {code}, "message": "x"}}"#);
            assert_eq!(refused(status, &body), kind, "{status} {code}");
        }
    }

    #[test]
    fn other_client_errors_are_refusals() {
        assert_eq!(refused(413, ""), RefusalKind::TooLarge);
        assert_eq!(refused(400, r#"{"code": 50035}"#), RefusalKind::Other);
        // An "unknown" code means absent only with its 404.
        assert_eq!(refused(400, r#"{"code": 10007}"#), RefusalKind::Other);
        assert_eq!(refused(403, r#"{"code": 10003}"#), RefusalKind::Other);
        assert_eq!(refused(404, "<html>"), RefusalKind::Other);
        assert_eq!(refused(401, r#"{"code": 0}"#), RefusalKind::Other);
        assert_eq!(refused(403, ""), RefusalKind::Other);
        let r = Refusal::new(403, Some(50001));
        assert_eq!(r.machine_code(), "http_403_50001");
        assert_eq!(Refusal::new(400, None).machine_code(), "http_400");
    }

    #[test]
    fn server_errors_are_unknown() {
        for status in [500, 502, 503, 504, 599] {
            assert_eq!(
                classify_failure::<()>(status, b"{}", None),
                Outcome::Unknown(UnknownCause::ServerError(status))
            );
        }
    }

    #[test]
    fn too_many_requests_carries_the_delay() {
        let body =
            br#"{"message": "You are being rate limited.", "retry_after": 2.5, "global": false}"#;
        assert_eq!(
            classify_failure::<()>(429, body, Some("3")),
            Outcome::RetryAfter(Duration::from_millis(2500))
        );
        assert_eq!(
            classify_failure::<()>(429, b"", Some("3")),
            Outcome::RetryAfter(Duration::from_secs(3))
        );
        assert_eq!(
            classify_failure::<()>(429, b"", None),
            Outcome::RetryAfter(DEFAULT_RETRY_AFTER)
        );
        assert_eq!(
            classify_failure::<()>(429, br#"{"retry_after": 0}"#, None),
            Outcome::RetryAfter(MIN_RETRY_AFTER)
        );
        assert_eq!(
            classify_failure::<()>(429, br#"{"retry_after": -1}"#, Some("nan")),
            Outcome::RetryAfter(DEFAULT_RETRY_AFTER)
        );
    }

    #[test]
    fn combinators_and_classes() {
        let done: Outcome<u8> = Outcome::Done(2);
        assert_eq!(done.clone().map(u16::from), Outcome::Done(2_u16));
        assert_eq!(
            done.clone().and_then(|v| Outcome::Done(v + 1)).done(),
            Some(3)
        );
        assert_eq!(done.class(), RestOutcome::Done);
        let refused: Outcome<u8> = Outcome::Refused(Refusal::new(404, Some(10007)));
        assert_eq!(refused.refusal(), Some(RefusalKind::UnknownMember));
        assert_eq!(refused.class(), RestOutcome::Refused);
        assert_eq!(
            Outcome::<()>::RetryAfter(Duration::ZERO).class(),
            RestOutcome::RateLimited
        );
        assert_eq!(
            Outcome::<()>::NotSent(NotSentCause::Connection).class(),
            RestOutcome::NotSent
        );
        assert!(!Outcome::<()>::Unknown(UnknownCause::Timeout).is_done());
    }
}
