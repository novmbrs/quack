//! Telemetry for every query, transaction, and pool, in one place.
//!
//! Each query runs through [`run`], which wraps it in a `{operation} {table}`
//! client span, records `db.client.operation.duration`, and classifies a
//! failure into `error.type`. Bind values and SQL text never reach spans,
//! metrics, or logs. All calls into `quack-telemetry` live in this module.

use std::future::Future;
use std::time::{Duration, Instant};

use quack_telemetry::labels::{
    DbCollection, DbErrorClass, DbOperation, PoolName, TransactionOutcome,
};
use quack_telemetry::{PoolSample, metrics, spans};
use tracing::Instrument as _;

use crate::DbError;

pub(crate) use quack_telemetry::labels::DbOperation::{Delete, Insert, Notify, Select, Update};

/// The pool name on connection metrics.
pub(crate) const POOL: PoolName = PoolName::new("main");

/// Runs one query future inside its client span and records its duration.
///
/// `table` is the statement's main table (a static name chosen in code); a
/// CTE is recorded by its business verb. `RowNotFound` is not a failure
/// here: it becomes [`DbError::NotFound`] without an `error.type`.
pub(crate) async fn run<T, F>(
    operation: DbOperation,
    table: &'static str,
    query: F,
) -> Result<T, DbError>
where
    F: Future<Output = Result<T, sqlx::Error>>,
{
    let collection = DbCollection::new(table);
    let span = spans::db(operation, collection);
    let started = Instant::now();
    let result = query.instrument(span.clone()).await;
    let elapsed = started.elapsed();
    match result {
        Ok(value) => {
            metrics().db.query(operation, collection, elapsed, None);
            Ok(value)
        }
        Err(error) => {
            let error = DbError::from(error);
            if matches!(error, DbError::NotFound) {
                metrics().db.query(operation, collection, elapsed, None);
            } else {
                let class = error.class();
                spans::record(&span, class);
                spans::record_error(&span, class_name(class));
                metrics()
                    .db
                    .query(operation, collection, elapsed, Some(class));
            }
            Err(error)
        }
    }
}

fn class_name(class: DbErrorClass) -> &'static str {
    quack_telemetry::labels::Attribute::as_str(class)
}

/// Records a finished transaction.
pub(crate) fn transaction(outcome: TransactionOutcome, duration: Duration) {
    metrics().db.transaction(outcome, duration);
}

/// Counts a transaction run again after `class` (serialization or deadlock).
pub(crate) fn transaction_retry(class: DbErrorClass) {
    metrics().db.transaction_retry(class);
}

/// Records how long acquiring a pool connection took, or that it timed out.
pub(crate) fn connection_acquired(wait: Duration, error: Option<&sqlx::Error>) {
    if matches!(error, Some(sqlx::Error::PoolTimedOut)) {
        metrics().db.connection_timeout(POOL);
    } else {
        metrics().db.connection_wait(POOL, wait);
    }
}

/// Installs the pool snapshot supplier for the connection gauges.
pub(crate) fn observe_pool(pool: sqlx::PgPool) {
    metrics().db.observe_pools(move || {
        let max = pool.options().get_max_connections();
        let size = pool.size();
        let idle = u32::try_from(pool.num_idle()).unwrap_or(u32::MAX).min(size);
        vec![PoolSample {
            pool: POOL,
            max,
            idle,
            used: size - idle,
        }]
    });
}

/// Something to record only once its transaction has committed.
#[derive(Clone, Copy, Debug)]
pub(crate) enum AfterCommit {
    /// A job was enqueued (`quack.jobs.enqueued`).
    JobEnqueued(crate::jobs::JobKind),
    /// A moderation log entry was written (`quack.modlog.entries`).
    ModLogEntry(
        quack_core::modlog::ModLogKind,
        quack_core::modlog::ModLogResult,
    ),
}

impl AfterCommit {
    pub(crate) fn record(self) {
        match self {
            Self::JobEnqueued(kind) => metrics().jobs.enqueued(job_kind(kind)),
            Self::ModLogEntry(kind, result) => {
                metrics().modlog.entry(
                    quack_telemetry::labels::ModLogKind::new(kind.as_str()),
                    modlog_result(result),
                );
            }
        }
    }
}

