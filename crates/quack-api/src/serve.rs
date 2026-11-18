//! Serving the application with graceful shutdown.

use std::future::Future;
use std::net::SocketAddr;

use axum::Router;
use tokio::net::TcpListener;

/// Serves `app` on `listener` until `shutdown` completes, then stops
/// accepting connections and lets in-flight requests finish. The socket
/// peer is recorded for the `sign_in` rate limit's client IP.
pub async fn serve(
    listener: TcpListener,
    app: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown)
    .await
}
