//! Interaction metrics.

use std::time::Duration;

use opentelemetry::metrics::{Counter, Histogram, Meter};

use super::observe::{counter, histogram};
use crate::catalog;
use crate::labels::{Attribute, CommandName, InteractionKind, InteractionOutcome, Reason};

/// Metrics for Discord interactions: acknowledgement latency, handling time,
/// and errors.
pub struct InteractionMetrics {
    ack: Histogram<f64>,
    duration: Histogram<f64>,
    errors: Counter<u64>,
}

impl InteractionMetrics {
    pub(crate) fn new(meter: &Meter) -> Self {
        Self {
            ack: histogram(meter, &catalog::INTERACTION_ACK_DURATION),
            duration: histogram(meter, &catalog::INTERACTION_DURATION),
            errors: counter(meter, &catalog::INTERACTION_ERRORS),
        }
    }

    /// Records the time from receiving the interaction to acknowledging it.
    pub fn acked(&self, kind: InteractionKind, command: CommandName, latency: Duration) {
        self.ack
            .record(latency.as_secs_f64(), &[kind.kv(), command.kv()]);
    }

    /// Records a handled interaction and its total handling time. For an
    /// outcome other than done, also call [`Self::error`] with the reason.
    pub fn finished(
        &self,
        kind: InteractionKind,
        command: CommandName,
        outcome: InteractionOutcome,
        duration: Duration,
    ) {
        self.duration.record(
            duration.as_secs_f64(),
            &[kind.kv(), command.kv(), outcome.kv()],
        );
    }

    /// Counts an interaction that ended refused or failed, by reason.
    pub fn error(&self, kind: InteractionKind, command: CommandName, reason: Reason) {
        self.errors.add(1, &[kind.kv(), command.kv(), reason.kv()]);
    }
}
