//! Starting telemetry: resource, providers, exporters, and the subscriber.

use std::thread;

use opentelemetry::{KeyValue, global};
use opentelemetry_otlp::{LogExporter, MetricExporter, Protocol, SpanExporter, WithExportConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::{LogExporter as LogExport, SdkLoggerProvider};
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider, exporter::PushMetricExporter};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::{Sampler, SdkTracerProvider, SpanExporter as SpanExport};
use opentelemetry_semantic_conventions::attribute::{
    DEPLOYMENT_ENVIRONMENT_NAME, SERVICE_INSTANCE_ID, SERVICE_VERSION,
};
use tracing_subscriber::util::SubscriberInitExt as _;

use crate::catalog;
use crate::config::{EXPORT_TIMEOUT, METRIC_INTERVAL, TelemetryConfig};
use crate::error::TelemetryError;
use crate::guard::{Providers, TelemetryGuard};
use crate::metrics;
use crate::subscriber::{self, ExportTargets};

/// The exporters for each signal, for [`init_with_exporters`].
pub struct Exporters<S, M, L> {
    /// Receives finished spans in batches.
    pub spans: S,
    /// Receives collected metrics periodically and at shutdown.
    pub metrics: M,
    /// Receives log records in batches.
    pub logs: L,
}

/// Starts telemetry: installs the global `tracing` subscriber, the W3C trace
/// context propagator, and, when `config.otlp_endpoint` is set, OTLP/HTTP
/// export of traces, metrics, and logs. Builds the global [`metrics()`]
/// catalog. Call once, early, before the Tokio runtime starts or from a
/// blocking context.
///
/// Export never blocks or fails the application: batches are bounded and
/// dropped when the collector is slow or down, and export errors are logged
/// to stderr.
pub fn init(config: &TelemetryConfig) -> Result<TelemetryGuard, TelemetryError> {
    match config.otlp_endpoint.as_deref() {
        Some(endpoint) => init_with_exporters(config, otlp_exporters(endpoint)?),
        None => init_without_export(config),
    }
}

/// Like [`init`], but exports through the given exporters (in-memory ones in
/// tests, or a different transport).
pub fn init_with_exporters<S, M, L>(
    config: &TelemetryConfig,
    exporters: Exporters<S, M, L>,
) -> Result<TelemetryGuard, TelemetryError>
where
    S: SpanExport + 'static,
    M: PushMetricExporter,
    L: LogExport + 'static,
{
    let resource = resource(config);
    let traces = SdkTracerProvider::builder()
        .with_resource(resource.clone())
        .with_sampler(Sampler::ParentBased(Box::new(Sampler::AlwaysOn)))
        .with_batch_exporter(exporters.spans)
        .build();
    let meters = SdkMeterProvider::builder()
        .with_resource(resource.clone())
        .with_reader(
            PeriodicReader::builder(exporters.metrics)
                .with_interval(METRIC_INTERVAL)
                .build(),
        )
        .with_view(catalog::view)
        .build();
    let logs = SdkLoggerProvider::builder()
        .with_resource(resource)
        .with_batch_exporter(exporters.logs)
        .build();
    let providers = Providers {
        traces,
        meters,
        logs,
    };

    let installed = subscriber::build(
        config,
        Some(ExportTargets {
            traces: &providers.traces,
            logs: &providers.logs,
        }),
    )
    .and_then(|subscriber| {
        subscriber
            .try_init()
            .map_err(|error| TelemetryError::SubscriberAlreadySet(error.to_string()))
    });
    let guard = TelemetryGuard::new(Some(providers));
    installed?;
    // `guard` owns the providers; the globals hold clones.
    install_globals(config, Some(&guard));
    Ok(guard)
}

fn init_without_export(config: &TelemetryConfig) -> Result<TelemetryGuard, TelemetryError> {
    subscriber::build(config, None)?
        .try_init()
        .map_err(|error| TelemetryError::SubscriberAlreadySet(error.to_string()))?;
    install_globals(config, None);
    Ok(TelemetryGuard::new(None))
}

fn install_globals(config: &TelemetryConfig, guard: Option<&TelemetryGuard>) {
    global::set_text_map_propagator(TraceContextPropagator::new());
    if let Some(providers) = guard.and_then(TelemetryGuard::providers) {
        global::set_tracer_provider(providers.traces.clone());
        global::set_meter_provider(providers.meters.clone());
    }
    if !metrics::install_global(&config.service_version) {
        tracing::warn!(
            "quack_telemetry::metrics() was used before init; its instruments are not exported"
        );
    }
    tracing::info!(
        exporting = guard.is_some_and(TelemetryGuard::is_exporting),
        "telemetry started"
    );
}

/// The resource shared by every signal.
fn resource(config: &TelemetryConfig) -> Resource {
    let instance = config
        .service_instance_id
        .clone()
        .unwrap_or_else(default_instance_id);
    Resource::builder()
        .with_service_name(config.service_name.clone())
        .with_attributes([
            KeyValue::new(SERVICE_VERSION, config.service_version.clone()),
            KeyValue::new(SERVICE_INSTANCE_ID, instance),
            KeyValue::new(DEPLOYMENT_ENVIRONMENT_NAME, config.environment.clone()),
        ])
        .build()
}

/// `<hostname>-<pid>`: unique per running process and readable in a backend.
fn default_instance_id() -> String {
    let host = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .or_else(|| std::env::var("HOSTNAME").ok())
        .map(|host| host.trim().to_owned())
        .filter(|host| !host.is_empty())
        .unwrap_or_else(|| "unknown".to_owned());
    format!("{host}-{}", std::process::id())
}

type OtlpExporters = Exporters<SpanExporter, MetricExporter, LogExporter>;

/// Builds the OTLP/HTTP (protobuf) exporters for `base`, such as
/// `http://collector:4318`.
///
/// The blocking HTTP client must not be created or dropped on a Tokio
/// runtime thread, so the exporters are built on a short-lived thread; on
/// failure the partial set is dropped there too.
fn otlp_exporters(base: &str) -> Result<OtlpExporters, TelemetryError> {
    let base = base.trim_end_matches('/');
    thread::scope(|scope| {
        scope
            .spawn(|| build_otlp_exporters(base))
            .join()
            .unwrap_or_else(|_| {
                Err(TelemetryError::Exporter {
                    signal: "all",
                    message: "building the exporters panicked".to_owned(),
                })
            })
    })
}

fn build_otlp_exporters(base: &str) -> Result<OtlpExporters, TelemetryError> {
    let failed = |signal: &'static str| {
        move |error: opentelemetry_otlp::ExporterBuildError| TelemetryError::Exporter {
            signal,
            message: error.to_string(),
        }
    };
    let spans = SpanExporter::builder()
        .with_http()
        .with_protocol(Protocol::HttpBinary)
        .with_endpoint(format!("{base}/v1/traces"))
        .with_timeout(EXPORT_TIMEOUT)
        .build()
        .map_err(failed("traces"))?;
    let metrics = MetricExporter::builder()
        .with_http()
        .with_protocol(Protocol::HttpBinary)
        .with_endpoint(format!("{base}/v1/metrics"))
        .with_timeout(EXPORT_TIMEOUT)
        .build()
        .map_err(failed("metrics"))?;
    let logs = LogExporter::builder()
        .with_http()
        .with_protocol(Protocol::HttpBinary)
        .with_endpoint(format!("{base}/v1/logs"))
        .with_timeout(EXPORT_TIMEOUT)
        .build()
        .map_err(failed("logs"))?;
    Ok(Exporters {
        spans,
        metrics,
        logs,
    })
}
