//! Job queue metrics.

use std::time::Duration;

use opentelemetry::metrics::{Counter, Histogram, Meter, ObservableGauge};

use super::observe::{Source, counter, f64_gauge, histogram, u64_gauge};
use crate::catalog;
use crate::labels::{Attribute, JobKind, JobOutcome};

/// The backlog of one job kind, sampled by the gauge task with the claim's
/// predicates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JobQueueSample {
    /// The job kind.
    pub kind: JobKind,
    /// Jobs ready to run now and not yet claimed.
    pub ready: u64,
    /// How long the oldest ready job has been waiting.
    pub oldest_ready_age: Duration,
    /// Dead jobs kept for inspection.
    pub dead: u64,
}

/// Job metrics: enqueued, started, finished, queue wait, lease expiries,
/// and the backlog.
pub struct JobMetrics {
    enqueued: Counter<u64>,
    started: Counter<u64>,
    duration: Histogram<f64>,
    queue_wait: Histogram<f64>,
    lease_expired: Counter<u64>,
    backlog: Source<Vec<JobQueueSample>>,
    _ready: ObservableGauge<u64>,
    _oldest: ObservableGauge<f64>,
    _dead: ObservableGauge<u64>,
}

impl JobMetrics {
    pub(crate) fn new(meter: &Meter) -> Self {
        let backlog: Source<Vec<JobQueueSample>> = Source::new();
        let source = backlog.clone();
        let ready = u64_gauge(meter, &catalog::JOBS_READY, move |observer| {
            for sample in source.read().unwrap_or_default() {
                observer.observe(sample.ready, &[sample.kind.kv()]);
            }
        });
        let source = backlog.clone();
        let oldest = f64_gauge(meter, &catalog::JOBS_OLDEST_READY_AGE, move |observer| {
            for sample in source.read().unwrap_or_default() {
                observer.observe(sample.oldest_ready_age.as_secs_f64(), &[sample.kind.kv()]);
            }
        });
        let source = backlog.clone();
        let dead = u64_gauge(meter, &catalog::JOBS_DEAD, move |observer| {
            for sample in source.read().unwrap_or_default() {
                observer.observe(sample.dead, &[sample.kind.kv()]);
            }
        });
        Self {
            enqueued: counter(meter, &catalog::JOBS_ENQUEUED),
            started: counter(meter, &catalog::JOBS_STARTED),
            duration: histogram(meter, &catalog::JOB_DURATION),
            queue_wait: histogram(meter, &catalog::JOB_QUEUE_WAIT),
            lease_expired: counter(meter, &catalog::JOB_LEASE_EXPIRED),
            backlog,
            _ready: ready,
            _oldest: oldest,
            _dead: dead,
        }
    }

    /// Counts a job added to the queue.
    pub fn enqueued(&self, kind: JobKind) {
        self.enqueued.add(1, &[kind.kv()]);
    }

    /// Counts a run starting and records how long the job waited after it
    /// was ready.
    pub fn started(&self, kind: JobKind, queue_wait: Duration) {
        self.started.add(1, &[kind.kv()]);
        self.queue_wait
            .record(queue_wait.as_secs_f64(), &[kind.kv()]);
    }

    /// Records a finished run and how long it took.
    pub fn finished(&self, kind: JobKind, outcome: JobOutcome, duration: Duration) {
        self.duration
            .record(duration.as_secs_f64(), &[kind.kv(), outcome.kv()]);
    }

    /// Counts a claimed job whose lease expired before its run settled it.
    pub fn lease_expired(&self, kind: JobKind) {
        self.lease_expired.add(1, &[kind.kv()]);
    }

    /// Installs the supplier of the ready and dead backlog per job kind. Kinds left
    /// out report nothing for that collection.
    pub fn observe_backlog(
        &self,
        supplier: impl Fn() -> Vec<JobQueueSample> + Send + Sync + 'static,
    ) {
        self.backlog.set(supplier);
    }
}
