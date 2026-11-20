//! The `tracing` subscriber stack.
//!
//! - stderr: every event the log filter allows, as text or JSON;
//! - traces: spans at `info` and above, sent to the tracer provider;
//! - the error feed's tap: every `error` event (§21.7);
//! - logs: events the log filter allows, sent to the logger provider with
//!   the active trace and span ids (the tracing layer activates each span's
//!   OpenTelemetry context on entry, which is what correlates them).
//!
//! The two export layers never see events from the exporters' own stack
//! (`opentelemetry*`, `hyper`, `reqwest`, …), so an export failure cannot
//! feed back into the exporter. Those events still reach stderr.
//!
//! Each layer has its own filter, so the log filter decides only what is
//! logged. Those filters never let a callsite's interest be cached as
//! "always" (see [`LayerFilter`]).

use std::io::IsTerminal as _;

use opentelemetry::trace::TracerProvider as _;
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::trace::SdkTracerProvider;
use tracing::subscriber::Interest;
use tracing::{Metadata, Subscriber, span};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::{Context, Filter, SubscriberExt as _};
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
    let mut layers: Vec<BoxedLayer> = vec![
        stderr_layer(config.log_format, filter(&config.log_filter)?),
        crate::errors::ErrorTap
            .with_filter(LevelFilter::ERROR)
            .boxed(),
    ];
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

fn stderr_layer(format: LogFormat, filter: LayerFilter) -> BoxedLayer {
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

fn filter(directives: &str) -> Result<LayerFilter, TelemetryError> {
    EnvFilter::builder()
        .parse(directives)
        .map(LayerFilter)
        .map_err(|error| TelemetryError::InvalidLogFilter {
            filter: directives.to_owned(),
            message: error.to_string(),
        })
}

/// One layer's [`EnvFilter`], asked again at every span and event it might
/// enable.
///
/// `tracing-subscriber` keeps each layer's verdict in a thread-local until
/// the span or event is dispatched. A bare `tracing::enabled!` probe of a
/// callsite only some layers enable never dispatches, so those verdicts
/// linger, and the next callsite cached as "always" skips filtering and
/// inherits them: with `quack_engine=debug`, HTTP spans and shutdown logs
/// went missing. Reporting "sometimes" instead of "always" makes every
/// dispatch filter again and overwrite them.
struct LayerFilter(EnvFilter);

impl<S> Filter<S> for LayerFilter {
    fn enabled(&self, meta: &Metadata<'_>, cx: &Context<'_, S>) -> bool {
        Filter::enabled(&self.0, meta, cx)
    }

    fn callsite_enabled(&self, meta: &'static Metadata<'static>) -> Interest {
        let interest = Filter::<S>::callsite_enabled(&self.0, meta);
        if interest.is_always() {
            Interest::sometimes()
        } else {
            interest
        }
    }

    fn max_level_hint(&self) -> Option<LevelFilter> {
        Filter::<S>::max_level_hint(&self.0)
    }

    fn on_new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, cx: Context<'_, S>) {
        Filter::on_new_span(&self.0, attrs, id, cx);
    }

    fn on_record(&self, id: &span::Id, values: &span::Record<'_>, cx: Context<'_, S>) {
        Filter::on_record(&self.0, id, values, cx);
    }

    fn on_enter(&self, id: &span::Id, cx: Context<'_, S>) {
        Filter::on_enter(&self.0, id, cx);
    }

    fn on_exit(&self, id: &span::Id, cx: Context<'_, S>) {
        Filter::on_exit(&self.0, id, cx);
    }

    fn on_close(&self, id: span::Id, cx: Context<'_, S>) {
        Filter::on_close(&self.0, id, cx);
    }
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
