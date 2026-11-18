//! The metric instrument catalog: one static table that drives everything.
//!
//! Every instrument Quack exports is declared once here with its name, type,
//! unit, allowed attributes, and histogram buckets. [`Metrics`](crate::Metrics)
//! builds its instruments from these specs, [`view`] enforces the attribute
//! allow-list and buckets in the SDK, and [`render_markdown`] produces
//! `METRICS.md`, which a test keeps in sync.

/// Declares one catalog entry as a `pub(crate)` constant.
macro_rules! spec {
    (
        $id:ident: $kind:ident $name:literal, unit $unit:literal,
        [$($attr:expr),* $(,)?], $desc:literal $(, buckets $buckets:expr)?
    ) => {
        pub(crate) const $id: super::InstrumentSpec = super::InstrumentSpec {
            name: $name,
            kind: super::InstrumentKind::$kind,
            unit: $unit,
            description: $desc,
            attributes: &[$($attr),*],
            buckets: spec!(@buckets $($buckets)?),
        };
    };
    (@buckets) => { &[] };
    (@buckets $buckets:expr) => { $buckets };
}

mod attributes;
mod buckets;
mod moderation;
mod platform;

use std::fmt::Write as _;

use opentelemetry_sdk::metrics::{Aggregation, Instrument, Stream};

pub use attributes::ATTRIBUTES;
pub(crate) use moderation::*;
pub(crate) use platform::*;

/// The OpenTelemetry instrument type of a catalog entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstrumentKind {
    /// A monotonic sum recorded as it happens.
    Counter,
    /// A sum that goes up and down, recorded as it happens.
    UpDownCounter,
    /// A distribution with explicit buckets.
    Histogram,
    /// A value read by a callback at each collection.
    Gauge,
    /// A monotonic sum read by a callback at each collection.
    ObservableCounter,
    /// A sum that goes up and down, read by a callback at each collection.
    ObservableUpDownCounter,
}

impl InstrumentKind {
    /// The name used in `METRICS.md`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Counter => "counter",
            Self::UpDownCounter => "updowncounter",
            Self::Histogram => "histogram",
            Self::Gauge => "gauge (observed)",
            Self::ObservableCounter => "counter (observed)",
            Self::ObservableUpDownCounter => "updowncounter (observed)",
        }
    }
}

/// One instrument in the catalog.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InstrumentSpec {
    /// The exported name: `quack.*`, or a semantic-convention name.
    pub name: &'static str,
    /// The instrument type.
    pub kind: InstrumentKind,
    /// The UCUM unit, such as `s`, `By`, or `{case}`.
    pub unit: &'static str,
    /// What the instrument measures.
    pub description: &'static str,
    /// The only attribute keys the SDK keeps for this instrument.
    pub attributes: &'static [&'static str],
    /// Explicit bucket boundaries; empty unless this is a histogram.
    pub buckets: &'static [f64],
}

/// One attribute key that catalog instruments may carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttributeSpec {
    /// The attribute key.
    pub key: &'static str,
    /// What the attribute means.
    pub description: &'static str,
    /// Every allowed value, or empty when values are static names chosen in
    /// code (commands, routes, tables).
    pub values: &'static [&'static str],
}

