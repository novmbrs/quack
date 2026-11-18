//! The middleware stack, outermost first (§14.7, §14.8, §18.4):
//!
//! 1. `telemetry`: the request's span (its trace ID is the request ID,
//!    returned as `traceparent` and in every error), `http.server.*`
//!    metrics by route template.
//! 2. panic catching: a panicking handler answers `500 internal_error`.
//! 3. `headers`: security headers; `no-store` and a deny-all CSP on API
//!    and health responses.
//! 4. gzip compression of text responses.
//! 5. the 10 s deadline for reads (`503 unavailable`).
//! 6. the 256 KiB body cap (`413 body_too_large`).
//! 7. `auth`: the session, resolved once; origin and CSRF checks on
//!    every API mutation.
//! 8. `rate_limit`: the route's class, keyed by account (and server) or
//!    client IP (`429 rate_limited` with `Retry-After`).

pub(crate) mod auth;
pub(crate) mod headers;
pub(crate) mod limits;
pub(crate) mod rate_limit;
pub(crate) mod telemetry;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::middleware::{from_fn, from_fn_with_state};
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::compression::CompressionLayer;

use crate::state::ApiState;

/// Wraps every route (and the fallback) in the stack above.
pub(crate) fn stack(router: Router<ApiState>, state: &ApiState) -> Router<ApiState> {
    router
        .layer(from_fn_with_state(state.clone(), rate_limit::layer))
        .layer(from_fn_with_state(state.clone(), auth::layer))
        .layer(DefaultBodyLimit::max(limits::MAX_BODY))
        .layer(from_fn(limits::body_limit))
        .layer(from_fn(|request, next| {
            limits::deadline(limits::DEADLINE, request, next)
        }))
        .layer(CompressionLayer::new().gzip(true))
        .layer(from_fn_with_state(state.clone(), headers::layer))
        .layer(CatchPanicLayer::custom(limits::panicked))
        .layer(from_fn(telemetry::layer))
}
