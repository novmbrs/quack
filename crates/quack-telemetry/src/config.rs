//! Telemetry settings, filled in by the binary from `QUACK_*` configuration.

use std::time::Duration;

/// How stderr log lines are formatted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogFormat {
    /// Human-readable lines, for development.
    #[default]
    Text,
    /// One JSON object per line, for production log collectors.
    Json,
}

/// Settings for [`init`](crate::init).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelemetryConfig {
    /// `service.name`; also names the tracer and meter scope's service.
    pub service_name: String,
    /// `service.version`, normally `env!("CARGO_PKG_VERSION")` of the binary.
    pub service_version: String,
    /// `service.instance.id`; `None` uses `<hostname>-<pid>`.
    pub service_instance_id: Option<String>,
    /// `deployment.environment.name`, such as `production` or `development`.
    pub environment: String,
    /// OTLP/HTTP base URL, such as `http://collector:4318`. `None` turns
    /// export off: only stderr logging runs.
    pub otlp_endpoint: Option<String>,
    /// `tracing` filter directives (`EnvFilter` syntax), such as `info` or
    /// `info,quack_db=debug`.
    pub log_filter: String,
    /// Stderr log format.
    pub log_format: LogFormat,
}

impl TelemetryConfig {
    /// A configuration with no export, `info` logging, and text output.
    pub fn new(service_name: impl Into<String>, service_version: impl Into<String>) -> Self {
        Self {
            service_name: service_name.into(),
            service_version: service_version.into(),
            service_instance_id: None,
            environment: "development".to_owned(),
            otlp_endpoint: None,
            log_filter: "info".to_owned(),
            log_format: LogFormat::Text,
        }
    }
}

/// Timeout for one OTLP export request.
pub(crate) const EXPORT_TIMEOUT: Duration = Duration::from_secs(5);
/// How often metrics are collected and exported.
pub(crate) const METRIC_INTERVAL: Duration = Duration::from_secs(30);
/// The default deadline for flushing all signals at shutdown.
pub const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(10);
