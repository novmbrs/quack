//! Sending one request: rate limiter, deadline, HTTP, classification, and
//! telemetry.
//!
//! Twilight's `twilight-http` client retries every 429 internally without a
//! bound, puts no deadline on reading the body, and cannot send string
//! nonces with `enforce_nonce`, so Quack sends requests itself with
//! `reqwest`, using Twilight's rate limiter for buckets. Nothing here retries:
//! a 429 comes back as `RetryAfter` and the caller's policy decides.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use quack_telemetry::labels::RateLimitScope;
use quack_telemetry::spans::SpanEvent;
use quack_telemetry::{metrics, spans};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, RETRY_AFTER, USER_AGENT};
use tokio::sync::oneshot;
use tracing::{Instrument as _, Span};
use twilight_http_ratelimiting::{Endpoint, Permit, RateLimitHeaders, RateLimiter};

use super::outcome::{NotSentCause, Outcome, UnknownCause, classify_failure};
use super::route::{Route, encode_component};

/// What Discord's API expects in `User-Agent`.
const QUACK_USER_AGENT: &str = concat!(
    "DiscordBot (https://github.com/quackdiscord/bot, ",
    env!("CARGO_PKG_VERSION"),
    ")"
);

/// Most characters in an `X-Audit-Log-Reason` (before encoding).
const AUDIT_LOG_REASON_CHARS: usize = 512;

/// A request body.
#[derive(Debug)]
pub(crate) enum Body {
    /// No body.
    Empty,
    /// A JSON document.
    Json(Vec<u8>),
    /// `multipart/form-data` with this boundary.
    Multipart { boundary: String, bytes: Vec<u8> },
}

/// One call to make.
#[derive(Debug)]
pub(crate) struct Request<'a> {
    pub(crate) route: Route,
    pub(crate) body: Body,
    /// The audit-log reason for mutating moderation calls.
    pub(crate) reason: Option<&'a str>,
    /// Whether the call carries files (the longer deadline applies).
    pub(crate) files: bool,
}

impl Request<'_> {
    pub(crate) const fn new(route: Route) -> Self {
        Self {
            route,
            body: Body::Empty,
            reason: None,
            files: false,
        }
    }
}

/// Sends requests to Discord's API.
#[derive(Debug)]
pub(crate) struct Transport {
    pub(crate) http: reqwest::Client,
    pub(crate) api_base: String,
    pub(crate) authorization: quack_core::Secret,
    pub(crate) limiter: RateLimiter,
    pub(crate) closed: AtomicBool,
    pub(crate) request_timeout: Duration,
    pub(crate) file_timeout: Duration,
    pub(crate) rate_limit_budget: Duration,
}

/// What Discord answered, before classification.
struct Answer {
    status: Option<u16>,
    outcome: Outcome<Vec<u8>>,
}

impl Answer {
    const fn without_status(outcome: Outcome<Vec<u8>>) -> Self {
        Self {
            status: None,
            outcome,
        }
    }
}

/// Encodes an audit-log reason: cut to 512 characters, then percent-encoded
/// UTF-8 as Discord expects.
pub(crate) fn encode_reason(reason: &str) -> String {
    let cut: String = reason.chars().take(AUDIT_LOG_REASON_CHARS).collect();
    encode_component(&cut)
}

/// Reads the bucket headers so the limiter learns this route's limit.
/// A 429 with `X-RateLimit-Scope: global` teaches nothing per route; a
/// shared 429 exhausts the bucket until `Retry-After`.
fn rate_limit_headers(headers: &HeaderMap) -> Option<RateLimitHeaders> {
    let text = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    let bucket = text(RateLimitHeaders::BUCKET)?.as_bytes().to_vec();
    match text(RateLimitHeaders::SCOPE) {
        Some("global") => None,
        Some("shared") => {
            let retry_after = text(RETRY_AFTER.as_str())?.trim().parse().ok()?;
            Some(RateLimitHeaders::shared(bucket, retry_after))
        }
        _ => {
            let limit = text(RateLimitHeaders::LIMIT)?.trim().parse().ok()?;
            let remaining = text(RateLimitHeaders::REMAINING)?.trim().parse().ok()?;
            let reset_after: f64 = text(RateLimitHeaders::RESET_AFTER)?.trim().parse().ok()?;
            let reset_after = (reset_after.is_finite() && (0.0..86_400.0).contains(&reset_after))
                .then(|| Duration::from_secs_f64(reset_after))?;
            Some(RateLimitHeaders {
                bucket,
                limit,
                remaining,
                reset_at: Instant::now() + reset_after,
            })
        }
    }
}

