//! With no OTLP endpoint, telemetry is stderr logging only and everything
//! else is a harmless no-op.

use std::time::{Duration, Instant};

use opentelemetry::global;
use opentelemetry::trace::{Span as _, Tracer as _};
use quack_telemetry::labels::{CommandName, InteractionKind, JobKind, JobOutcome};
use quack_telemetry::{TelemetryConfig, current_trace_context, init, metrics, spans};

#[test]
fn init_without_an_endpoint_exports_nothing_and_never_fails() {
    let guard = init(&TelemetryConfig::new("quack-test", "0.0.0")).unwrap();
    assert!(!guard.is_exporting());

    // Spans work but carry no OpenTelemetry context.
    let span = spans::interaction(
        InteractionKind::SlashCommand,
        CommandName::new("help"),
        None,
        1,
        2,
    );
    span.in_scope(|| {
        tracing::info!("logged to stderr only");
        assert!(current_trace_context().is_none());
    });

    // The global tracer is the no-op one.
    let mut otel_span = global::tracer("test").start("noop");
    assert!(!otel_span.span_context().is_valid());
    otel_span.end();

    // Metrics record into the no-op provider.
    metrics().jobs.finished(
        JobKind::Housekeeping,
        JobOutcome::Done,
        Duration::from_millis(3),
    );

    // A second init is refused instead of replacing the subscriber.
    assert!(init(&TelemetryConfig::new("quack-test", "0.0.0")).is_err());

    let started = Instant::now();
    guard.shutdown().unwrap();
    assert!(started.elapsed() < Duration::from_secs(1));
}
