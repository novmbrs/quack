//! Evidence (§7): the `capture_evidence` job handler and its finalize.
//! Capture happens after the case is committed, so creating a case never
//! waits on Discord fetches or file copies.

mod access;
mod capture;
mod copy;

use quack_core::evidence::EvidenceFailure;
use quack_core::ids::EvidenceId;
use quack_db::evidence::{self, Evidence};
use quack_db::jobs::{ClaimedJob, Handled, JobResult, settle as settle_job};
use quack_db::messages::{self, Owner};
use quack_db::{DbError, cases};
use quack_telemetry::labels::Outcome as Label;
use quack_telemetry::metrics;
use quack_telemetry::spans::SpanEvent;

use crate::Engine;
use crate::obs::{event, kv};

pub use capture::{capture, finalize};

/// The `capture_evidence` payload.
#[derive(Clone, Copy, Debug, serde::Deserialize)]
struct Payload {
    evidence_id: EvidenceId,
}

/// Fails the item (`saving` → `failed`), bumps the receipt, and finishes.
async fn fail(
    engine: &Engine,
    job: &ClaimedJob,
    item: &Evidence,
    failure: EvidenceFailure,
) -> Result<Handled, DbError> {
    let mut tx = engine.db().begin().await?;
    cases::lock(&mut tx, item.guild, item.case).await?;
    evidence::lock(&mut tx, item.guild, item.id).await?;
    evidence::mark_failed(&mut tx, item.guild, item.id, failure).await?;
    messages::bump(&mut tx, item.guild, Owner::Case(item.case)).await?;
    settle_job(&mut tx, job, &JobResult::Done).await?;
    tx.commit().await?;
    captured(item, Label::Failed, 0, 0, "failed");
    Ok(Handled::Settled(JobResult::Done))
}

/// Counts a finished capture and adds its span event.
fn captured(item: &Evidence, outcome: Label, files: u64, bytes: u64, state: &str) {
    metrics().evidence.captured(outcome, files, bytes);
    event(
        SpanEvent::EvidenceCaptured,
        vec![kv("evidence.id", item.id.get()), kv("state", state)],
    );
}
