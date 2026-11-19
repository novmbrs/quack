//! One claimed job's run: the deadline, the handler call with panics
//! caught, finalize when the budget is spent, and the settle.

use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;

use tracing::Instrument as _;

use super::admit::Permit;
use super::{Handled, HandlerFuture, Phase, Shared};
use crate::jobs::{ClaimedJob, JobResult, RunAt, settle};
use crate::obs::{self, RunOutcome};
use crate::{Db, DbError};

/// How many finalize runs a job gets before it is kept as dead: finalize
/// is database-only, so a failure that repeats this often needs an
/// operator, not another lease.
pub const FINALIZE_TRIES: u32 = 3;

/// Runs one claimed job under its deadline and records how it ended.
pub(super) async fn run_job(shared: Arc<Shared>, job: ClaimedJob, permit: Permit) {
    let kind = job.kind;
    let span = obs::job_span(&job);
    let started = Instant::now();
    obs::job_started(kind, job.queue_wait());
    let deadline = tokio::time::Instant::from_std(job.deadline);
    let outcome = async {
        tokio::select! {
            result = tokio::time::timeout_at(deadline, run_phases(&shared, &job)) => {
                result.unwrap_or(RunOutcome::TimedOut)
            }
            () = shared.hard_stop.cancelled() => RunOutcome::TimedOut,
        }
    }
    .instrument(span.clone())
    .await;
    obs::job_finished(&span, kind, outcome, started.elapsed());
    drop(permit);
}

/// Why a handler call produced no result.
enum Failure {
    Db(DbError),
    Panic,
}

impl Failure {
    fn code(&self) -> String {
        match self {
            Self::Db(error) => format!("db_{}", obs::error_class(error)),
            Self::Panic => "panic".to_owned(),
        }
    }

    /// Whether running the same finalize again can't help (the records
    /// can't be read or the values can't be stored).
    fn is_permanent(&self) -> bool {
        matches!(
            self,
            Self::Db(DbError::Decode(_) | DbError::Invalid(_) | DbError::Rejected { .. })
        )
    }
}

/// Calls the handler with panics caught, both while it builds its future
/// and while the future runs.
async fn call(shared: &Shared, job: &ClaimedJob, phase: Phase) -> Result<Handled, Failure> {
    let future = catch_unwind(AssertUnwindSafe(|| (shared.handler)(job.clone(), phase)))
        .map_err(|_| Failure::Panic)?;
    match (CatchUnwind { future }).await {
        Some(Ok(handled)) => Ok(handled),
        Some(Err(error)) => Err(Failure::Db(error)),
        None => Err(Failure::Panic),
    }
}

/// Calls the handler (or finalize), then settles when asked to.
async fn run_phases(shared: &Shared, job: &ClaimedJob) -> RunOutcome {
    let mut phase = if job.budget_spent() {
        Phase::Finalize
    } else {
        Phase::Run
    };
    loop {
        let result = match call(shared, job, phase).await {
            Ok(Handled::Settled(result)) => return RunOutcome::of(&result, phase),
            Ok(Handled::Settle(result)) => result,
            Err(Failure::Db(DbError::Stale)) => return RunOutcome::LostLease,
            Err(failure) => failed(job, phase, &failure),
        };
        if phase == Phase::Run && matches!(result, JobResult::Retry { .. }) && job.is_last_attempt()
        {
            phase = Phase::Finalize;
            continue;
        }
        return match settle_alone(&shared.db, job, &result).await {
            Ok(()) if matches!(result, JobResult::Dead { .. }) => RunOutcome::Dead,
            Ok(()) => RunOutcome::of(&result, phase),
            Err(DbError::Stale) => RunOutcome::LostLease,
            Err(error) => {
                tracing::warn!(job_kind = %job.kind, error = %error, "settling a job failed");
                RunOutcome::Failed
            }
        };
    }
}

/// The result for a handler call that failed: a run retries with the
/// kind's backoff; a finalize retries a few times, then the job is kept as
/// dead for an operator (its domain record could not be settled).
fn failed(job: &ClaimedJob, phase: Phase, failure: &Failure) -> JobResult {
    let error = failure.code();
    let retry = JobResult::Retry {
        at: RunAt::In(job.kind.policy().backoff(job.attempt)),
        error: error.clone(),
    };
    if phase == Phase::Run {
        tracing::warn!(job_kind = %job.kind, job_id = %job.id, error = %error, "job handler failed");
        return retry;
    }
    let tries = job.attempt.saturating_sub(job.max_attempts).max(1);
    if failure.is_permanent() || tries >= FINALIZE_TRIES {
        tracing::error!(
            job_kind = %job.kind,
            job_id = %job.id,
            error = %error,
            "job finalize could not settle its records; the job is kept as dead for an operator"
        );
        return JobResult::Dead {
            error: format!("finalize_{error}"),
        };
    }
    tracing::warn!(job_kind = %job.kind, job_id = %job.id, error = %error, "job finalize failed");
    retry
}

async fn settle_alone(db: &Db, job: &ClaimedJob, result: &JobResult) -> Result<(), DbError> {
    let (job, result) = (job.clone(), result.clone());
    db.transaction(move |tx| {
        let (job, result) = (job.clone(), result.clone());
        Box::pin(async move { settle(tx, &job, &result).await })
    })
    .await
}

/// A handler future with panics caught: `None` when it panicked.
struct CatchUnwind {
    future: HandlerFuture,
}

impl Future for CatchUnwind {
    type Output = Option<Result<Handled, DbError>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let future = &mut self.future;
        match catch_unwind(AssertUnwindSafe(|| future.as_mut().poll(cx))) {
            Ok(Poll::Ready(result)) => Poll::Ready(Some(result)),
            Ok(Poll::Pending) => Poll::Pending,
            Err(_) => Poll::Ready(None),
        }
    }
}