/// Which limit a 429 was for: `X-RateLimit-Scope` (`user` is a route
/// bucket), or `X-RateLimit-Global: true` on older answers.
fn limit_scope(headers: &HeaderMap) -> RateLimitScope {
    let text = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    match text(RateLimitHeaders::SCOPE) {
        Some("global") => RateLimitScope::Global,
        Some("shared") => RateLimitScope::Shared,
        _ if text("x-ratelimit-global") == Some("true") => RateLimitScope::Global,
        _ => RateLimitScope::Route,
    }
}

impl Transport {
    /// Sends one request and classifies the answer. `Done` carries the
    /// response body.
    pub(crate) async fn send(&self, request: Request<'_>) -> Outcome<Vec<u8>> {
        let route = &request.route;
        let span = spans::discord(route.method.telemetry(), route.url_template());
        let method = route.method.telemetry();
        let template = route.url_template();
        let started = Instant::now();
        let answer = self.send_in(request, &span).instrument(span.clone()).await;
        let class = answer.outcome.class();
        if let Some(status) = answer.status {
            spans::record_status(&span, status);
        }
        spans::record(&span, class);
        if !answer.outcome.is_done() {
            spans::record_error(&span, class.to_string());
        }
        metrics()
            .discord
            .request(method, template, answer.status, class, started.elapsed());
        answer.outcome
    }

    async fn send_in(&self, request: Request<'_>, span: &Span) -> Answer {
        if self.closed.load(Ordering::Acquire) {
            return Answer::without_status(Outcome::NotSent(NotSentCause::ShuttingDown));
        }
        let permit = match self.permit(&request.route, span).await {
            Ok(permit) => permit,
            Err(outcome) => return Answer::without_status(outcome),
        };
        let deadline = tokio::time::Instant::now()
            + if request.files {
                self.file_timeout
            } else {
                self.request_timeout
            };
        let route_template = request.route.url_template();
        let Some(http_request) = self.build(request) else {
            return Answer::without_status(Outcome::NotSent(NotSentCause::Invalid));
        };
        let response =
            match tokio::time::timeout_at(deadline, self.http.execute(http_request)).await {
                Err(_) => return Answer::without_status(Outcome::Unknown(UnknownCause::Timeout)),
                Ok(Err(error)) => return Answer::without_status(error_outcome(&error)),
                Ok(Ok(response)) => response,
            };
        let status = response.status().as_u16();
        permit.complete(rate_limit_headers(response.headers()));
        let retry_after = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let scope = limit_scope(response.headers());
        let success = (200..300).contains(&status);
        let body = match tokio::time::timeout_at(deadline, response.bytes()).await {
            Ok(Ok(bytes)) => bytes.to_vec(),
            // A success whose body was lost: the change happened but Quack
            // doesn't know what Discord returned.
            Err(_) if success => {
                return Answer {
                    status: Some(status),
                    outcome: Outcome::Unknown(UnknownCause::Timeout),
                };
            }
            Ok(Err(_)) if success => {
                return Answer {
                    status: Some(status),
                    outcome: Outcome::Unknown(UnknownCause::Connection),
                };
            }
            // A failure status is definite even without its body.
            Err(_) | Ok(Err(_)) => Vec::new(),
        };
        let outcome = if success {
            Outcome::Done(body)
        } else {
            classify_failure(status, &body, retry_after.as_deref())
        };
        if let Outcome::RetryAfter(delay) = outcome {
            // A 429 is counted by its status on the request metric; the hit
            // is also counted by scope, so global limits can be alerted on.
            metrics().discord.rate_limited(route_template, scope, delay);
            spans::event(span, SpanEvent::RateLimited, Vec::new());
        }
        if !outcome.is_done() {
            tracing::debug!(status, "discord answered with a failure");
        }
        Answer {
            status: Some(status),
            outcome,
        }
    }

    /// Waits for the route's rate-limit permit, at most the budget. A bucket
    /// that resets later than the budget answers `RetryAfter` at once.
    async fn permit(&self, route: &Route, span: &Span) -> Result<Permit, Outcome<Vec<u8>>> {
        let endpoint = Endpoint {
            method: route.method.limiter(),
            path: route.limit_path.clone(),
        };
        let budget = self.rate_limit_budget;
        let (seen_tx, mut seen_rx) = oneshot::channel::<Option<Duration>>();
        let started = Instant::now();
        let acquire = self.limiter.acquire_if(endpoint, move |bucket| {
            let wait = bucket
                .filter(|b| b.remaining == 0)
                .map(|b| b.reset_at.saturating_duration_since(Instant::now()));
            let _ = seen_tx.send(wait);
            wait.is_none_or(|w| w <= budget)
        });
        let result = tokio::time::timeout(budget, acquire).await;
        let waited = started.elapsed();
        let bucket_wait = seen_rx.try_recv().ok().flatten();
        if bucket_wait.is_some() {
            metrics()
                .discord
                .rate_limited(route.url_template(), RateLimitScope::Route, waited);
            spans::record_rate_limit_wait(span, waited);
            spans::event(span, SpanEvent::RateLimited, Vec::new());
        }
        match result {
            Ok(Some(permit)) => Ok(permit),
            Ok(None) => Err(Outcome::RetryAfter(bucket_wait.unwrap_or(budget))),
            Err(_) => Err(Outcome::RetryAfter(
                bucket_wait
                    .map_or(Duration::from_secs(1), |w| w.saturating_sub(waited))
                    .max(Duration::from_millis(50)),
            )),
        }
    }

