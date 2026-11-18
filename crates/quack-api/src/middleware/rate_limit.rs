//! In-process rate limits (§14.7): a token bucket per class and key, at
//! most 100,000 keys per class (least recently used evicted). Signed-in
//! requests are keyed by account (and server, for server writes); every
//! other request by client IP, so an invalid cookie can't mint buckets.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, FromRequestParts, MatchedPath, RawPathParams, Request, State};
use axum::http::HeaderMap;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use quack_telemetry::labels::RateLimitClass;
use quack_telemetry::metrics;
use quack_text::api::ApiCode;

use crate::auth::session::SessionLookup;
use crate::error::ApiError;
use crate::routes::rate_class;
use crate::state::ApiState;

/// Most keys one class tracks.
const CAPACITY: usize = 100_000;

/// A route's rate-limit class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum RateClass {
    /// Sign-in: 20 per 10 minutes per client IP.
    SignIn,
    /// Reads: 120 per minute per account.
    Read,
    /// Server writes: 30 per minute per account and server.
    Write,
    /// Adding cases: 20 per minute per account and server.
    CaseCreate,
    /// Retry, dismiss, undo: 10 per minute per account and server.
    ActionControl,
    /// Member writes: 10 per minute per account.
    MemberWrite,
    /// Not limited (health probes, dashboard files).
    None,
}

impl RateClass {
    const ALL: [Self; 6] = [
        Self::SignIn,
        Self::Read,
        Self::Write,
        Self::CaseCreate,
        Self::ActionControl,
        Self::MemberWrite,
    ];

    /// The class's metric label.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::SignIn => "sign_in",
            Self::Read => "read",
            Self::Write => "write",
            Self::CaseCreate => "case_create",
            Self::ActionControl => "action_control",
            Self::MemberWrite => "member_write",
            Self::None => "none",
        }
    }

    /// Requests allowed per window.
    const fn limit(self) -> Option<(u32, Duration)> {
        match self {
            Self::SignIn => Some((20, Duration::from_secs(600))),
            Self::Read => Some((120, Duration::from_secs(60))),
            Self::Write => Some((30, Duration::from_secs(60))),
            Self::CaseCreate => Some((20, Duration::from_secs(60))),
            Self::ActionControl | Self::MemberWrite => Some((10, Duration::from_secs(60))),
            Self::None => None,
        }
    }

    const fn per_server(self) -> bool {
        matches!(self, Self::Write | Self::CaseCreate | Self::ActionControl)
    }
}

#[derive(Clone, Copy, Debug)]
struct Bucket {
    tokens: f64,
    at: Instant,
}

/// One class's buckets.
#[derive(Debug, Default)]
struct Buckets {
    map: Mutex<HashMap<String, Bucket>>,
}

impl Buckets {
    /// Takes a token for `key`, or says how many seconds until one is free.
    fn take(&self, key: String, capacity: u32, window: Duration, now: Instant) -> Result<(), u64> {
        let Ok(mut map) = self.map.lock() else {
            return Ok(());
        };
        let rate = f64::from(capacity) / window.as_secs_f64();
        if !map.contains_key(&key)
            && map.len() >= CAPACITY
            && let Some(oldest) = map.iter().min_by_key(|(_, b)| b.at).map(|(k, _)| k.clone())
        {
            map.remove(&oldest);
        }
        let bucket = map.entry(key).or_insert(Bucket {
            tokens: f64::from(capacity),
            at: now,
        });
        let refill = now.saturating_duration_since(bucket.at).as_secs_f64() * rate;
        bucket.tokens = (bucket.tokens + refill).min(f64::from(capacity));
        bucket.at = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            Ok(())
        } else {
            let wait = ((1.0 - bucket.tokens) / rate).ceil();
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "wait is a small positive number of seconds"
            )]
            Err((wait as u64).max(1))
        }
    }
}

/// Every class's buckets.
#[derive(Debug, Default)]
pub(crate) struct RateLimits {
    classes: HashMap<RateClass, Buckets>,
}

impl RateLimits {
    pub(crate) fn new() -> Self {
        Self {
            classes: RateClass::ALL
                .into_iter()
                .map(|c| (c, Buckets::default()))
                .collect(),
        }
    }

    /// Takes a token of `class` for `key`.
    pub(crate) fn check(&self, class: RateClass, key: String) -> Result<(), u64> {
        match (class.limit(), self.classes.get(&class)) {
            (Some((capacity, window)), Some(buckets)) => {
                buckets.take(key, capacity, window, Instant::now())
            }
            _ => Ok(()),
        }
    }
}

