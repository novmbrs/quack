//! The `capture_evidence` handler (§7.2): check the adder's access, fetch
//! the message, store the snapshot once under the case lock, copy files
//! one per transaction, and settle.

use quack_core::evidence::{
    EvidenceFailure, EvidenceState, FileCopyState, copy_eligibility, files_within_caps,
};
use quack_core::ids::FileId;
use quack_core::limits::{
    EVIDENCE_EMBED_DESCRIPTION_CHARS, EVIDENCE_EMBED_TITLE_CHARS, EVIDENCE_TEXT_CHARS,
    MAX_EVIDENCE_EMBEDS, truncate_chars,
};
use quack_db::evidence::{self, EmbedSummary, Evidence, NewFile, Snapshot};
use quack_db::jobs::{ClaimedJob, Handled, JobResult, fence, settle as settle_job};
use quack_db::messages::{self, Owner};
use quack_db::{DbError, cases, settings};
use quack_telemetry::labels::Outcome as Label;

use super::access::{Unavailable, can_read};
use super::copy::{Copied, copy_pending};
use super::{Payload, captured, fail};
use crate::jobs::{dead, retry};
use crate::port::{FetchedMessage, Outcome, RefusalKind};
use crate::{Engine, obs};

/// Captures one evidence item (§7.2). Safe to repeat: the snapshot is
/// stored once under the case lock, and each file has its own state and
/// nonce.
pub async fn capture(engine: &Engine, job: &ClaimedJob) -> Result<Handled, DbError> {
    let (Some(guild), Ok(payload)) = (job.guild, job.payload::<Payload>()) else {
        return Ok(dead("bad_payload"));
    };
    obs::use_case("capture_evidence", Some(guild), async {
        let item = {
            let mut conn = engine.db().conn().await?;
            evidence::get(&mut conn, guild, payload.evidence_id).await?
        };
        let Some(item) = item else {
            return Ok(dead("evidence_missing"));
        };
        if item.state != EvidenceState::Saving {
            return Ok(Handled::Settle(JobResult::Done));
        }
        if !item.snapshot_stored()
            && let Some(handled) = snapshot(engine, job, &item).await?
        {
            return Ok(handled);
        }
        let copied = match copy_pending(engine, job, &item).await? {
            Copied::All(copied) => copied,
            Copied::Stop(handled) => return Ok(handled),
        };
        settle_saved(engine, job, &item, copied).await
    })
    .await
}

/// Steps 2–4: access check, fetch, and the snapshot. `Some` when the run
/// ends here.
async fn snapshot(
    engine: &Engine,
    job: &ClaimedJob,
    item: &Evidence,
) -> Result<Option<Handled>, DbError> {
    if let Some(staff) = item.added_by {
        match can_read(engine, item.guild, item.channel, staff).await {
            Ok(true) => {}
            Ok(false) => {
                return fail(engine, job, item, EvidenceFailure::ModeratorNoAccess)
                    .await
                    .map(Some);
            }
            Err(Unavailable) => return Ok(Some(Handled::Settle(retry(job, "facts_unavailable")))),
        }
    }
    let message = match engine
        .discord()
        .fetch_message(item.channel, item.message)
        .await
    {
        Outcome::Done(m) => m,
        Outcome::Refused(r) => {
            let failure = match r.kind {
                RefusalKind::UnknownMessage | RefusalKind::UnknownChannel => {
                    EvidenceFailure::MessageDeleted
                }
                RefusalKind::MissingAccess | RefusalKind::MissingPermissions => {
                    EvidenceFailure::QuackNoAccess
                }
                _ => EvidenceFailure::DiscordError,
            };
            return fail(engine, job, item, failure).await.map(Some);
        }
        other => return Ok(Some(Handled::Settle(retry(job, other.code())))),
    };
    if message.guild_id != Some(item.guild) {
        return fail(engine, job, item, EvidenceFailure::OtherServer)
            .await
            .map(Some);
    }
    save(engine, job, item, &message).await?;
    Ok(None)
}

