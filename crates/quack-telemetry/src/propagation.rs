//! Carrying trace context through the job queue.
//!
//! A job row stores the W3C `traceparent` of the span that enqueued it. The
//! job's own span starts a new trace and links back to that one, so a slow
//! retry an hour later doesn't stretch the original trace.

use std::collections::HashMap;

use opentelemetry::propagation::TextMapPropagator as _;
use opentelemetry::trace::{SpanContext, TraceContextExt as _};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt as _;

const TRACEPARENT: &str = "traceparent";

/// The current span's W3C `traceparent`, for storing with a job. `None` when
/// telemetry export is off or there is no sampled span.
pub fn current_trace_context() -> Option<String> {
    trace_context_of(&Span::current())
}

/// `span`'s W3C `traceparent`, or `None` if it has no OpenTelemetry context.
pub fn trace_context_of(span: &Span) -> Option<String> {
    let context = span.context();
    if !context.span().span_context().is_valid() {
        return None;
    }
    let mut carrier = HashMap::new();
    TraceContextPropagator::new().inject_context(&context, &mut carrier);
    carrier.remove(TRACEPARENT)
}

/// Parses a stored `traceparent`; `None` if it is malformed.
pub fn parse_trace_context(traceparent: &str) -> Option<SpanContext> {
    let carrier = HashMap::from([(TRACEPARENT.to_owned(), traceparent.to_owned())]);
    let context = TraceContextPropagator::new().extract(&carrier);
    let span_context = context.span().span_context().clone();
    span_context.is_valid().then_some(span_context)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_traceparent() {
        let value = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        let context = parse_trace_context(value).unwrap();
        assert_eq!(
            context.trace_id().to_string(),
            "4bf92f3577b34da6a3ce929d0e0e4736"
        );
        assert!(context.is_sampled());
        assert!(context.is_remote());
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_trace_context("not a traceparent").is_none());
        assert!(
            parse_trace_context("00-00000000000000000000000000000000-00f067aa0ba902b7-01")
                .is_none()
        );
    }

    #[test]
    fn no_context_without_a_layer() {
        let span = tracing::info_span!("plain");
        assert!(trace_context_of(&span).is_none());
    }
}
