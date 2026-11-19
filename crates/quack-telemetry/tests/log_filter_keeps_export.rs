//! The log filter only changes what is logged: a debug directive for one
//! crate must not cost any exported span or log.

use std::future::{Future, ready};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::logs::{LogBatch, LogExporter};
use opentelemetry_sdk::metrics::InMemoryMetricExporter;
use opentelemetry_sdk::trace::{SpanData, SpanExporter};
use quack_telemetry::labels::{HttpMethod, HttpRoute};
use quack_telemetry::{Exporters, TelemetryConfig, init_with_exporters, spans};

/// Keeps every exported span.
#[derive(Debug, Clone, Default)]
struct SpanRecorder(Arc<Mutex<Vec<SpanData>>>);

impl SpanExporter for SpanRecorder {
    fn export(&self, batch: Vec<SpanData>) -> impl Future<Output = OTelSdkResult> + Send {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend(batch);
        ready(Ok(()))
    }
}

/// Keeps every exported log body.
#[derive(Debug, Clone, Default)]
struct LogRecorder(Arc<Mutex<Vec<String>>>);

impl LogExporter for LogRecorder {
    fn export(&self, batch: LogBatch<'_>) -> impl Future<Output = OTelSdkResult> + Send {
        let mut logs = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        for (record, _scope) in batch.iter() {
            logs.push(record.body().map_or_else(String::new, |b| format!("{b:?}")));
        }
        ready(Ok(()))
    }
}

#[test]
fn a_debug_log_filter_keeps_every_span_and_log() {
    let spans_out = SpanRecorder::default();
    let logs_out = LogRecorder::default();
    let mut config = TelemetryConfig::new("quack-test", "9.9.9");
    config.log_filter = "info,quack_engine=debug".to_owned();
    let guard = init_with_exporters(
        &config,
        Exporters {
            spans: spans_out.clone(),
            metrics: InMemoryMetricExporter::default(),
            logs: logs_out.clone(),
        },
    )
    .unwrap();

    for _ in 0..10 {
        // Logged at debug, but too verbose for traces: only some layers
        // enable it, and a bare probe never dispatches anything.
        let _ = tracing::enabled!(target: "quack_engine::cases", tracing::Level::DEBUG);
        let span = spans::http(HttpMethod::Get, HttpRoute::new("/livez"));
        span.in_scope(|| tracing::debug!(target: "quack_engine::cases", "case drafted"));
        drop(span);
        tracing::info!(target: "quack::app", "shutting down");
    }
    guard
        .with_deadline(Duration::from_secs(5))
        .shutdown()
        .unwrap();

    let spans = spans_out.0.lock().unwrap();
    let http = spans.iter().filter(|s| s.name == "GET /livez").count();
    assert_eq!(http, 10, "exported spans: {}", spans.len());
    let logs = logs_out.0.lock().unwrap();
    let count = |text: &str| logs.iter().filter(|b| b.contains(text)).count();
    assert_eq!(count("shutting down"), 10, "{logs:?}");
    assert_eq!(count("case drafted"), 10, "{logs:?}");
}
