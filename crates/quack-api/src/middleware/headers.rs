//! Security headers on every response (§14.8), and `no-store` plus a
//! deny-all CSP on API and health responses.

use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::middleware::Next;
use axum::response::Response;

use crate::state::ApiState;

const API_CSP: &str = "default-src 'none'; frame-ancestors 'none'";

/// Whether a path is the API or a health probe (not dashboard files).
pub(crate) fn is_api_or_health(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/") || path == "/livez" || path == "/readyz"
}

fn set(headers: &mut HeaderMap, name: header::HeaderName, value: &'static str) {
    headers.insert(name, HeaderValue::from_static(value));
}

/// Adds the headers after the handler answers.
pub(crate) async fn layer(State(state): State<ApiState>, request: Request, next: Next) -> Response {
    let api = is_api_or_health(request.uri().path());
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    set(headers, header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    set(headers, header::REFERRER_POLICY, "no-referrer");
    set(headers, header::X_FRAME_OPTIONS, "DENY");
    headers.insert(
        "permissions-policy",
        HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );
    if state.config().public_url.is_https() {
        set(
            headers,
            header::STRICT_TRANSPORT_SECURITY,
            "max-age=31536000",
        );
    }
    if api {
        set(headers, header::CACHE_CONTROL, "no-store");
        set(headers, header::CONTENT_SECURITY_POLICY, API_CSP);
    }
    response
}
