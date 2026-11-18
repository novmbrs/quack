//! The module's job kinds, registered with the engine's runner.

use std::sync::Arc;

use quack_db::jobs::{ClaimedJob, HandlerFuture, JobKind, Phase};
use quack_engine::Engine;
use quack_engine::jobs::ModuleJobs;
use quack_engine::jobs::module::dead;

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
