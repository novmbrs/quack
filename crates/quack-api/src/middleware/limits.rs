//! Request limits (§14.8): the 256 KiB body cap (the upload route's
//! larger one), the 10 s deadline, and
//! turning a panic into the error envelope. Each answers with the §14.6
//! envelope, never a bare status.

use std::any::Any;
use std::time::Duration;

use axum::extract::{MatchedPath, Request};
use axum::http::{Method, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use quack_text::api::ApiCode;

use crate::error::ApiError;

/// The largest request body.
pub(crate) const MAX_BODY: usize = quack_core::limits::MAX_BODY_BYTES;

/// The request deadline.
pub(crate) const DEADLINE: Duration = Duration::from_secs(10);

/// The largest upload request body (route 60).
pub(crate) const MAX_UPLOAD_BODY: usize = quack_core::limits::MAX_UPLOAD_BODY_BYTES;

/// The body cap of a request: the upload route's, else [`MAX_BODY`].
fn cap(request: &Request) -> usize {
    let upload = request.method() == Method::POST
        && request
            .extensions()
            .get::<MatchedPath>()
            .is_some_and(|m| m.as_str() == crate::routes::UPLOAD_ROUTE);
    if upload { MAX_UPLOAD_BODY } else { MAX_BODY }
}

/// Refuses a declared body over the request's cap before reading it;
/// bodies without a length are capped by `DefaultBodyLimit` when an
/// extractor reads them, or by the upload route while it reads, which
/// answer the same way.
pub(crate) async fn body_limit(request: Request, next: Next) -> Response {
    let max = cap(&request) as u64;
    let declared = request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if declared.is_some_and(|n| n > max) {
        return ApiError::api(ApiCode::BodyTooLarge).into_response();
    }
    next.run(request).await
}

/// Whether a method may write. Writes are never cut off by the deadline:
/// once their transaction may have begun, the request finishes (statement
/// and lock timeouts bound it), so "nothing changed" is never said about
/// a write that committed (§14.8).
fn is_write(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// Ends a read after `deadline` with `503 unavailable`.
pub(crate) async fn deadline(deadline: Duration, request: Request, next: Next) -> Response {
    if is_write(request.method()) {
        return next.run(request).await;
    }
    match tokio::time::timeout(deadline, next.run(request)).await {
        Ok(response) => response,
        Err(_) => ApiError::api(ApiCode::Unavailable).into_response(),
    }
}

/// The response for a handler that panicked: `500 internal_error`. The
/// panic's message is not logged (it could hold request data).
pub(crate) fn panicked(_: Box<dyn Any + Send + 'static>) -> Response {
    tracing::error!("a request handler panicked");
    ApiError::internal().into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::routing::get;
    use tower::ServiceExt as _;

    #[tokio::test]
    async fn slow_reads_end_with_the_envelope() {
        let app = Router::new()
            .route(
                "/slow",
                get(|| async {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    "late"
                }),
            )
            .layer(axum::middleware::from_fn(|r, n| {
                deadline(Duration::from_millis(20), r, n)
            }));
        let response = app
            .oneshot(
                Request::get("/slow")
                    .body(Body::empty())
                    .unwrap_or_default(),
            )
            .await;
        let response = response.unwrap_or_else(|e| match e {});
        assert_eq!(response.status(), 503);
        let body = axum::body::to_bytes(response.into_body(), 1 << 16).await;
        let json: serde_json::Value =
            serde_json::from_slice(&body.unwrap_or_default()).unwrap_or_default();
        assert_eq!(json["error"]["code"], "unavailable");
    }
}
