//! OpenTelemetry for Quack: traces, metrics, and logs over OTLP.
//!
//! - [`init`] starts everything from a [`TelemetryConfig`] and returns a
//!   [`TelemetryGuard`] that flushes all three signals at shutdown. Without
//!   an OTLP endpoint only stderr logging runs; nothing else changes for
//!   callers.
//! - [`metrics()`] is the typed instrument catalog. Record through its
//!   families (`metrics().jobs.finished(kind, outcome, duration)`); every
//!   name, unit, attribute, and bucket comes from [`catalog`], which also
//!   generates `METRICS.md`.
//! - [`spans`] builds the span for each unit of work, and
//!   [`current_trace_context`] / [`spans::job`] carry traces through the
//!   job queue as links.
//!
//! - [`errors`] buffers `error` events for the ops error feed.
//!
//! Metric attributes are typed [`labels`], so ids can never become metric
//! labels; ids go on spans and logs.

pub mod catalog;
mod config;
mod error;
pub mod errors;
mod guard;
mod init;
pub mod keys;
pub mod labels;
mod metrics;
mod propagation;
pub mod spans;
mod subscriber;

pub use config::{LogFormat, SHUTDOWN_DEADLINE, TelemetryConfig};
pub use error::TelemetryError;
pub use guard::TelemetryGuard;
pub use init::{Exporters, init, init_with_exporters};
pub use metrics::*;
pub use propagation::{current_trace_context, parse_trace_context, trace_context_of};
/// Extension methods on `tracing` spans for OpenTelemetry attributes,
/// links, and status.
pub use tracing_opentelemetry::OpenTelemetrySpanExt;