/// The client's address: the socket peer, or with `hops` trusted proxies
/// the `hops`-th address from the right of `X-Forwarded-For`.
pub(crate) fn client_ip(headers: &HeaderMap, peer: Option<SocketAddr>, hops: u8) -> Option<IpAddr> {
    if hops > 0 {
        let forwarded: Vec<&str> = headers
            .get_all("x-forwarded-for")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','))
            .map(str::trim)
            .collect();
        if let Some(index) = forwarded.len().checked_sub(usize::from(hops))
            && let Some(ip) = forwarded.get(index).and_then(|s| s.parse().ok())
        {
            return Some(ip);
        }
    }
    peer.map(|p| p.ip())
}

/// Applies the matched route's class.
pub(crate) async fn layer(State(state): State<ApiState>, request: Request, next: Next) -> Response {
    let matched = request
        .extensions()
        .get::<MatchedPath>()
        .map(|m| m.as_str().to_owned());
    let class = rate_class(request.method(), matched.as_deref());
    if class == RateClass::None {
        return next.run(request).await;
    }
    let (mut parts, body) = request.into_parts();
    let account = parts
        .extensions
        .get::<SessionLookup>()
        .and_then(SessionLookup::account);
    let key = match account {
        Some(account) if class != RateClass::SignIn => {
            let mut key = format!("a:{account}");
            if class.per_server() {
                let guild = RawPathParams::from_request_parts(&mut parts, &())
                    .await
                    .ok()
                    .and_then(|p| {
                        p.iter()
                            .find(|(k, _)| *k == "guild_id")
                            .map(|(_, v)| guild_bucket(v))
                    });
                key.push_str(":g:");
                key.push_str(&guild.unwrap_or_default());
            }
            key
        }
        _ => {
            let peer = parts
                .extensions
                .get::<ConnectInfo<SocketAddr>>()
                .map(|c| c.0);
            let ip = client_ip(&parts.headers, peer, state.config().trusted_proxy_hops);
            format!(
                "ip:{}",
                ip.map_or_else(|| "unknown".to_owned(), |ip| ip.to_string())
            )
        }
    };
    if let Err(seconds) = state.limits().check(class, key) {
        metrics()
            .http
            .rate_limited(RateLimitClass::new(class.name()));
        return ApiError::api(ApiCode::RateLimited(seconds)).into_response();
    }
    next.run(Request::from_parts(parts, body)).await
}

/// A server's part of a rate-limit key, canonical as the staff extractor
/// parses it: every spelling of one server is one bucket, and every
/// malformed one shares the account's empty bucket.
fn guild_bucket(raw: &str) -> String {
    raw.parse::<quack_core::ids::GuildId>()
        .map(|g| g.get().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn spellings_of_one_server_share_a_bucket() {
        assert_eq!(super::guild_bucket("1001"), super::guild_bucket("0001001"));
        assert_eq!(super::guild_bucket("x"), super::guild_bucket("%31"));
        assert_ne!(super::guild_bucket("1001"), super::guild_bucket("1002"));
    }

    #[test]
    fn buckets_refill_and_say_when() {
        let b = Buckets::default();
        let now = Instant::now();
        let window = Duration::from_secs(60);
        for _ in 0..10 {
            assert!(b.take("k".into(), 10, window, now).is_ok());
        }
        assert_eq!(b.take("k".into(), 10, window, now), Err(6));
        assert!(b.take("other".into(), 10, window, now).is_ok());
        assert!(
            b.take("k".into(), 10, window, now + Duration::from_secs(6))
                .is_ok()
        );
    }

    #[test]
    fn client_ip_honors_trusted_hops_only() {
        let peer: SocketAddr = "10.0.0.1:5000".parse().unwrap_or_else(|_| unreachable!());
        let mut h = HeaderMap::new();
        h.insert(
            "x-forwarded-for",
            HeaderValue::from_static("1.1.1.1, 2.2.2.2, 3.3.3.3"),
        );
        assert_eq!(client_ip(&h, Some(peer), 0), Some(peer.ip()));
        assert_eq!(client_ip(&h, Some(peer), 1), "3.3.3.3".parse().ok());
        assert_eq!(client_ip(&h, Some(peer), 2), "2.2.2.2".parse().ok());
        assert_eq!(client_ip(&h, Some(peer), 5), Some(peer.ip()));
    }
}
