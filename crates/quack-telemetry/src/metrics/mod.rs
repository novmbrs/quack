//! The typed metric catalog: [`Metrics`] and its families.
//!
//! Build it once with [`Metrics::new`] (tests) or use the process-wide one
//! from [`metrics()`](crate::metrics), which [`init`](crate::init) builds
//! from the global meter. Each family has record methods that take typed
//! attributes, so names, units, and attribute sets come only from the
//! catalog.

mod db;
mod discord;
mod http;
mod interactions;
mod jobs;
mod memory;
mod moderation;
mod modules;
mod observe;
mod process;
mod runtime;
mod tally;

use std::sync::OnceLock;

use opentelemetry::metrics::Meter;
use opentelemetry::{InstrumentationScope, global};
use opentelemetry_semantic_conventions::SCHEMA_URL;

pub use db::{DbMetrics, PoolSample};
pub use discord::{DiscordMetrics, GatewayMetrics};
pub use http::{HttpMetrics, InFlightRequest};
pub use interactions::InteractionMetrics;
pub use jobs::{JobMetrics, JobQueueSample};
pub use memory::{CacheMetrics, QueueMetrics};
pub use moderation::{
    ActionMetrics, AppealMetrics, CaseMetrics, DmMetrics, EvidenceMetrics, ModLogMetrics,
    TrackedMessageMetrics,
};
pub use modules::{ActivityLogMetrics, HoneypotMetrics, TicketMetrics};
pub use runtime::RuntimeMetrics;
pub use tally::{JobTally, KindRuns, TallyRead};

/// The instrumentation scope name for every Quack instrument.
pub const SCOPE: &str = "quack";

/// Every Quack metric instrument, grouped by family.
pub struct Metrics {
    /// Discord interactions.
    pub interactions: InteractionMetrics,
    /// Cases.
    pub cases: CaseMetrics,
    /// Actions and undo actions.
    pub actions: ActionMetrics,
    /// Case and appeal DMs.
    pub dms: DmMetrics,
    /// Appeals.
    pub appeals: AppealMetrics,
    /// Evidence capture.
    pub evidence: EvidenceMetrics,
    /// The moderation log and log channel.
    pub modlog: ModLogMetrics,
    /// Tracked Discord messages.
    pub messages: TrackedMessageMetrics,
    /// The job queue.
    pub jobs: JobMetrics,
    /// Discord REST calls.
    pub discord: DiscordMetrics,
    /// The Discord gateway.
    pub gateway: GatewayMetrics,
    /// The HTTP API and sign-in.
    pub http: HttpMetrics,
    /// PostgreSQL.
    pub db: DbMetrics,
    /// Bounded in-memory caches.
    pub caches: CacheMetrics,
    /// Bounded in-memory queues.
    pub queues: QueueMetrics,
    /// The tickets module.
    pub tickets: TicketMetrics,
    /// The server activity log module.
    pub activity_log: ActivityLogMetrics,
    /// The honeypot module.
    pub honeypot: HoneypotMetrics,
    /// Tokio, the process, and build info.
    pub runtime: RuntimeMetrics,
}

impl Metrics {
    /// Builds every instrument from `meter`. `version` is reported on
    /// `quack.build.info`.
    pub fn new(meter: &Meter, version: &str) -> Self {
        Self {
            interactions: InteractionMetrics::new(meter),
            cases: CaseMetrics::new(meter),
            actions: ActionMetrics::new(meter),
            dms: DmMetrics::new(meter),
            appeals: AppealMetrics::new(meter),
            evidence: EvidenceMetrics::new(meter),
            modlog: ModLogMetrics::new(meter),
            messages: TrackedMessageMetrics::new(meter),
            jobs: JobMetrics::new(meter),
            discord: DiscordMetrics::new(meter),
            gateway: GatewayMetrics::new(meter),
            http: HttpMetrics::new(meter),
            db: DbMetrics::new(meter),
            caches: CacheMetrics::new(meter),
            queues: QueueMetrics::new(meter),
            tickets: TicketMetrics::new(meter),
            activity_log: ActivityLogMetrics::new(meter),
            honeypot: HoneypotMetrics::new(meter),
            runtime: RuntimeMetrics::new(meter, version),
        }
    }
}

static GLOBAL: OnceLock<Metrics> = OnceLock::new();

/// The process-wide catalog. [`init`](crate::init) builds it from the
/// exporting meter provider; called before `init` (or without it), it is
/// built from whatever global provider exists, normally the no-op one.
pub fn metrics() -> &'static Metrics {
    GLOBAL.get_or_init(|| Metrics::new(&global_meter(env!("CARGO_PKG_VERSION")), "unknown"))
}

/// Builds the process-wide catalog from the current global meter provider.
/// Returns `false` if it already existed (and so may be bound to an earlier
/// provider).
pub(crate) fn install_global(version: &str) -> bool {
    let mut installed = false;
    GLOBAL.get_or_init(|| {
        installed = true;
        Metrics::new(&global_meter(version), version)
    });
    installed
}

/// The meter every Quack instrument uses, tagged with the semconv schema
/// the catalog follows.
pub(crate) fn global_meter(version: &str) -> Meter {
    global::meter_with_scope(scope(version))
}

/// The instrumentation scope for Quack's meter and tracer.
pub(crate) fn scope(version: &str) -> InstrumentationScope {
    InstrumentationScope::builder(SCOPE)
        .with_version(version.to_owned())
        .with_schema_url(SCHEMA_URL)
        .build()
}
