//! The telemetry guard: owns the providers and flushes them at shutdown.

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::trace::SdkTracerProvider;

use crate::config::SHUTDOWN_DEADLINE;
use crate::error::TelemetryError;

/// The three SDK providers behind an exporting setup.
pub(crate) struct Providers {
    pub(crate) traces: SdkTracerProvider,
    pub(crate) meters: SdkMeterProvider,
    pub(crate) logs: SdkLoggerProvider,
}

impl Providers {
    fn flush(&self) -> Vec<String> {
        let mut failures = Vec::new();
        if let Err(error) = self.traces.force_flush() {
            failures.push(format!("traces: {error}"));
        }
        if let Err(error) = self.logs.force_flush() {
            failures.push(format!("logs: {error}"));
        }
        if let Err(error) = self.meters.force_flush() {
            failures.push(format!("metrics: {error}"));
        }
        failures
    }

    fn shutdown(&self, timeout: Duration) -> Vec<String> {
        let mut failures = Vec::new();
        if let Err(error) = self.traces.shutdown_with_timeout(timeout) {
            failures.push(format!("traces: {error}"));
        }
        if let Err(error) = self.logs.shutdown_with_timeout(timeout) {
            failures.push(format!("logs: {error}"));
        }
        if let Err(error) = self.meters.shutdown_with_timeout(timeout) {
            failures.push(format!("metrics: {error}"));
        }
        failures
    }
}

/// Keeps telemetry running. Call [`TelemetryGuard::shutdown`] after the
/// application has drained so buffered spans, logs, and metrics are
/// exported; dropping the guard does the same with the default deadline.
///
/// Flushing runs on its own thread, so it is safe to call from inside or
/// outside a Tokio runtime, and it never blocks longer than the deadline.
#[must_use = "dropping the guard shuts telemetry down"]
pub struct TelemetryGuard {
    providers: Option<Providers>,
    deadline: Duration,
}

impl TelemetryGuard {
    pub(crate) fn new(providers: Option<Providers>) -> Self {
        Self {
            providers,
            deadline: SHUTDOWN_DEADLINE,
        }
    }

    pub(crate) fn providers(&self) -> Option<&Providers> {
        self.providers.as_ref()
    }

    /// Whether spans, metrics, and logs are being exported over OTLP.
    pub fn is_exporting(&self) -> bool {
        self.providers.is_some()
    }

    /// Sets how long [`Self::shutdown`] may take in total.
    pub fn with_deadline(mut self, deadline: Duration) -> Self {
        self.deadline = deadline;
        self
    }

    /// Exports everything buffered so far without shutting down.
    pub fn force_flush(&self) -> Result<(), TelemetryError> {
        let Some(providers) = &self.providers else {
            return Ok(());
        };
        let failures = thread::scope(|scope| {
            scope
                .spawn(|| providers.flush())
                .join()
                .unwrap_or_else(|_| vec!["flushing panicked".to_owned()])
        });
        if failures.is_empty() {
            Ok(())
        } else {
            Err(TelemetryError::Shutdown(failures))
        }
    }

    /// Flushes and stops all three signals within the deadline. Telemetry
    /// recorded afterwards is dropped.
    pub fn shutdown(mut self) -> Result<(), TelemetryError> {
        self.shutdown_inner()
    }

    fn shutdown_inner(&mut self) -> Result<(), TelemetryError> {
        let Some(providers) = self.providers.take() else {
            return Ok(());
        };
        let deadline = self.deadline;
        let (done, wait) = mpsc::channel();
        let spawned = thread::Builder::new()
            .name("quack-telemetry-shutdown".to_owned())
            .spawn(move || {
                // The receiver may be gone after a timeout; nothing to report then.
                let _ = done.send(providers.shutdown(deadline));
            });
        if let Err(error) = spawned {
            return Err(TelemetryError::Shutdown(vec![format!(
                "couldn't start the shutdown thread: {error}"
            )]));
        }
        match wait.recv_timeout(deadline) {
            Ok(failures) if failures.is_empty() => Ok(()),
            Ok(failures) => Err(TelemetryError::Shutdown(failures)),
            Err(_) => Err(TelemetryError::ShutdownTimedOut(deadline)),
        }
    }
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        if let Err(error) = self.shutdown_inner() {
            tracing::warn!(%error, "telemetry shutdown on drop failed");
        }
    }
}
