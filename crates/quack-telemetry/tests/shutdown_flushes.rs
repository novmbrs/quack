//! Shutdown exports everything still buffered in the batch processors and
//! the periodic metric reader, and exported logs carry trace ids.

use std::future::{Future, ready};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use opentelemetry::trace::{SpanId, TraceId};
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::logs::{LogBatch, LogExporter};
use opentelemetry_sdk::metrics::InMemoryMetricExporter;
use opentelemetry_sdk::metrics::data::{ResourceMetrics, ScopeMetrics};
use opentelemetry_sdk::trace::{SpanData, SpanExporter};
use quack_telemetry::labels::{ActionKind, CaseSource, CommandName, InteractionKind};
use quack_telemetry::{Exporters, TelemetryConfig, init_with_exporters, metrics, spans};

/// Keeps every exported span; unlike the SDK's in-memory exporter it does
/// not clear them on shutdown.
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

/// One exported log: its body and trace context.
type RecordedLog = (String, Option<(TraceId, SpanId)>);

/// Keeps each exported log's body and trace context.
#[derive(Debug, Clone, Default)]
struct LogRecorder(Arc<Mutex<Vec<RecordedLog>>>);

impl LogExporter for LogRecorder {
    fn export(&self, batch: LogBatch<'_>) -> impl Future<Output = OTelSdkResult> + Send {
        let mut logs = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        for (record, _scope) in batch.iter() {
            let body = record
                .body()
                .map_or_else(String::new, |body| format!("{body:?}"));
            let context = record.trace_context().map(|c| (c.trace_id, c.span_id));
            logs.push((body, context));
        }
        ready(Ok(()))
    }
}

#[test]
fn shutdown_flushes_traces_metrics_and_logs() {
    let spans_out = SpanRecorder::default();
    let logs_out = LogRecorder::default();
    let metrics_out = InMemoryMetricExporter::default();
    let mut config = TelemetryConfig::new("quack-test", "9.9.9");
    config.environment = "test".to_owned();
    let guard = init_with_exporters(
        &config,
        Exporters {
            spans: spans_out.clone(),
            metrics: metrics_out.clone(),
            logs: logs_out.clone(),
        },
    )
    .unwrap();
    assert!(guard.is_exporting());

    let span = spans::interaction(
        InteractionKind::MessageCommand,
        CommandName::new("Add case"),
        Some(1),
        2,
    );
    span.in_scope(|| {
        tracing::info!("case saved");
        metrics().cases.created(
            CaseSource::MessageCommand,
            Some(ActionKind::Kick),
            3,
            Duration::from_millis(12),
        );
    });
    drop(span);

    // Batches wait up to 5 s and metrics 30 s; only shutdown can have
    // exported them this quickly.
    guard
        .with_deadline(Duration::from_secs(5))
        .shutdown()
        .unwrap();

    let exported_spans = spans_out.0.lock().unwrap();
    let span = exported_spans
        .iter()
        .find(|s| s.name == "interaction message_command Add case")
        .unwrap();

    let logs = logs_out.0.lock().unwrap();
    let (_, context) = logs
        .iter()
        .find(|(body, _)| body.contains("case saved"))
        .unwrap();
    assert_eq!(
        *context,
        Some((span.span_context.trace_id(), span.span_context.span_id()))
    );

    let names: Vec<String> = metrics_out
        .get_finished_metrics()
        .unwrap()
        .iter()
        .flat_map(ResourceMetrics::scope_metrics)
        .flat_map(ScopeMetrics::metrics)
        .map(|metric| metric.name().to_owned())
        .collect();
    assert!(
        names.iter().any(|n| n == "quack.cases.created"),
        "{names:?}"
    );
    assert!(names.iter().any(|n| n == "quack.build.info"), "{names:?}");
}