    fn build(&self, request: Request<'_>) -> Option<reqwest::Request> {
        let route = request.route;
        let mut url = format!("{}{}", self.api_base, route.path);
        if let Some(query) = &route.query {
            url.push('?');
            url.push_str(query);
        }
        let mut builder = self
            .http
            .request(route.method.reqwest(), url)
            .header(USER_AGENT, QUACK_USER_AGENT);
        if route.auth {
            builder = builder.header(AUTHORIZATION, self.authorization.expose());
        }
        if let Some(reason) = request.reason {
            builder = builder.header("X-Audit-Log-Reason", encode_reason(reason));
        }
        builder = match request.body {
            Body::Empty => builder.body(Vec::new()),
            Body::Json(bytes) => builder.header(CONTENT_TYPE, "application/json").body(bytes),
            Body::Multipart { boundary, bytes } => builder
                .header(
                    CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(bytes),
        };
        builder.build().ok()
    }
}

/// Classifies a transport error: a failed connect never sent anything;
/// anything later may have reached Discord.
pub(crate) fn error_outcome<T>(error: &reqwest::Error) -> Outcome<T> {
    if error.is_connect() {
        Outcome::NotSent(NotSentCause::Connection)
    } else if error.is_timeout() {
        Outcome::Unknown(UnknownCause::Timeout)
    } else if error.is_builder() {
        Outcome::NotSent(NotSentCause::Invalid)
    } else {
        Outcome::Unknown(UnknownCause::Connection)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, HeaderValue::from_static(value));
        }
        map
    }

    #[test]
    fn reads_bucket_headers() {
        let user = rate_limit_headers(&headers(&[
            ("x-ratelimit-bucket", "abc"),
            ("x-ratelimit-limit", "5"),
            ("x-ratelimit-remaining", "0"),
            ("x-ratelimit-reset-after", "1.5"),
        ]))
        .unwrap();
        assert_eq!((user.limit, user.remaining), (5, 0));
        assert_eq!(user.bucket, b"abc");
        let shared = rate_limit_headers(&headers(&[
            ("x-ratelimit-bucket", "abc"),
            ("x-ratelimit-scope", "shared"),
            ("retry-after", "4"),
        ]))
        .unwrap();
        assert_eq!(shared.remaining, 0);
        assert!(
            rate_limit_headers(&headers(&[
                ("x-ratelimit-bucket", "abc"),
                ("x-ratelimit-scope", "global"),
            ]))
            .is_none()
        );
        assert!(rate_limit_headers(&headers(&[("x-ratelimit-limit", "5")])).is_none());
        assert!(
            rate_limit_headers(&headers(&[
                ("x-ratelimit-bucket", "abc"),
                ("x-ratelimit-limit", "5"),
                ("x-ratelimit-remaining", "0"),
                ("x-ratelimit-reset-after", "-1"),
            ]))
            .is_none()
        );
    }

    #[test]
    fn reads_the_scope_of_a_429() {
        assert_eq!(
            limit_scope(&headers(&[("x-ratelimit-scope", "global")])),
            RateLimitScope::Global
        );
        assert_eq!(
            limit_scope(&headers(&[("x-ratelimit-global", "true")])),
            RateLimitScope::Global
        );
        assert_eq!(
            limit_scope(&headers(&[("x-ratelimit-scope", "shared")])),
            RateLimitScope::Shared
        );
        assert_eq!(
            limit_scope(&headers(&[("x-ratelimit-scope", "user")])),
            RateLimitScope::Route
        );
        assert_eq!(limit_scope(&headers(&[])), RateLimitScope::Route);
    }

    #[test]
    fn audit_log_reasons_are_cut_and_encoded() {
        assert_eq!(
            encode_reason("Quack case #4: Spam"),
            "Quack%20case%20%234%3A%20Spam"
        );
        let long = "é".repeat(600);
        assert_eq!(encode_reason(&long), "%C3%A9".repeat(512));
    }
}
