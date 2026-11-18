//! Job queue metrics.

use std::time::Duration;

use opentelemetry::metrics::{Counter, Histogram, Meter, ObservableGauge};

use super::observe::{Source, counter, f64_gauge, histogram, u64_gauge};
use crate::catalog;
use crate::labels::{Attribute, JobKind, JobOutcome};

/// The ready backlog of one job kind, sampled by the queue's owner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JobQueueSample {
    /// The job kind.
    pub kind: JobKind,
    /// Jobs ready to run now and not yet claimed.
    pub ready: u64,
    /// How long the oldest ready job has been waiting.
    pub oldest_ready_age: Duration,
}

/// Job metrics: enqueued, started, finished, queue wait, and the backlog.
pub struct JobMetrics {
    enqueued: Counter<u64>,
    started: Counter<u64>,
    duration: Histogram<f64>,
    queue_wait: Histogram<f64>,
    backlog: Source<Vec<JobQueueSample>>,
    _ready: ObservableGauge<u64>,
    _oldest: ObservableGauge<f64>,
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
        Self {
            enqueued: counter(meter, &catalog::JOBS_ENQUEUED),
            started: counter(meter, &catalog::JOBS_STARTED),
            duration: histogram(meter, &catalog::JOB_DURATION),
            queue_wait: histogram(meter, &catalog::JOB_QUEUE_WAIT),
            backlog,
            _ready: ready,
            _oldest: oldest,
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

    /// Installs the supplier of the ready backlog per job kind. Kinds left
    /// out report nothing for that collection.
    pub fn observe_backlog(
        &self,
        supplier: impl Fn() -> Vec<JobQueueSample> + Send + Sync + 'static,
    ) {
        self.backlog.set(supplier);
    }
}
