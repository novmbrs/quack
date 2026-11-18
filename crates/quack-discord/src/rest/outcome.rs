//! The typed result of one Discord call, and how an HTTP answer maps to it.
//!
//! Getting this right is what makes retries safe: `Refused` means Discord
//! definitely changed nothing, `NotSent` means the request never left Quack,
//! and `Unknown` means it may have taken effect (§5.2, §6.4, §10.4).

use std::time::Duration;

use quack_telemetry::labels::RestOutcome;
use serde::Deserialize;

/// The typed result of one Discord call (§17.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome<T> {
    /// Discord confirmed it.
    Done(T),
    /// Discord definitely refused; nothing changed.
    Refused(Refusal),
    /// Rate limited before anything was sent; try again after the delay.
    RetryAfter(Duration),
    /// The request was never sent: nothing changed.
    NotSent(NotSentCause),
    /// The request may or may not have taken effect.
    Unknown(UnknownCause),
}

impl<T> Outcome<T> {
    /// Maps the confirmed value.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Outcome<U> {
        match self {
            Self::Done(v) => Outcome::Done(f(v)),
            Self::Refused(r) => Outcome::Refused(r),
            Self::RetryAfter(d) => Outcome::RetryAfter(d),
            Self::NotSent(c) => Outcome::NotSent(c),
            Self::Unknown(c) => Outcome::Unknown(c),
        }
    }

    /// Chains a second step that runs only when this one is `Done`.
    pub fn and_then<U>(self, f: impl FnOnce(T) -> Outcome<U>) -> Outcome<U> {
        match self {
            Self::Done(v) => f(v),
            Self::Refused(r) => Outcome::Refused(r),
            Self::RetryAfter(d) => Outcome::RetryAfter(d),
            Self::NotSent(c) => Outcome::NotSent(c),
            Self::Unknown(c) => Outcome::Unknown(c),
        }
    }

    /// The confirmed value, or the same failure for another value type, so
    /// a multi-step call can stop at its first failure.
    pub fn into_result<U>(self) -> Result<T, Outcome<U>> {
        match self {
            Self::Done(v) => Ok(v),
            Self::Refused(r) => Err(Outcome::Refused(r)),
            Self::RetryAfter(d) => Err(Outcome::RetryAfter(d)),
            Self::NotSent(c) => Err(Outcome::NotSent(c)),
            Self::Unknown(c) => Err(Outcome::Unknown(c)),
        }
    }

    /// The confirmed value, if any.
    pub fn done(self) -> Option<T> {
        match self {
            Self::Done(v) => Some(v),
            _ => None,
        }
    }

    /// Whether Discord confirmed it.
    pub const fn is_done(&self) -> bool {
        matches!(self, Self::Done(_))
    }

    /// The refusal kind, when Discord refused.
    pub const fn refusal(&self) -> Option<RefusalKind> {
        match self {
            Self::Refused(r) => Some(r.kind),
            _ => None,
        }
    }

    /// The outcome class recorded on metrics and spans.
    pub const fn class(&self) -> RestOutcome {
        match self {
            Self::Done(_) => RestOutcome::Done,
            Self::Refused(_) => RestOutcome::Refused,
            Self::RetryAfter(_) => RestOutcome::RateLimited,
            Self::NotSent(_) => RestOutcome::NotSent,
            Self::Unknown(_) => RestOutcome::Unknown,
        }
    }
}

/// A definite refusal: the HTTP status, Discord's JSON error code, and what
/// it means for Quack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Refusal {
    /// The HTTP status (4xx).
    pub status: u16,
    /// Discord's JSON error code, when the body carried one.
    pub code: Option<u32>,
    /// What the refusal means.
    pub kind: RefusalKind,
}

impl Refusal {
    /// A refusal from a status and optional JSON error code.
    pub const fn new(status: u16, code: Option<u32>) -> Self {
        Self {
            status,
            code,
            kind: RefusalKind::from_answer(status, code),
        }
    }

    /// The machine code stored in `last_error` columns, such as
    /// `http_403_50001` (§10.1).
    pub fn machine_code(&self) -> String {
        match self.code {
            Some(code) => format!("http_{}_{code}", self.status),
            None => format!("http_{}", self.status),
        }
    }
}

/// What a definite refusal means (§17.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RefusalKind {
    /// 50013: Quack lacks a permission.
    MissingPermissions,
    /// 50001: Quack can't see the channel or resource.
    MissingAccess,
    /// 50007: the member's DMs are closed or they blocked Quack.
    CannotDm,
    /// 50278: Quack shares no server with the member.
    NoMutualGuilds,
    /// 10004: Quack is not in the server.
    UnknownGuild,
    /// 10007: not a member of the server.
    UnknownMember,
    /// 10013: no such account.
    UnknownUser,
    /// 10026: no such ban.
    UnknownBan,
    /// 10008: no such message.
    UnknownMessage,
    /// 10003: no such channel.
    UnknownChannel,
    /// 413, or 40005: the request or a file is too large.
    TooLarge,
    /// Any other definite refusal (4xx except 429).
    Other,
}

impl RefusalKind {
    /// Maps Discord's answer. The JSON code decides when it is one Quack
    /// knows; otherwise a bare 413 is `TooLarge` and anything else `Other`.
    pub const fn from_answer(status: u16, code: Option<u32>) -> Self {
        match code {
            Some(50013) => Self::MissingPermissions,
            Some(50001) => Self::MissingAccess,
            Some(50007) => Self::CannotDm,
            Some(50278) => Self::NoMutualGuilds,
            Some(10004) => Self::UnknownGuild,
            Some(10007) => Self::UnknownMember,
            Some(10013) => Self::UnknownUser,
            Some(10026) => Self::UnknownBan,
            Some(10008) => Self::UnknownMessage,
            Some(10003) => Self::UnknownChannel,
            Some(40005) => Self::TooLarge,
            _ if status == 413 => Self::TooLarge,
            _ => Self::Other,
        }
    }

    /// Whether the thing asked about does not exist (any `Unknown…` code).
    pub const fn is_unknown_resource(self) -> bool {
        matches!(
            self,
            Self::UnknownGuild
                | Self::UnknownMember
                | Self::UnknownUser
                | Self::UnknownBan
                | Self::UnknownMessage
                | Self::UnknownChannel
        )
    }
}

/// Why a request may have taken effect without Quack learning the answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnknownCause {
    /// The deadline passed after the request was sent.
    Timeout,
    /// Discord answered 5xx.
    ServerError(u16),
    /// The connection broke after the request was sent.
    Connection,
    /// Discord answered success with a body Quack could not read.
    Malformed,
}

/// Why a request never left Quack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotSentCause {
    /// Connecting failed (DNS, TCP, or TLS) before anything was sent.
    Connection,
    /// The client is shutting down and accepts no new calls.
    ShuttingDown,
    /// Quack refused to build the request (a disallowed download URL, an
    /// unencodable header). Retrying will not help.
    Invalid,
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
        assert!(RefusalKind::UnknownMember.is_unknown_resource());
        assert!(!RefusalKind::MissingAccess.is_unknown_resource());
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
