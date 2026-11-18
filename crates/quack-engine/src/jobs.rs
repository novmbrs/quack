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
        _ => Ok(dead("no_handler")),
    }
}

/// Retry with the kind's backoff after this attempt.
pub(crate) fn retry(job: &ClaimedJob, error: impl Into<String>) -> JobResult {
    JobResult::Retry {
        at: RunAt::In(job.kind.policy().backoff(job.attempt)),
        error: error.into(),
    }
}

/// The job's records are missing or malformed: keep it as dead.
pub(crate) fn dead(error: &str) -> Handled {
    Handled::Settle(JobResult::Dead {
        error: error.to_owned(),
    })
}