fn modlog_result(
    result: quack_core::modlog::ModLogResult,
) -> quack_telemetry::labels::ModLogResult {
    use quack_core::modlog::ModLogResult as R;
    use quack_telemetry::labels::ModLogResult as L;
    match result {
        R::Ok => L::Ok,
        R::Failed => L::Failed,
        R::Refused => L::Refused,
    }
}

/// How one run of a job ended, for `quack.job.duration` and the span.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RunOutcome {
    Done,
    Retry,
    Deferred,
    Continued,
    Rescheduled,
    Finalized,
    Dead,
    LostLease,
    TimedOut,
    /// The run finished but its result could not be stored; the lease
    /// expires and the job runs again.
    Failed,
}

impl RunOutcome {
    pub(crate) fn of(result: &crate::jobs::JobResult, phase: crate::jobs::Phase) -> Self {
        use crate::jobs::JobResult as R;
        if phase == crate::jobs::Phase::Finalize {
            return Self::Finalized;
        }
        match result {
            R::Done => Self::Done,
            R::Retry { .. } => Self::Retry,
            R::Defer { .. } => Self::Deferred,
            R::Continue => Self::Continued,
            R::Reschedule { .. } => Self::Rescheduled,
            R::Dead { .. } => Self::Dead,
        }
    }

    fn label(self) -> quack_telemetry::labels::JobOutcome {
        use quack_telemetry::labels::JobOutcome as L;
        match self {
            Self::Done => L::Done,
            Self::Retry | Self::Failed => L::Retry,
            Self::Deferred => L::Deferred,
            Self::Continued => L::Continued,
            Self::Rescheduled => L::Rescheduled,
            Self::Finalized => L::Finalized,
            Self::Dead => L::Dead,
            Self::LostLease => L::LostLease,
            Self::TimedOut => L::TimedOut,
        }
    }
}

/// The telemetry label of a job kind (the same names).
pub(crate) fn job_kind(kind: crate::jobs::JobKind) -> quack_telemetry::labels::JobKind {
    use crate::jobs::JobKind as K;
    use quack_telemetry::labels::JobKind as L;
    match kind {
        K::RunAction => L::RunAction,
        K::SendDm => L::SendDm,
        K::SyncMessage => L::SyncMessage,
        K::PostModLog => L::PostModLog,
        K::CaptureEvidence => L::CaptureEvidence,
        K::HoneypotCase => L::HoneypotCase,
        K::HoneypotCleanup => L::HoneypotCleanup,
        K::OpenTicket => L::OpenTicket,
        K::CloseTicket => L::CloseTicket,
        K::Housekeeping => L::Housekeeping,
    }
}

/// The `job {kind}` span for one run: a new root linked to the enqueuer.
pub(crate) fn job_span(job: &crate::jobs::ClaimedJob) -> tracing::Span {
    spans::job(&spans::JobRun {
        kind: job_kind(job.kind),
        job_id: job.id,
        attempt: job.attempt,
        max_attempts: job.max_attempts,
        queue_wait: job.queue_wait(),
        guild_id: job.guild.map(quack_core::ids::GuildId::get),
        enqueued_by: job.trace_context.as_deref(),
    })
}

/// Counts a run starting and its queue wait.
pub(crate) fn job_started(kind: crate::jobs::JobKind, queue_wait: Duration) {
    metrics().jobs.started(job_kind(kind), queue_wait);
}

/// Records a finished run on its span and in `quack.job.duration`.
pub(crate) fn job_finished(
    span: &tracing::Span,
    kind: crate::jobs::JobKind,
    outcome: RunOutcome,
    duration: Duration,
) {
    spans::record(span, outcome.label());
    metrics()
        .jobs
        .finished(job_kind(kind), outcome.label(), duration);
}

/// Counts jobs whose lease expired (`quack.job.lease_expired`).
pub(crate) fn lease_expired(kind: crate::jobs::JobKind, jobs: u64) {
    for _ in 0..jobs {
        metrics().jobs.lease_expired(job_kind(kind));
    }
}

/// The `error.type` value of an error, for job `last_error` codes.
pub(crate) fn error_class(error: &DbError) -> &'static str {
    class_name(error.class())
}

/// The gauge sample of one kind's backlog.
pub(crate) fn backlog_sample(backlog: &crate::jobs::JobBacklog) -> quack_telemetry::JobQueueSample {
    quack_telemetry::JobQueueSample {
        kind: job_kind(backlog.kind),
        ready: backlog.ready,
        oldest_ready_age: backlog.oldest_ready_age,
        dead: backlog.dead,
    }
}
