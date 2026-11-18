//! Metrics for the optional modules: tickets, server activity log, honeypot.

use opentelemetry::metrics::{Counter, Histogram, Meter, ObservableGauge};

use super::observe::{Source, counter, histogram, u64_gauge};
use crate::catalog;
use crate::labels::{ActivityEvent, ActivityStage, Attribute, HoneypotOutcome, Outcome};

/// Tickets module metrics.
pub struct TicketMetrics {
    opened: Counter<u64>,
    closed: Counter<u64>,
    transcript: Histogram<f64>,
    open: Source<u64>,
    _open_gauge: ObservableGauge<u64>,
}

impl TicketMetrics {
    pub(crate) fn new(meter: &Meter) -> Self {
        let open: Source<u64> = Source::new();
        let source = open.clone();
        let open_gauge = u64_gauge(meter, &catalog::TICKETS_OPEN, move |observer| {
            if let Some(count) = source.read() {
                observer.observe(count, &[]);
            }
        });
        Self {
            opened: counter(meter, &catalog::TICKETS_OPENED),
            closed: counter(meter, &catalog::TICKETS_CLOSED),
            transcript: histogram(meter, &catalog::TICKET_TRANSCRIPT_SIZE),
            open,
            _open_gauge: open_gauge,
        }
    }

    /// Counts a ticket opened.
    pub fn opened(&self) {
        self.opened.add(1, &[]);
    }

    /// Counts a ticket closed.
    pub fn closed(&self) {
        self.closed.add(1, &[]);
    }

    /// Records the size of a saved transcript.
    #[allow(
        clippy::cast_precision_loss,
        reason = "byte sizes far below 2^52 are exact in f64"
    )]
    pub fn transcript_saved(&self, bytes: u64) {
        self.transcript.record(bytes as f64, &[]);
    }

    /// Installs the supplier of how many tickets are open.
    pub fn observe_open(&self, supplier: impl Fn() -> u64 + Send + Sync + 'static) {
        self.open.set(supplier);
    }
}

/// Server activity log module metrics.
pub struct ActivityLogMetrics {
    events: Counter<u64>,
}

impl ActivityLogMetrics {
    pub(crate) fn new(meter: &Meter) -> Self {
        Self {
            events: counter(meter, &catalog::ACTIVITY_EVENTS),
        }
    }

    /// Counts an event reaching a stage (seen, posted, or dropped).
    pub fn event(&self, event: ActivityEvent, stage: ActivityStage) {
        self.events.add(1, &[event.kv(), stage.kv()]);
    }
}

/// Honeypot module metrics.
pub struct HoneypotMetrics {
    hits: Counter<u64>,
    cleanups: Counter<u64>,
}

impl HoneypotMetrics {
    pub(crate) fn new(meter: &Meter) -> Self {
        Self {
            hits: counter(meter, &catalog::HONEYPOT_HITS),
            cleanups: counter(meter, &catalog::HONEYPOT_CLEANUPS),
        }
    }

    /// Counts a post in a honeypot channel.
    pub fn hit(&self, outcome: HoneypotOutcome) {
        self.hits.add(1, &[outcome.kv()]);
    }

    /// Counts a cleanup run.
    pub fn cleanup(&self, outcome: Outcome) {
        self.cleanups.add(1, &[outcome.kv()]);
    }
}
