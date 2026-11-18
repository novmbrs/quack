//! A collector that is down never crashes or blocks the application, even
//! when telemetry starts and stops on a Tokio runtime thread.

use std::time::{Duration, Instant};

use quack_telemetry::labels::{CommandName, InteractionKind, JobKind};
use quack_telemetry::{TelemetryConfig, init, metrics, spans};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unreachable_collector_does_not_block_or_crash() {
    let mut config = TelemetryConfig::new("quack-test", "0.0.0");
    // Port 9 (discard) refuses connections on any normal host.
    config.otlp_endpoint = Some("http://127.0.0.1:9/".to_owned());
    let guard = init(&config).unwrap();
    assert!(guard.is_exporting());

    let started = Instant::now();
    for _ in 0..1_000 {
        let span = spans::interaction(
            InteractionKind::SlashCommand,
            CommandName::new("case view"),
            Some(1),
            2,
        );
        span.in_scope(|| tracing::info!("handled"));
        metrics().jobs.enqueued(JobKind::CaseDm);
    }
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "recording blocked on export: {:?}",
        started.elapsed()
    );

    let deadline = Duration::from_secs(5);
    let started = Instant::now();
    // Export fails, so shutdown may report errors, but it must return in time.
    let _ = guard.with_deadline(deadline).shutdown();
    assert!(started.elapsed() < deadline + Duration::from_secs(1));
}
