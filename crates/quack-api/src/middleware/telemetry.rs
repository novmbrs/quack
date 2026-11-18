//! The outermost layer: one `{method} {route}` server span per request
//! (§18.4), `http.server.request.duration` and `http.server.active_requests`
//! from the `quack-telemetry` catalog, and the `traceparent` response
//! header. Spans and metrics use the route template, never the raw path
//! or query, so the OAuth callback's code and state never reach telemetry.

use std::time::Instant;

use axum::extract::{MatchedPath, Request};
use axum::http::{HeaderValue, Method};
use axum::middleware::Next;
use axum::response::Response;
use opentelemetry::trace::TraceContextExt as _;
use quack_telemetry::OpenTelemetrySpanExt as _;
use quack_telemetry::labels::{HttpMethod, HttpRoute};
use quack_telemetry::{metrics, spans};
use tracing::Instrument as _;

use crate::routes::route_template;

/// The semconv method label.
fn method_label(method: &Method) -> HttpMethod {
    match *method {
        Method::GET => HttpMethod::Get,
        Method::HEAD => HttpMethod::Head,
        Method::POST => HttpMethod::Post,
        Method::PUT => HttpMethod::Put,
        Method::PATCH => HttpMethod::Patch,
        Method::DELETE => HttpMethod::Delete,
        Method::OPTIONS => HttpMethod::Options,
        _ => HttpMethod::Other,
    }
}

/// The route label: the matched template, or a fixed name for requests
/// no route matched.
fn route_label(request: &Request) -> HttpRoute {
    let matched = request
        .extensions()
        .get::<MatchedPath>()
        .map(MatchedPath::as_str);
    HttpRoute::new(route_template(matched, request.uri().path()))
}

/// The 32-hex trace ID of the current span, when it is being traced.
pub(crate) fn current_trace_id() -> Option<String> {
    let context = tracing::Span::current().context();
    let span = context.span();
    let sc = span.span_context();
    sc.is_valid().then(|| sc.trace_id().to_string())
}

fn traceparent(span: &tracing::Span) -> Option<HeaderValue> {
    let context = span.context();
    let otel = context.span();
    let sc = otel.span_context();
    if !sc.is_valid() {
        return None;
    }
    HeaderValue::from_str(&format!(
        "00-{}-{}-{:02x}",
        sc.trace_id(),
        sc.span_id(),
        sc.trace_flags().to_u8()
    ))
    .ok()
}

/// Traces and measures one request.
pub(crate) async fn layer(request: Request, next: Next) -> Response {
    let method = method_label(request.method());
    let route = route_label(&request);
    let span = spans::http(method, route);
    let _in_flight = metrics().http.in_flight(method);
    let started = Instant::now();
    let mut response = next.run(request).instrument(span.clone()).await;
    let status = response.status().as_u16();
    spans::record_status(&span, status);
    if response.status().is_server_error() {
        spans::record_error(
            &span,
            response.status().canonical_reason().unwrap_or("error"),
        );
    }
    metrics()
        .http
        .request(method, route, status, started.elapsed());
    if let Some(value) = traceparent(&span) {
        response.headers_mut().insert("traceparent", value);
    }
    response
}
