//! The HTTP listener (§17.5 step 5): `/livez` at once, `/readyz` from the
//! readiness flags and a 1-second database check, and the API's routes
//! (an integration point, see `crate::wiring::api_router`).

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use quack_db::Db;
use serde_json::{Map, Value, json};
use tokio::net::TcpListener;

use super::Readiness;

/// What `/readyz` reads.
#[derive(Clone)]
struct Health {
    ready: Arc<Readiness>,
    db: Db,
}

/// `/livez` and `/readyz` (§14.9).
pub fn health(ready: Arc<Readiness>, db: Db) -> Router {
    Router::new()
        .route(
            "/livez",
            get(|| async { axum::Json(json!({"status": "ok"})) }),
        )
        .route("/readyz", get(readyz))
        .with_state(Health { ready, db })
}

async fn readyz(State(h): State<Health>) -> impl IntoResponse {
    let ping = sqlx::query("SELECT 1").execute(h.db.pool());
    let database = matches!(
        tokio::time::timeout(Duration::from_secs(1), ping).await,
        Ok(Ok(_))
    );
    let mut checks = Map::new();
    checks.insert(
        "database".to_owned(),
        Value::from(if database { "ok" } else { "failed" }),
    );
    for (name, ok) in h.ready.checks() {
        checks.insert(
            name.to_owned(),
            Value::from(if ok { "ok" } else { "failed" }),
        );
    }
    let ready = database && h.ready.is_ready();
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        axum::Json(json!({"status": if ready { "ok" } else { "unavailable" }, "checks": checks})),
    )
}

/// Serves `app` on `listener` until `shutdown`, then lets in-flight
/// requests finish.
pub async fn serve(
    listener: TcpListener,
    app: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
}
