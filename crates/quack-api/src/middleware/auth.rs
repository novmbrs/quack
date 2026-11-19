//! Resolves the request's session once (shared by every later layer and
//! extractor), and guards every state-changing API request with the
//! origin and CSRF checks of §14.5.

use axum::extract::{Request, State};
use axum::http::{HeaderMap, Method};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use quack_text::api::ApiCode;

use crate::auth::crypto;
use crate::auth::session::{self, SessionLookup};
use crate::error::ApiError;
use crate::state::ApiState;

/// The header carrying the session's CSRF token.
pub(crate) const CSRF_HEADER: &str = "x-csrf-token";

fn is_mutation(method: &Method) -> bool {
    matches!(
        *method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    )
}

/// Rule 1: `Origin` equals the public origin; without `Origin`,
/// `Sec-Fetch-Site` must be `same-origin`.
fn origin_ok(headers: &HeaderMap, origin: &str) -> bool {
    match headers.get("origin") {
        Some(value) => value.to_str().is_ok_and(|v| v == origin),
        None => headers
            .get("sec-fetch-site")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.eq_ignore_ascii_case("same-origin")),
    }
}

/// Rule 2: `X-CSRF-Token` equals the session's token (constant time).
fn csrf_ok(headers: &HeaderMap, lookup: &SessionLookup) -> Result<(), ApiError> {
    let session = lookup.require()?;
    let sent = headers
        .get(CSRF_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    if crypto::same(sent, &session.csrf_token) {
        Ok(())
    } else {
        Err(ApiError::api(ApiCode::CsrfRejected))
    }
}

/// Looks the session up for `/api` requests and checks mutations.
pub(crate) async fn layer(
    State(state): State<ApiState>,
    mut request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    if !(path == "/api" || path.starts_with("/api/")) {
        return next.run(request).await;
    }
    let lookup = match session::lookup(&state, request.headers()).await {
        Ok(lookup) => lookup,
        Err(error) => return error.into_response(),
    };
    if is_mutation(request.method()) {
        if !origin_ok(request.headers(), state.config().public_url.origin()) {
            return ApiError::api(ApiCode::OriginRejected).into_response();
        }
        if let Err(error) = csrf_ok(request.headers(), &lookup) {
            return error.into_response();
        }
    }
    request.extensions_mut().insert(lookup);
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn origin_rules() {
        let origin = "https://quack.example";
        let mut h = HeaderMap::new();
        assert!(!origin_ok(&h, origin));
        h.insert("sec-fetch-site", HeaderValue::from_static("same-origin"));
        assert!(origin_ok(&h, origin));
        h.insert("origin", HeaderValue::from_static("https://evil.example"));
        assert!(!origin_ok(&h, origin));
        h.insert("origin", HeaderValue::from_static("https://quack.example"));
        assert!(origin_ok(&h, origin));
        h.insert("origin", HeaderValue::from_static("null"));
        assert!(!origin_ok(&h, origin));
    }
}
