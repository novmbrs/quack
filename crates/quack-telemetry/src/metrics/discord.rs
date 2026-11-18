//! Discord REST and gateway metrics.

use std::time::Duration;

use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Histogram, Meter, ObservableGauge};

use super::observe::{Source, counter, histogram, u64_gauge};
use crate::catalog;
use crate::keys;
use crate::labels::{
    Attribute, GatewayConnectionEvent, GatewayEvent, HttpMethod, RateLimitScope, RestOutcome,
    UrlTemplate,
};

/// Discord REST metrics: calls, rate-limit hits, and rate-limit waits.
pub struct DiscordMetrics {
    requests: Histogram<f64>,
    rate_limit_hits: Counter<u64>,
    rate_limit_wait: Histogram<f64>,
}

impl DiscordMetrics {
    pub(crate) fn new(meter: &Meter) -> Self {
        Self {
            requests: histogram(meter, &catalog::HTTP_CLIENT_REQUEST_DURATION),
            rate_limit_hits: counter(meter, &catalog::DISCORD_RATE_LIMIT_HITS),
            rate_limit_wait: histogram(meter, &catalog::DISCORD_RATE_LIMIT_WAIT),
        }
    }

    /// Records one REST call. `status` is `None` when no response arrived.
    pub fn request(
        &self,
        method: HttpMethod,
        route: UrlTemplate,
        status: Option<u16>,
        outcome: RestOutcome,
        duration: Duration,
    ) {
        let seconds = duration.as_secs_f64();
        match status {
            Some(status) => self.requests.record(
                seconds,
                &[
                    method.kv(),
                    route.kv(),
                    KeyValue::new(keys::HTTP_RESPONSE_STATUS_CODE, i64::from(status)),
                    outcome.kv(),
                ],
            ),
            None => self
                .requests
                .record(seconds, &[method.kv(), route.kv(), outcome.kv()]),
        }
    }

    /// Records a call that waited for a rate limit, and how long it waited.
    pub fn rate_limited(&self, route: UrlTemplate, scope: RateLimitScope, wait: Duration) {
        let attributes = [route.kv(), scope.kv()];
        self.rate_limit_hits.add(1, &attributes);
        self.rate_limit_wait.record(wait.as_secs_f64(), &attributes);
    }
}

/// Gateway metrics: events, heartbeats, connection changes, and state.
pub struct GatewayMetrics {
    events: Histogram<f64>,
    heartbeat: Histogram<f64>,
    connection_events: Counter<u64>,
    connected: Source<bool>,
    guilds: Source<u64>,
    _connected_gauge: ObservableGauge<u64>,
    _guilds_gauge: ObservableGauge<u64>,
}

impl GatewayMetrics {
    pub(crate) fn new(meter: &Meter) -> Self {
        let connected: Source<bool> = Source::new();
        let source = connected.clone();
        let connected_gauge = u64_gauge(meter, &catalog::GATEWAY_CONNECTED, move |observer| {
            if let Some(connected) = source.read() {
                observer.observe(u64::from(connected), &[]);
            }
        });
        let guilds: Source<u64> = Source::new();
        let source = guilds.clone();
        let guilds_gauge = u64_gauge(meter, &catalog::GATEWAY_GUILDS, move |observer| {
            if let Some(count) = source.read() {
                observer.observe(count, &[]);
            }
        });
        Self {
            events: histogram(meter, &catalog::GATEWAY_EVENT_DURATION),
            heartbeat: histogram(meter, &catalog::GATEWAY_HEARTBEAT_LATENCY),
            connection_events: counter(meter, &catalog::GATEWAY_CONNECTION_EVENTS),
            connected,
            guilds,
            _connected_gauge: connected_gauge,
            _guilds_gauge: guilds_gauge,
        }
    }

    /// Records a handled gateway event and how long handling took.
    pub fn event(&self, event: GatewayEvent, duration: Duration) {
        self.events.record(duration.as_secs_f64(), &[event.kv()]);
    }

    /// Records the latency of one heartbeat acknowledgement.
    pub fn heartbeat(&self, latency: Duration) {
        self.heartbeat.record(latency.as_secs_f64(), &[]);
    }

    /// Counts a connection change.
    pub fn connection(&self, event: GatewayConnectionEvent) {
        self.connection_events.add(1, &[event.kv()]);
    }

    /// Installs the supplier of whether the session is connected.
    pub fn observe_connected(&self, supplier: impl Fn() -> bool + Send + Sync + 'static) {
        self.connected.set(supplier);
    }

    /// Installs the supplier of how many servers Quack is in.
    pub fn observe_guilds(&self, supplier: impl Fn() -> u64 + Send + Sync + 'static) {
        self.guilds.set(supplier);
    }
}
