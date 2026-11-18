//! The `tracing` subscriber stack.
//!
//! - stderr: every event the log filter allows, as text or JSON;
//! - traces: spans at `info` and above, sent to the tracer provider;
//! - logs: events the log filter allows, sent to the logger provider with
//!   the active trace and span ids (the tracing layer activates each span's
//!   OpenTelemetry context on entry, which is what correlates them).
//!
//! The two export layers never see events from the exporters' own stack
//! (`opentelemetry*`, `hyper`, `reqwest`, …), so an export failure cannot
//! feed back into the exporter. Those events still reach stderr.

use std::io::IsTerminal as _;

use opentelemetry::trace::TracerProvider as _;
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::trace::SdkTracerProvider;
use tracing::Subscriber;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::{EnvFilter, Layer, Registry};

use crate::config::{LogFormat, TelemetryConfig};
use crate::error::TelemetryError;
use crate::metrics::scope;

/// Targets kept out of exported traces and logs.
const EXPORT_EXCLUSIONS: &str =
    "opentelemetry=off,hyper=off,hyper_util=off,h2=off,reqwest=off,tonic=off,tower=off";

/// Spans exported regardless of the log filter, so lowering log verbosity
/// never loses traces.
const TRACE_FILTER: &str = "info";

type BoxedLayer = Box<dyn Layer<Registry> + Send + Sync>;

/// The providers the export layers write to.
pub(crate) struct ExportTargets<'a> {
    pub(crate) traces: &'a SdkTracerProvider,
    pub(crate) logs: &'a SdkLoggerProvider,
}

/// Builds the subscriber; `export` is `None` when telemetry export is off.
pub(crate) fn build(
    config: &TelemetryConfig,
    export: Option<ExportTargets<'_>>,
) -> Result<impl Subscriber + Send + Sync + for<'a> LookupSpan<'a>, TelemetryError> {
    let mut layers: Vec<BoxedLayer> =
        vec![stderr_layer(config.log_format, filter(&config.log_filter)?)];
    if let Some(export) = export {
        let tracer = export
            .traces
            .tracer_with_scope(scope(&config.service_version));
        layers.push(
            tracing_opentelemetry::layer()
                .with_tracer(tracer)
                .with_filter(filter(&format!("{TRACE_FILTER},{EXPORT_EXCLUSIONS}"))?)
                .boxed(),
        );
        layers.push(
            OpenTelemetryTracingBridge::new(export.logs)
                .with_filter(filter(&format!(
                    "{},{EXPORT_EXCLUSIONS}",
                    config.log_filter
                ))?)
                .boxed(),
        );
    }
    Ok(Registry::default().with(layers))
}

fn stderr_layer(format: LogFormat, filter: EnvFilter) -> BoxedLayer {
    let layer = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);
    match format {
        LogFormat::Text => layer
            .with_ansi(std::io::stderr().is_terminal())
            .with_filter(filter)
            .boxed(),
        LogFormat::Json => layer
            .json()
            .flatten_event(true)
            .with_current_span(true)
            .with_span_list(false)
            .with_filter(filter)
            .boxed(),
    }
}

fn filter(directives: &str) -> Result<EnvFilter, TelemetryError> {
    EnvFilter::builder()
        .parse(directives)
        .map_err(|error| TelemetryError::InvalidLogFilter {
            filter: directives.to_owned(),
            message: error.to_string(),
        })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn rejects_a_bad_filter() {
        let mut config = TelemetryConfig::new("quack", "0.0.0");
        config.log_filter = "info,quack=loud".to_owned();
        assert!(matches!(
            build(&config, None),
            Err(TelemetryError::InvalidLogFilter { .. })
        ));
    }
}
