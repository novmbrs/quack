//! The module's job kinds, registered with the engine's runner.

use std::sync::Arc;

use quack_db::jobs::{self, ClaimedJob, Handled, HandlerFuture, JobKind, JobResult, Phase, RunAt};
use quack_db::{DbError, Tx};
use quack_engine::Engine;
use quack_engine::jobs::ModuleJobs;

use crate::{case_job, cleanup_job};

/// The honeypot module's jobs, `honeypot_case` and `honeypot_cleanup`, for
/// `quack_engine::jobs::runner` (and the test queue driver).
pub fn jobs(engine: Engine) -> ModuleJobs {
    ModuleJobs {
        kinds: &[JobKind::HoneypotCase, JobKind::HoneypotCleanup],
        handler: Arc::new(move |job: ClaimedJob, phase: Phase| -> HandlerFuture {
            let engine = engine.clone();
            Box::pin(async move {
                match (job.kind, phase) {
                    (JobKind::HoneypotCase, Phase::Run) => case_job::run(&engine, &job).await,
                    (JobKind::HoneypotCase, Phase::Finalize) => {
                        case_job::finalize(&engine, &job).await
                    }
                    (JobKind::HoneypotCleanup, Phase::Run) => cleanup_job::run(&engine, &job).await,
                    (JobKind::HoneypotCleanup, Phase::Finalize) => {
                        cleanup_job::finalize(&engine, &job).await
                    }
                    _ => Ok(dead("no_handler")),
                }
            })
        }),
    }
}

/// Retry with the kind's backoff after this attempt.
pub(crate) fn retry(job: &ClaimedJob, error: impl Into<String>) -> Handled {
    Handled::Settle(JobResult::Retry {
        at: RunAt::In(job.kind.policy().backoff(job.attempt)),
        error: error.into(),
    })
}

/// The job's records are missing or malformed: keep it as dead.
pub(crate) fn dead(error: &str) -> Handled {
    Handled::Settle(JobResult::Dead {
        error: error.to_owned(),
    })
}

/// A fenced transaction for a job's writes: lost leases roll back.
pub(crate) async fn begin(engine: &Engine, job: &ClaimedJob) -> Result<Tx, DbError> {
    let mut tx = engine.db().begin().await?;
    jobs::fence(&mut tx, job).await?;
    Ok(tx)
}

/// Settles the job with `result` together with `tx`'s writes.
pub(crate) async fn finish(
    mut tx: Tx,
    job: &ClaimedJob,
    result: JobResult,
) -> Result<Handled, DbError> {
    jobs::settle(&mut tx, job, &result).await?;
    tx.commit().await?;
    Ok(Handled::Settled(result))
}
