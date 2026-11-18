//! Telemetry captured in memory for one test process: install it with
//! [`capture`] before anything records a metric, run the system, then
//! [`Captured::flush`] and read what was exported.

#![allow(
    dead_code,
    unreachable_pub,
    clippy::cast_precision_loss,
    reason = "each test binary reads part of it; values are small counts"
)]

use std::collections::BTreeMap;
use std::sync::OnceLock;

use opentelemetry::KeyValue;
use opentelemetry_sdk::logs::InMemoryLogExporter;
use opentelemetry_sdk::metrics::InMemoryMetricExporter;
use opentelemetry_sdk::metrics::data::{
    AggregatedMetrics, MetricData, ResourceMetrics, ScopeMetrics,
};
use opentelemetry_sdk::trace::{InMemorySpanExporter, SpanData};
use quack_telemetry::{Exporters, TelemetryConfig, TelemetryGuard, init_with_exporters};

/// Instruments only `serve`'s startup feeds (the Tokio runtime and the
/// gauge task); `tests/serve.rs` checks them.
pub const SERVE_STARTUP: &[&str] = &[
    "quack.runtime.workers",
    "quack.runtime.tasks",
    "quack.runtime.global_queue.depth",
    "quack.runtime.worker.busy_time",
    "quack.runtime.worker.parks",
    "quack.actions.failed",
    "quack.appeals.open",
    "quack.sessions.active",
    "quack.jobs.ready",
    "quack.jobs.oldest_ready.age",
    "quack.jobs.dead",
];

/// One exported data point: its attributes and its value (a histogram's
/// count).
#[derive(Clone, Debug)]
pub struct Point {
    /// Each attribute's key and value, as text.
    pub attributes: Vec<(String, String)>,
    /// The value, or the number of observations of a histogram.
    pub value: f64,
}

/// The exporters every signal writes to.
pub struct Captured {
    spans: InMemorySpanExporter,
    metrics: InMemoryMetricExporter,
    guard: TelemetryGuard,
}

/// Installs in-memory export for the whole process, once. Logs are off;
/// spans still export at `info` (§18.5).
pub fn capture() -> &'static Captured {
    static CAPTURED: OnceLock<Captured> = OnceLock::new();
    CAPTURED.get_or_init(|| {
        let spans = InMemorySpanExporter::default();
        let metrics = InMemoryMetricExporter::default();
        let mut config = TelemetryConfig::new("quack-test", "0.0.0");
        "off".clone_into(&mut config.log_filter);
        let guard = init_with_exporters(
            &config,
            Exporters {
                spans: spans.clone(),
                metrics: metrics.clone(),
                logs: InMemoryLogExporter::default(),
            },
        )
        .expect("telemetry starts once per test process");
        Captured {
            spans,
            metrics,
            guard,
        }
    })
}

impl Captured {
    /// Exports everything recorded so far.
    pub fn flush(&self) {
        self.guard.force_flush().expect("telemetry flushes");
    }

    /// Every span exported so far.
    pub fn spans(&self) -> Vec<SpanData> {
        self.spans.get_finished_spans().unwrap_or_default()
    }

    /// Every data point exported so far, by instrument name.
    pub fn metrics(&self) -> BTreeMap<String, Vec<Point>> {
        let mut out: BTreeMap<String, Vec<Point>> = BTreeMap::new();
        let batches = self.metrics.get_finished_metrics().unwrap_or_default();
        for metric in batches
            .iter()
            .flat_map(ResourceMetrics::scope_metrics)
            .flat_map(ScopeMetrics::metrics)
        {
            let points = match metric.data() {
                AggregatedMetrics::F64(data) => points(data, |v| *v),
                AggregatedMetrics::U64(data) => points(data, |v| *v as f64),
                AggregatedMetrics::I64(data) => points(data, |v| *v as f64),
            };
            out.entry(metric.name().to_owned())
                .or_default()
                .extend(points);
        }
        out
    }
}

fn points<T: Copy>(data: &MetricData<T>, value: impl Fn(&T) -> f64) -> Vec<Point> {
    let attributes = |attrs: &mut dyn Iterator<Item = &KeyValue>| {
        attrs
            .map(|kv| (kv.key.to_string(), kv.value.to_string()))
            .collect()
    };
    match data {
        MetricData::Gauge(g) => g
            .data_points()
            .map(|p| Point {
                attributes: attributes(&mut p.attributes()),
                value: value(&p.value()),
            })
            .collect(),
        MetricData::Sum(s) => s
            .data_points()
            .map(|p| Point {
                attributes: attributes(&mut p.attributes()),
                value: value(&p.value()),
            })
            .collect(),
        MetricData::Histogram(h) => h
            .data_points()
            .map(|p| Point {
                attributes: attributes(&mut p.attributes()),
                value: p.count() as f64,
            })
            .collect(),
        MetricData::ExponentialHistogram(_) => Vec::new(),
    }
}