/// Step 4: one transaction, the case then the item; files admitted under
/// the per-message and per-case caps, each with its first copy state.
async fn save(
    engine: &Engine,
    job: &ClaimedJob,
    item: &Evidence,
    message: &FetchedMessage,
) -> Result<(), DbError> {
    let mut tx = engine.db().begin().await?;
    cases::lock(&mut tx, item.guild, item.case).await?;
    evidence::lock(&mut tx, item.guild, item.id).await?;
    let channel = settings::get(&mut tx, item.guild)
        .await?
        .and_then(|s| s.evidence_channel);
    let case_files = evidence::count_files_for_case(&mut tx, item.guild, item.case).await?;
    let total = u32::try_from(message.attachments.len()).unwrap_or(u32::MAX);
    let admitted = files_within_caps(case_files, total);
    let files: Vec<NewFile> = message
        .attachments
        .iter()
        .take(admitted as usize)
        .enumerate()
        .map(|(i, a)| NewFile {
            id: FileId::from_uuid(uuid::Uuid::now_v7()),
            position: u8::try_from(i + 1).unwrap_or(u8::MAX),
            attachment_id: Some(a.id),
            filename: truncate_chars(&a.filename, 255).to_owned(),
            content_type: a.content_type.clone(),
            size_bytes: a.size_bytes,
            original_url: a.url.clone(),
            copy_state: match channel {
                None => FileCopyState::LinkOnly,
                Some(_) => match copy_eligibility(a.size_bytes, a.content_type.as_deref()) {
                    Ok(()) => FileCopyState::Pending,
                    Err(state) => state,
                },
            },
        })
        .collect();
    let snapshot = Snapshot {
        author: Some(message.author_id),
        content: (!message.content.is_empty())
            .then(|| truncate_chars(&message.content, EVIDENCE_TEXT_CHARS).to_owned()),
        created_at: message.created_at,
        edited_at: message.edited_at,
        embeds: message
            .embeds
            .iter()
            .take(MAX_EVIDENCE_EMBEDS)
            .map(|e| EmbedSummary {
                kind: e.kind.clone(),
                title: e
                    .title
                    .as_deref()
                    .map(|t| truncate_chars(t, EVIDENCE_EMBED_TITLE_CHARS).to_owned()),
                description: e
                    .description
                    .as_deref()
                    .map(|d| truncate_chars(d, EVIDENCE_EMBED_DESCRIPTION_CHARS).to_owned()),
                url: e.url.clone(),
            })
            .collect(),
        files_not_saved: total - admitted,
    };
    evidence::save_snapshot(&mut tx, item.guild, item.id, &snapshot, &files).await?;
    fence(&mut tx, job).await?;
    tx.commit().await
}

/// Step 6: every file left `pending`: the item is `saved`.
async fn settle_saved(
    engine: &Engine,
    job: &ClaimedJob,
    item: &Evidence,
    copied: (u64, u64),
) -> Result<Handled, DbError> {
    let mut tx = engine.db().begin().await?;
    cases::lock(&mut tx, item.guild, item.case).await?;
    evidence::lock(&mut tx, item.guild, item.id).await?;
    evidence::mark_saved(&mut tx, item.guild, item.id).await?;
    messages::bump(&mut tx, item.guild, Owner::Case(item.case)).await?;
    settle_job(&mut tx, job, &JobResult::Done).await?;
    tx.commit().await?;
    captured(item, Label::Done, copied.0, copied.1, "saved");
    Ok(Handled::Settled(JobResult::Done))
}

/// Settles a `capture_evidence` job whose budget is spent, without calling
/// Discord: pending files → `copy_failed`; the item → `saved` when its
/// snapshot was stored, else `failed` (`discord_error`).
pub async fn finalize(engine: &Engine, job: &ClaimedJob) -> Result<Handled, DbError> {
    let (Some(guild), Ok(payload)) = (job.guild, job.payload::<Payload>()) else {
        return Ok(dead("bad_payload"));
    };
    let mut tx = engine.db().begin().await?;
    let Some(item) = evidence::get(&mut tx, guild, payload.evidence_id).await? else {
        return Ok(dead("evidence_missing"));
    };
    cases::lock(&mut tx, guild, item.case).await?;
    let Some(item) = evidence::lock(&mut tx, guild, item.id).await? else {
        return Ok(dead("evidence_missing"));
    };
    if item.state == EvidenceState::Saving {
        evidence::fail_pending_files(&mut tx, guild, item.id, false).await?;
        if item.snapshot_stored() {
            evidence::mark_saved(&mut tx, guild, item.id).await?;
        } else {
            evidence::mark_failed(&mut tx, guild, item.id, EvidenceFailure::DiscordError).await?;
        }
        messages::bump(&mut tx, guild, Owner::Case(item.case)).await?;
    }
    settle_job(&mut tx, job, &JobResult::Done).await?;
    tx.commit().await?;
    Ok(Handled::Settled(JobResult::Done))
}
