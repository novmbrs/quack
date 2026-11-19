//! HTTP API metrics.

use std::time::Duration;

use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Histogram, Meter, ObservableGauge, UpDownCounter};

use super::observe::{Source, counter, histogram, u64_gauge, up_down_counter};
use crate::catalog;
use crate::keys;
use crate::labels::{Attribute, HttpMethod, HttpRoute, RateLimitClass, SignInOutcome};

/// HTTP API metrics: requests, requests in flight, sign-ins, rate-limit
/// rejections, and active sessions.
pub struct HttpMetrics {
    duration: Histogram<f64>,
    active: UpDownCounter<i64>,
    sign_ins: Counter<u64>,
    rate_limited: Counter<u64>,
    sessions: Source<u64>,
    _sessions_gauge: ObservableGauge<u64>,
}

impl HttpMetrics {
    pub(crate) fn new(meter: &Meter) -> Self {
        let sessions: Source<u64> = Source::new();
        let source = sessions.clone();
        let sessions_gauge = u64_gauge(meter, &catalog::SESSIONS_ACTIVE, move |observer| {
            if let Some(count) = source.read() {
                observer.observe(count, &[]);
            }
        });
        Self {
            duration: histogram(meter, &catalog::HTTP_SERVER_REQUEST_DURATION),
            active: up_down_counter(meter, &catalog::HTTP_SERVER_ACTIVE_REQUESTS),
            sign_ins: counter(meter, &catalog::SIGN_INS),
            rate_limited: counter(meter, &catalog::HTTP_RATE_LIMITED),
            sessions,
            _sessions_gauge: sessions_gauge,
        }
    }

    /// Marks a request in flight until the returned guard is dropped.
    pub fn in_flight(&self, method: HttpMethod) -> InFlightRequest {
        self.active.add(1, &[method.kv()]);
        InFlightRequest {
            active: self.active.clone(),
            method,
        }
    }

    /// Records a finished request.
    pub fn request(&self, method: HttpMethod, route: HttpRoute, status: u16, duration: Duration) {
        self.duration.record(
            duration.as_secs_f64(),
            &[
                method.kv(),
                route.kv(),
                KeyValue::new(keys::HTTP_RESPONSE_STATUS_CODE, i64::from(status)),
            ],
        );
    }

    /// Counts a dashboard sign-in.
    pub fn sign_in(&self, outcome: SignInOutcome) {
        self.sign_ins.add(1, &[outcome.kv()]);
    }

    /// Counts a request rejected by Quack's own rate limits.
    pub fn rate_limited(&self, class: RateLimitClass) {
        self.rate_limited.add(1, &[class.kv()]);
    }

    /// Installs the supplier of how many sessions are unexpired.
    pub fn observe_sessions(&self, supplier: impl Fn() -> u64 + Send + Sync + 'static) {
        self.sessions.set(supplier);
    }
}

/// Keeps one request counted in `http.server.active_requests` until dropped.
#[must_use = "the request stops counting as in flight when this is dropped"]
pub struct InFlightRequest {
    active: UpDownCounter<i64>,
    method: HttpMethod,
}

impl Drop for InFlightRequest {
    fn drop(&mut self) {
        self.active.add(-1, &[self.method.kv()]);
    }
}
