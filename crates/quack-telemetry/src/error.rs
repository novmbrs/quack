//! Telemetry errors.

use std::time::Duration;

/// Why telemetry could not start or stop cleanly.
#[derive(Debug, thiserror::Error)]
pub enum TelemetryError {
    /// The log filter directives did not parse.
    #[error("the log filter {filter:?} is invalid: {message}")]
    InvalidLogFilter {
        /// The filter as configured.
        filter: String,
        /// What the parser said.
        message: String,
    },
    /// An OTLP exporter could not be built (for example, a bad endpoint).
    #[error("couldn't set up the OTLP {signal} exporter: {message}")]
    Exporter {
        /// `traces`, `metrics`, or `logs`.
        signal: &'static str,
        /// What the exporter builder said.
        message: String,
    },
    /// Another global `tracing` subscriber is already installed.
    #[error("a tracing subscriber is already installed: {0}")]
    SubscriberAlreadySet(String),
    /// One or more signals failed to flush at shutdown.
    #[error("telemetry didn't shut down cleanly: {}", .0.join("; "))]
    Shutdown(Vec<String>),
    /// Shutdown did not finish before the deadline.
    #[error("telemetry didn't finish flushing within {0:?}")]
    ShutdownTimedOut(Duration),
}