/// Every instrument, grouped by family in the order `METRICS.md` lists them.
pub static INSTRUMENTS: &[&InstrumentSpec] = &[
    // Interactions.
    &INTERACTION_ACK_DURATION,
    &INTERACTION_DURATION,
    &INTERACTION_ERRORS,
    // Cases.
    &CASES_CREATED,
    &CASES_VOIDED,
    &CASE_CREATE_DURATION,
    &CASE_LEVEL,
    // Actions.
    &ACTION_ATTEMPTS,
    &ACTION_COMPLETION_DURATION,
    &ACTION_RETRIES,
    &ACTIONS_FAILED,
    &ACTION_UNDO_SKIPPED,
    // DMs.
    &DM_OUTCOMES,
    &DM_SEND_DURATION,
    // Appeals.
    &APPEAL_TRANSITIONS,
    &APPEAL_DECISION_DURATION,
    &APPEALS_OPEN,
    // Evidence.
    &EVIDENCE_CAPTURES,
    &EVIDENCE_FILES,
    &EVIDENCE_BYTES,
    // Moderation log.
    &MODLOG_ENTRIES,
    &MODLOG_POSTS,
    // Tracked messages.
    &MESSAGE_SYNCS,
    &MESSAGE_SYNC_LAG,
    // Jobs.
    &JOBS_ENQUEUED,
    &JOBS_STARTED,
    &JOB_DURATION,
    &JOB_QUEUE_WAIT,
    &JOBS_READY,
    &JOBS_OLDEST_READY_AGE,
    // Discord REST.
    &HTTP_CLIENT_REQUEST_DURATION,
    &DISCORD_RATE_LIMIT_HITS,
    &DISCORD_RATE_LIMIT_WAIT,
    // Gateway.
    &GATEWAY_EVENT_DURATION,
    &GATEWAY_HEARTBEAT_LATENCY,
    &GATEWAY_CONNECTION_EVENTS,
    &GATEWAY_CONNECTED,
    &GATEWAY_GUILDS,
    // HTTP API.
    &HTTP_SERVER_REQUEST_DURATION,
    &HTTP_SERVER_ACTIVE_REQUESTS,
    &SIGN_INS,
    &HTTP_RATE_LIMITED,
    &SESSIONS_ACTIVE,
    // Database.
    &DB_OPERATION_DURATION,
    &DB_CONNECTION_COUNT,
    &DB_CONNECTION_MAX,
    &DB_CONNECTION_WAIT_TIME,
    &DB_CONNECTION_TIMEOUTS,
    &DB_TRANSACTION_DURATION,
    &DB_TRANSACTION_RETRIES,
    // Caches and queues.
    &CACHE_SIZE,
    &CACHE_LOOKUPS,
    &CACHE_EVICTIONS,
    &QUEUE_DEPTH,
    &QUEUE_DROPPED,
    // Modules.
    &TICKETS_OPENED,
    &TICKETS_CLOSED,
    &TICKETS_OPEN,
    &TICKET_TRANSCRIPT_SIZE,
    &ACTIVITY_EVENTS,
    &HONEYPOT_HITS,
    &HONEYPOT_CLEANUPS,
    // Runtime and process.
    &RUNTIME_WORKERS,
    &RUNTIME_TASKS,
    &RUNTIME_GLOBAL_QUEUE_DEPTH,
    &RUNTIME_WORKER_BUSY_TIME,
    &RUNTIME_WORKER_PARKS,
    &PROCESS_CPU_TIME,
    &PROCESS_MEMORY_USAGE,
    &PROCESS_MEMORY_VIRTUAL,
    &PROCESS_FILE_DESCRIPTORS,
    &PROCESS_THREADS,
    &PROCESS_UPTIME,
    &BUILD_INFO,
];

/// Looks up an instrument by its exported name.
pub fn find(name: &str) -> Option<&'static InstrumentSpec> {
    INSTRUMENTS.iter().copied().find(|spec| spec.name == name)
}

/// The SDK view for catalog instruments: keeps only the declared attributes
/// and applies the declared histogram buckets. Instruments outside the
/// catalog keep the SDK defaults.
pub fn view(instrument: &Instrument) -> Option<Stream> {
    let spec = find(instrument.name())?;
    let mut stream = Stream::builder().with_allowed_attribute_keys(
        spec.attributes
            .iter()
            .map(|key| opentelemetry::Key::from_static_str(key)),
    );
    if spec.kind == InstrumentKind::Histogram {
        stream = stream.with_aggregation(Aggregation::ExplicitBucketHistogram {
            boundaries: spec.buckets.to_vec(),
            record_min_max: true,
        });
    }
    stream.build().ok()
}

/// Renders the catalog as the Markdown in `METRICS.md`.
pub fn render_markdown() -> String {
    let mut out = String::from(
        "# Quack metrics\n\n\
         <!-- Generated from crates/quack-telemetry/src/catalog. Do not edit by hand.\n     \
         Regenerate with: UPDATE_METRICS_DOC=1 cargo test -p quack-telemetry metrics_doc -->\n\n\
         Every instrument Quack exports over OTLP. Names start with `quack.` unless an\n\
         OpenTelemetry semantic convention defines the instrument. Durations are in\n\
         seconds. Metric attributes never carry ids; ids go on spans and logs.\n\
         Histograms also count: use a histogram's count for the rate of the thing it\n\
         times (requests, job runs, interactions handled).\n\n\
         ## Instruments\n\n\
         | Name | Type | Unit | Attributes | Description |\n\
         | --- | --- | --- | --- | --- |\n",
    );
    for spec in INSTRUMENTS {
        let attributes = if spec.attributes.is_empty() {
            "—".to_owned()
        } else {
            spec.attributes
                .iter()
                .map(|key| format!("`{key}`"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let _ = writeln!(
            out,
            "| `{}` | {} | `{}` | {} | {} |",
            spec.name,
            spec.kind.as_str(),
            spec.unit,
            attributes,
            spec.description
        );
    }

    out.push_str("\n## Histogram buckets\n\n| Name | Bucket boundaries |\n| --- | --- |\n");
    for spec in INSTRUMENTS.iter().filter(|spec| !spec.buckets.is_empty()) {
        let bounds = spec
            .buckets
            .iter()
            .map(f64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(out, "| `{}` | {} |", spec.name, bounds);
    }

    out.push_str("\n## Attributes\n\n| Key | Values | Description |\n| --- | --- | --- |\n");
    for attribute in ATTRIBUTES {
        let values = if attribute.values.is_empty() {
            "static names chosen in code".to_owned()
        } else {
            attribute
                .values
                .iter()
                .map(|value| format!("`{value}`"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let _ = writeln!(
            out,
            "| `{}` | {} | {} |",
            attribute.key, values, attribute.description
        );
    }
    out
}

#[cfg(test)]
mod tests;
