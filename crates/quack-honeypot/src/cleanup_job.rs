//! The `honeypot_cleanup` job (§13.4.2 step 4): deletes one honeypot post
//! after its case saved it. Safe to repeat: a delete is idempotent (404
//! counts as done) and the record is a compare-and-set.

use std::time::Duration;

use quack_core::evidence::EvidenceState;
use quack_core::ids::{ChannelId, GuildId, MessageId};
use quack_db::jobs::{ClaimedJob, Handled, JobResult, RunAt};
use quack_db::{DbError, evidence, honeypot};
use quack_engine::Engine;
use quack_engine::port::{Outcome, RefusalKind};
use quack_telemetry::labels::Outcome as Label;
use quack_telemetry::metrics;
use time::OffsetDateTime;

use crate::jobs::{begin, dead, finish, retry};
use crate::obs::span;

/// The `honeypot_cleanup` payload.
#[derive(Clone, Copy, Debug, serde::Deserialize)]
struct Payload {
    channel_id: ChannelId,
    message_id: MessageId,
}

/// The longest a cleanup waits for the case to save its evidence.
const EVIDENCE_WAIT: time::Duration = time::Duration::minutes(10);
/// How often it looks again while waiting.
const EVIDENCE_POLL: Duration = Duration::from_secs(5);

/// Deletes the post once the case's evidence for it is no longer saving.
pub(crate) async fn run(engine: &Engine, job: &ClaimedJob) -> Result<Handled, DbError> {
    let (Some(guild), Ok(p)) = (job.guild, job.payload::<Payload>()) else {
        return Ok(dead("bad_payload"));
    };
    span("job honeypot_cleanup", guild, async {
        let mut conn = engine.db().conn().await?;
        let Some(hit) = honeypot::hit(&mut conn, guild, p.message_id).await? else {
            return Ok(Handled::Settle(JobResult::Done));
        };
        let Some(case) = hit.case else {
            // Only hits that became cases are cleaned up.
            return Ok(Handled::Settle(JobResult::Done));
        };
        let saving = evidence::for_case(&mut conn, guild, case)
            .await?
            .iter()
            .any(|e| e.message == p.message_id && e.state == EvidenceState::Saving);
        drop(conn);
        if saving && OffsetDateTime::now_utc() - job.created_at < EVIDENCE_WAIT {
            return Ok(Handled::Settle(JobResult::Defer {
                at: RunAt::In(EVIDENCE_POLL),
            }));
        }
        let failed = match engine
            .discord()
            .delete_message(p.channel_id, p.message_id)
            .await
        {
            Outcome::Done(()) => false,
            Outcome::Refused(r) if r.kind == RefusalKind::UnknownMessage => false,
            Outcome::Refused(_) => true,
            other => return Ok(retry(job, other.code())),
        };
        record(engine, job, guild, p.message_id, failed).await
    })
    .await
}

async fn record(
    engine: &Engine,
    job: &ClaimedJob,
    guild: GuildId,
    message: MessageId,
    failed: bool,
) -> Result<Handled, DbError> {
    let mut tx = begin(engine, job).await?;
    let changed = honeypot::mark_cleanup(&mut tx, guild, message, failed).await?;
    let handled = finish(tx, job, JobResult::Done).await?;
    if changed {
        metrics()
            .honeypot
            .cleanup(if failed { Label::Failed } else { Label::Done });
    }
    Ok(handled)
}

/// Finalization (budget spent, database only): the post stays and staff
/// can delete it.
pub(crate) async fn finalize(engine: &Engine, job: &ClaimedJob) -> Result<Handled, DbError> {
    let (Some(guild), Ok(p)) = (job.guild, job.payload::<Payload>()) else {
        return Ok(dead("bad_payload"));
    };
    span("job honeypot_cleanup finalize", guild, async {
        record(engine, job, guild, p.message_id, true).await
    })
    .await
}
