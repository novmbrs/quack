//! The engine's job handlers, registered with `quack-db`'s generic
//! [`Runner`] through one function the binary calls ([`handler`]).
//!
//! Every non-module kind of §16.5 is handled here; the optional modules
//! register their own kinds with [`ModuleJobs`].

use std::sync::Arc;

use quack_db::DbError;
use quack_db::jobs::{
    ClaimedJob, Handled, Handler, HandlerFuture, JobKind, JobResult, Phase, RunAt, Runner,
    RunnerConfig,
};

use crate::Engine;

/// The settle helpers the module crates' job handlers share.
pub mod module {
    use quack_db::jobs::{self, ClaimedJob, Handled, JobResult};
    use quack_db::{DbError, Tx};

    pub use super::dead;
    use crate::Engine;

    /// Retry with the kind's backoff after this attempt.
    pub fn retry(job: &ClaimedJob, error: impl Into<String>) -> Handled {
        Handled::Settle(super::retry(job, error))
    }

    /// A fenced transaction for a job's writes: lost leases roll back.
    pub async fn begin(engine: &Engine, job: &ClaimedJob) -> Result<Tx, DbError> {
        let mut tx = engine.db().begin().await?;
        jobs::fence(&mut tx, job).await?;
        Ok(tx)
    }

    /// Settles the job with `result` together with `tx`'s writes.
    pub async fn finish(
        mut tx: Tx,
        job: &ClaimedJob,
        result: JobResult,
    ) -> Result<Handled, DbError> {
        jobs::settle(&mut tx, job, &result).await?;
        tx.commit().await?;
        Ok(Handled::Settled(result))
    }
}

/// A module's job kinds and the handler that runs them (and finalizes
/// them when their budget is spent, `Phase::Finalize`).
#[derive(Clone)]
pub struct ModuleJobs {
    /// The kinds the module owns.
    pub kinds: &'static [JobKind],
    /// Its handler, called for every claimed job of those kinds.
    pub handler: Handler,
}

impl std::fmt::Debug for ModuleJobs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModuleJobs")
            .field("kinds", &self.kinds)
            .finish_non_exhaustive()
    }
}

/// The handler for the runner: the engine's kinds plus the modules'.
pub fn handler(engine: Engine, modules: Vec<ModuleJobs>) -> Handler {
    let modules = Arc::new(modules);
    Arc::new(move |job: ClaimedJob, phase: Phase| -> HandlerFuture {
        let engine = engine.clone();
        let modules = Arc::clone(&modules);
        Box::pin(async move {
            if let Some(module) = modules.iter().find(|m| m.kinds.contains(&job.kind)) {
                return (module.handler)(job, phase).await;
            }
            handle(&engine, &job, phase).await
        })
    })
}

/// A runner over [`handler`].
pub fn runner(engine: Engine, config: RunnerConfig, modules: Vec<ModuleJobs>) -> Runner {
    let db = engine.db().clone();
    Runner::new(db, config, handler(engine, modules))
}

/// Runs (or finalizes) one claimed job of an engine kind.
pub async fn handle(engine: &Engine, job: &ClaimedJob, phase: Phase) -> Result<Handled, DbError> {
    match (job.kind, phase) {
        (JobKind::RunAction, Phase::Run) => crate::actions::run(engine, job).await,
        (JobKind::RunAction, Phase::Finalize) => crate::actions::finalize(engine, job).await,
        (JobKind::SendDm, Phase::Run) => crate::dms::send(engine, job).await,
        (JobKind::SendDm, Phase::Finalize) => crate::dms::finalize(engine, job).await,
        (JobKind::SyncMessage, Phase::Run) => crate::messages::sync(engine, job).await,
        (JobKind::SyncMessage, Phase::Finalize) => crate::messages::finalize(engine, job).await,
        (JobKind::PostModLog, Phase::Run) => crate::modlog::post(engine, job).await,
        (JobKind::PostModLog, Phase::Finalize) => crate::modlog::finalize_post(engine, job).await,
        (JobKind::CaptureEvidence, Phase::Run) => crate::evidence::capture(engine, job).await,
        (JobKind::CaptureEvidence, Phase::Finalize) => crate::evidence::finalize(engine, job).await,
        (JobKind::Housekeeping, _) => housekeeping(engine, job).await,
        _ => Ok(dead("no_handler")),
    }
}

/// One `housekeeping` run (§16.6): deletes expired rows in batches for at
/// most 60 seconds, then runs again at once when rows remain, else in 10
/// minutes. Safe to repeat: it only deletes what has expired.
async fn housekeeping(engine: &Engine, job: &ClaimedJob) -> Result<Handled, DbError> {
    crate::obs::use_case("housekeeping", None, async {
        let mut conn = engine.db().conn().await?;
        let report = quack_db::housekeeping::run(
            &mut conn,
            &engine.config().session_key_fingerprint,
            HOUSEKEEPING_BUDGET,
        )
        .await?;
        let next = if report.more {
            RunAt::Now
        } else {
            RunAt::In(HOUSEKEEPING_EVERY)
        };
        tracing::debug!(job_id = %job.id.get(), ?report, "housekeeping pass");
        Ok(Handled::Settle(JobResult::Reschedule { at: next }))
    })
    .await
}

/// The longest one housekeeping run deletes for.
const HOUSEKEEPING_BUDGET: std::time::Duration = std::time::Duration::from_secs(60);
/// How often housekeeping runs when nothing is left.
const HOUSEKEEPING_EVERY: std::time::Duration = std::time::Duration::from_mins(10);

/// Retry with the kind's backoff after this attempt.
/// Whether the run has room for `calls` more Discord calls at the port's
/// 10-second deadline each, plus a 5-second margin before the claim's own
/// deadline (§16.1). Without it, the run continues in a new claim rather
/// than start a call the deadline could cut off mid-flight.
pub(crate) fn has_room(job: &ClaimedJob, calls: u32) -> bool {
    const CALL: std::time::Duration = std::time::Duration::from_secs(10);
    const MARGIN: std::time::Duration = std::time::Duration::from_secs(5);
    job.time_left() >= CALL * calls + MARGIN
}

pub(crate) fn retry(job: &ClaimedJob, error: impl Into<String>) -> JobResult {
    JobResult::Retry {
        at: RunAt::In(job.kind.policy().backoff(job.attempt)),
        error: error.into(),
    }
}

/// The job's records are missing or malformed: keep it as dead.
pub fn dead(error: &str) -> Handled {
    Handled::Settle(JobResult::Dead {
        error: error.to_owned(),
    })
}
