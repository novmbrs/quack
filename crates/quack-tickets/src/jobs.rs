//! The module's job kinds, registered with the engine's runner.

use std::sync::Arc;

use quack_core::ids::TicketId;
use quack_db::jobs::{self, ClaimedJob, Handled, HandlerFuture, JobKind, JobResult, Phase, RunAt};
use quack_db::{DbError, Tx};
use quack_engine::Engine;
use quack_engine::jobs::ModuleJobs;

use crate::{close_job, open_job};

/// The tickets module's jobs, `open_ticket` and `close_ticket`, for
/// `quack_engine::jobs::runner` (and the test queue driver).
pub fn jobs(engine: Engine) -> ModuleJobs {
    ModuleJobs {
        kinds: &[JobKind::OpenTicket, JobKind::CloseTicket],
        handler: Arc::new(move |job: ClaimedJob, phase: Phase| -> HandlerFuture {
            let engine = engine.clone();
            Box::pin(async move {
                match (job.kind, phase) {
                    (JobKind::OpenTicket, Phase::Run) => open_job::run(&engine, &job).await,
                    (JobKind::OpenTicket, Phase::Finalize) => {
                        open_job::finalize(&engine, &job).await
                    }
                    (JobKind::CloseTicket, Phase::Run) => close_job::run(&engine, &job).await,
                    (JobKind::CloseTicket, Phase::Finalize) => {
                        close_job::finalize(&engine, &job).await
                    }
                    _ => Ok(dead("no_handler")),
                }
            })
        }),
    }
}

/// The `open_ticket` and `close_ticket` payload.
#[derive(Clone, Copy, Debug, serde::Deserialize)]
pub(crate) struct Payload {
    pub ticket_id: TicketId,
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
