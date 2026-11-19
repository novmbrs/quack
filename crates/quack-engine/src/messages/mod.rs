//! Tracked messages (§10): the `sync_message` job, the only writer of a
//! receipt, appeal post, ticket panel, ticket post, or honeypot warning.

mod render;
mod stale;
mod sync;

use quack_core::ids::MessageRowId;
use quack_core::messages::MessageKind;
use quack_db::jobs::{ClaimedJob, Handled, JobResult, settle as settle_job};
use quack_db::messages::{self, Owner, TrackedMessage};
use quack_db::{DbError, Tx};
use quack_telemetry::labels::Outcome as Label;
use quack_telemetry::metrics;
use quack_telemetry::spans::SpanEvent;

use crate::obs::{event, kv};
use crate::{Engine, obs};

pub use stale::message_deleted;
pub use sync::{finalize, sync};

/// The `sync_message` payload.
#[derive(Clone, Copy, Debug, serde::Deserialize)]
struct Payload {
    message_row_id: MessageRowId,
}

const fn kind(row: &TrackedMessage) -> MessageKind {
    match row.owner {
        Owner::Case(_) => MessageKind::CaseReceipt,
        Owner::Appeal(_) => MessageKind::AppealPost,
        Owner::Ticket(_) => MessageKind::TicketPost,
        Owner::Server(kind) => kind,
    }
}

/// Counts a finished sync and adds its span event (§10.3 metrics).
fn synced(row: &TrackedMessage, outcome: Label) {
    let k = kind(row);
    metrics().messages.synced(obs::message_kind(k), outcome);
    if outcome == Label::Done {
        let lag = obs::since(row.wanted_at, time::OffsetDateTime::now_utc());
        metrics().messages.lag(obs::message_kind(k), lag);
    }
    event(
        SpanEvent::MessageSynced,
        vec![
            kv("message_row.id", row.id.get()),
            kv("kind", k.as_str()),
            kv(
                "outcome",
                quack_telemetry::labels::Attribute::as_str(outcome),
            ),
        ],
    );
}

/// Ends a run with `result` in a fenced transaction holding `tx`'s writes.
async fn finish(mut tx: Tx, job: &ClaimedJob, result: JobResult) -> Result<Handled, DbError> {
    settle_job(&mut tx, job, &result).await?;
    tx.commit().await?;
    Ok(Handled::Settled(result))
}

/// A sync that can't write records the machine code and finishes; the
/// next bump tries again (§10.3).
async fn give_up(
    engine: &Engine,
    job: &ClaimedJob,
    row: &TrackedMessage,
    code: &str,
) -> Result<Handled, DbError> {
    let mut tx = engine.db().begin().await?;
    messages::set_error(&mut tx, row.guild, row.id, row.generation, code).await?;
    let handled = finish(tx, job, JobResult::Done).await?;
    synced(row, Label::Failed);
    Ok(handled)
}
