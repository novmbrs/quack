//! Copying evidence files to the evidence channel (§7.3): one file per
//! transaction, so progress survives a crash; each upload follows §10.4
//! with the file's id as its nonce.

use std::time::{Duration, Instant};

use quack_core::evidence::FileCopyState;
use quack_core::ids::ChannelId;
use quack_core::limits::MAX_COPY_BYTES;
use quack_core::permissions::Permission;
use quack_db::evidence::{self, Evidence, EvidenceFile};
use quack_db::jobs::{ClaimedJob, Handled, JobResult, fence};
use quack_db::{DbError, cases, settings};
use quack_text::Msg;

use crate::Engine;
use crate::channels::{ChannelCheck, staff_channel};
use crate::jobs::retry;
use crate::port::{Nonce, Outcome, OutgoingFile};

/// Upload tries per file (§7.3).
const TRIES: u32 = 3;
/// The CDN hosts Quack downloads from.
const HOSTS: [&str; 2] = [
    "https://cdn.discordapp.com/",
    "https://media.discordapp.net/",
];
/// The time a download and an upload may take, plus margin (§16.1).
const CALL_ROOM: Duration = Duration::from_secs(35);

/// How the copy step ended.
pub(super) enum Copied {
    /// Every file left `pending`: files copied and bytes copied.
    All((u64, u64)),
    /// The run ends here (retry, or `Continue` when time is short).
    Stop(Handled),
}

enum One {
    Copied(
        ChannelId,
        quack_core::ids::MessageId,
        quack_core::ids::AttachmentId,
        u64,
    ),
    /// A definite refusal, a non-CDN URL, or three tries spent (§7.3).
    Failed,
}

/// Step 5: copies each pending file.
pub(super) async fn copy_pending(
    engine: &Engine,
    job: &ClaimedJob,
    item: &Evidence,
    started: Instant,
) -> Result<Copied, DbError> {
    let (pending, channel) = {
        let mut conn = engine.db().conn().await?;
        let pending = evidence::pending_files(&mut conn, item.id).await?;
        let channel = settings::get(&mut conn, item.guild)
            .await?
            .and_then(|s| s.evidence_channel);
        (pending, channel)
    };
    if pending.is_empty() {
        return Ok(Copied::All((0, 0)));
    }
    let Some(channel) = channel else {
        fail_all(engine, job, item, false).await?;
        return Ok(Copied::All((0, 0)));
    };
    let needs = Permission::ViewChannel | Permission::SendMessages | Permission::AttachFiles;
    match staff_channel(engine, item.guild, channel, needs).await {
        ChannelCheck::Ok => {}
        ChannelCheck::Problems(_) => {
            fail_all(engine, job, item, true).await?;
            return Ok(Copied::All((0, 0)));
        }
        ChannelCheck::Missing => {
            fail_all(engine, job, item, false).await?;
            return Ok(Copied::All((0, 0)));
        }
        ChannelCheck::Unavailable => {
            return Ok(Copied::Stop(Handled::Settle(retry(
                job,
                "facts_unavailable",
            ))));
        }
    }
    let deadline = job.kind.policy().deadline();
    let (mut files, mut bytes) = (0, 0);
    for file in &pending {
        if started.elapsed() + CALL_ROOM > deadline {
            return Ok(Copied::Stop(Handled::Settle(JobResult::Continue)));
        }
        start(engine, job, item, file).await?;
        let one = copy_one(engine, channel, file).await;
        let mut tx = engine.db().begin().await?;
        cases::lock(&mut tx, item.guild, item.case).await?;
        evidence::lock(&mut tx, item.guild, item.id).await?;
        match one {
            One::Copied(c, m, a, size) => {
                if evidence::mark_copied(&mut tx, file.id, c, m, a).await? {
                    files += 1;
                    bytes += size;
                }
            }
            One::Failed => {
                evidence::set_file_state(&mut tx, file.id, FileCopyState::CopyFailed).await?;
            }
        }
        fence(&mut tx, job).await?;
        tx.commit().await?;
    }
    Ok(Copied::All((files, bytes)))
}

/// Records the upload's start (its nonce window) before any call.
async fn start(
    engine: &Engine,
    job: &ClaimedJob,
    item: &Evidence,
    file: &EvidenceFile,
) -> Result<(), DbError> {
    let mut tx = engine.db().begin().await?;
    cases::lock(&mut tx, item.guild, item.case).await?;
    evidence::lock(&mut tx, item.guild, item.id).await?;
    evidence::start_copy(&mut tx, file.id).await?;
    fence(&mut tx, job).await?;
    tx.commit().await
}

/// Downloads one file from Discord's CDN and uploads it as a plain message
/// with one spoilered attachment, up to three tries.
async fn copy_one(engine: &Engine, channel: ChannelId, file: &EvidenceFile) -> One {
    if !HOSTS.iter().any(|h| file.original_url.starts_with(h)) {
        return One::Failed;
    }
    let discord = engine.discord();
    let nonce = Nonce::new(file.id.to_u22());
    let empty = Msg::default();
    for _ in 0..TRIES {
        let bytes = match discord
            .download_attachment(&file.original_url, MAX_COPY_BYTES)
            .await
        {
            Outcome::Done(bytes) => bytes,
            Outcome::Refused(_) => return One::Failed,
            _ => continue,
        };
        let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        let upload = [OutgoingFile {
            name: format!("SPOILER_{}", file.filename),
            content_type: file.content_type.clone(),
            bytes,
        }];
        match discord.send(channel, &empty, &upload, &nonce).await {
            Outcome::Done(sent) => {
                let Some((_, attachment)) = sent.attachments.first() else {
                    return One::Failed;
                };
                return One::Copied(sent.channel_id, sent.message_id, *attachment, size);
            }
            Outcome::Refused(_) => return One::Failed,
            _ => {}
        }
    }
    One::Failed
}

/// Every pending file of the item → `copy_failed` (`not_staff_only` also
/// marks the item).
async fn fail_all(
    engine: &Engine,
    job: &ClaimedJob,
    item: &Evidence,
    not_staff_only: bool,
) -> Result<(), DbError> {
    let mut tx = engine.db().begin().await?;
    cases::lock(&mut tx, item.guild, item.case).await?;
    evidence::lock(&mut tx, item.guild, item.id).await?;
    evidence::fail_pending_files(&mut tx, item.id, not_staff_only).await?;
    fence(&mut tx, job).await?;
    tx.commit().await
}
