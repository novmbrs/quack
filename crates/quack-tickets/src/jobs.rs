//! The module's job kinds, registered with the engine's runner.

use std::sync::Arc;

use quack_core::ids::TicketId;
use quack_db::jobs::{ClaimedJob, HandlerFuture, JobKind, Phase};
use quack_engine::Engine;
use quack_engine::jobs::ModuleJobs;
use quack_engine::jobs::module::dead;

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
