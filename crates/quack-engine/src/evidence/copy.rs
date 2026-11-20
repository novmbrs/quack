//! Copying evidence files to the evidence channel (§7.3): one file per
//! transaction, so progress survives a crash; each upload follows §10.4
//! with the file's id as its nonce. A file's bytes come from Discord's CDN
//! (a message attachment, or an interaction attachment's short-lived URL)
//! or, for a dashboard upload, from the bytes staged in PostgreSQL, which
//! are deleted once the file is copied.

use std::time::Duration;

use quack_core::evidence::FileCopyState;
use quack_core::ids::ChannelId;
use quack_core::limits::MAX_COPY_BYTES;
use quack_db::evidence::{self, Evidence, EvidenceFile};
use quack_db::jobs::{ClaimedJob, Handled, JobResult, fence};
use quack_db::{DbError, cases, settings};
use quack_text::Msg;

use crate::Engine;
use crate::channels::{ChannelCheck, staff_channel};
use crate::jobs::retry;
use crate::port::{Nonce, Outcome, OutgoingFile};

/// Upload tries per file (§7.3), reserved durably before each upload.
const TRIES: u16 = 3;
/// Download tries per file within one run (a download creates nothing).
const DOWNLOAD_TRIES: u32 = 3;
/// The CDN hosts Quack downloads from.
const HOSTS: [&str; 2] = [
    "https://cdn.discordapp.com/",
    "https://media.discordapp.net/",
];
/// The longest a download, a destination check, or an upload may take
/// (the port's deadlines), and the margin left before the claim's own
/// deadline (§16.1).
const DOWNLOAD_ROOM: Duration = Duration::from_secs(30);
const CHECK_ROOM: Duration = Duration::from_secs(10);
const UPLOAD_ROOM: Duration = Duration::from_secs(30);
const MARGIN: Duration = Duration::from_secs(5);

/// How the copy step ended.
pub(super) enum Copied {
    /// Every file left `pending`: files copied and bytes copied.
    All((u64, u64)),
    /// The run ends here (retry, or `Continue` when time is short).
    Stop(Handled),
}

/// How one file's copy ended.
enum One {
    Copied(
        ChannelId,
        quack_core::ids::MessageId,
        quack_core::ids::AttachmentId,
        u64,
    ),
    /// A definite refusal, a non-CDN URL, or three tries spent (§7.3).
    Failed,
    /// The destination is gone or no longer staff-only: stop every file.
    Destination(Destination),
    /// Stop the run here; the file stays `pending` (time, rate limit,
    /// unavailable facts).
    Stop(Handled),
}

/// Where uploads go, checked from current settings and facts right before
/// every upload (§7.3): a channel that stopped being staff-only receives
/// nothing more.
#[derive(Clone, Copy)]
enum Destination {
    Ok(ChannelId),
    /// No evidence channel any more, or it is gone.
    Missing,
    NotStaffOnly,
    Unavailable,
}

async fn destination(engine: &Engine, item: &Evidence) -> Result<Destination, DbError> {
    let channel = {
        let mut conn = engine.db().conn().await?;
        settings::get(&mut conn, item.guild)
            .await?
            .and_then(|s| s.evidence_channel)
    };
    let Some(channel) = channel else {
        return Ok(Destination::Missing);
    };
    Ok(
        match staff_channel(engine, item.guild, channel, super::uploads::DESTINATION).await {
            ChannelCheck::Ok => Destination::Ok(channel),
            ChannelCheck::Problems(_) => Destination::NotStaffOnly,
            ChannelCheck::Missing => Destination::Missing,
            ChannelCheck::Unavailable => Destination::Unavailable,
        },
    )
}

/// Step 6: copies each pending file.
pub(super) async fn copy_pending(
    engine: &Engine,
    job: &ClaimedJob,
    item: &Evidence,
) -> Result<Copied, DbError> {
    let pending = {
        let mut conn = engine.db().conn().await?;
        evidence::pending_files(&mut conn, item.guild, item.id).await?
    };
    if pending.is_empty() {
        return Ok(Copied::All((0, 0)));
    }
    let (mut files, mut bytes) = (0, 0);
    for file in &pending {
        let one = copy_one(engine, job, item, file).await?;
        let state = match one {
            One::Stop(handled) => return Ok(Copied::Stop(handled)),
            One::Destination(d) => return stopped(engine, job, item, d).await,
            One::Copied(c, m, a, size) => Ok((c, m, a, size)),
            One::Failed => Err(FileCopyState::CopyFailed),
        };
        let mut tx = engine.db().begin().await?;
        cases::lock(&mut tx, item.guild, item.case).await?;
        evidence::lock(&mut tx, item.guild, item.id).await?;
        match state {
            Ok((c, m, a, size)) => {
                if evidence::mark_copied(&mut tx, item.guild, file.id, c, m, a).await? {
                    files += 1;
                    bytes += size;
                }
                evidence::delete_staged(&mut tx, item.guild, item.id, Some(file.id)).await?;
            }
            Err(failed) => {
                evidence::set_file_state(&mut tx, item.guild, file.id, failed).await?;
            }
        }
        fence(&mut tx, job).await?;
        tx.commit().await?;
    }
    Ok(Copied::All((files, bytes)))
}

/// The destination stopped the copy: every pending file fails (marking
/// the item when the channel isn't staff-only), or the run retries when
/// the facts were unavailable.
async fn stopped(
    engine: &Engine,
    job: &ClaimedJob,
    item: &Evidence,
    d: Destination,
) -> Result<Copied, DbError> {
    match d {
        Destination::Unavailable | Destination::Ok(_) => Ok(Copied::Stop(Handled::Settle(retry(
            job,
            "facts_unavailable",
        )))),
        Destination::Missing | Destination::NotStaffOnly => {
            fail_all(engine, job, item, matches!(d, Destination::NotStaffOnly)).await?;
            Ok(Copied::All((0, 0)))
        }
    }
}

/// Reserves an upload try (and its nonce window) under the case and item
/// locks, with the job's fence. `None`: the file's budget is spent.
async fn reserve(
    engine: &Engine,
    job: &ClaimedJob,
    item: &Evidence,
    file: &EvidenceFile,
) -> Result<Option<u16>, DbError> {
    let mut tx = engine.db().begin().await?;
    cases::lock(&mut tx, item.guild, item.case).await?;
    evidence::lock(&mut tx, item.guild, item.id).await?;
    let reserved = evidence::reserve_upload(&mut tx, item.guild, file.id, TRIES).await?;
    fence(&mut tx, job).await?;
    tx.commit().await?;
    Ok(reserved)
}

/// Gives back a reserved try that never reached Discord, in the same lock
/// order as the reservation and with the job's fence, so a worker that
/// lost its lease can't give back a newer worker's try.
async fn release(
    engine: &Engine,
    job: &ClaimedJob,
    item: &Evidence,
    file: &EvidenceFile,
) -> Result<(), DbError> {
    let mut tx = engine.db().begin().await?;
    cases::lock(&mut tx, item.guild, item.case).await?;
    evidence::lock(&mut tx, item.guild, item.id).await?;
    evidence::release_upload(&mut tx, item.guild, file.id).await?;
    fence(&mut tx, job).await?;
    tx.commit().await
}

/// Where a file's bytes come from.
enum Bytes<'a> {
    /// Download from this CDN URL.
    Cdn(&'a str),
    /// Staged in PostgreSQL (a dashboard upload).
    Staged,
}

/// A file's bytes for one upload try: downloaded (counted against the
/// run's download tries) or read from staging.
enum Got {
    Bytes(Vec<u8>),
    /// Try the download again.
    Again,
    /// The run ends here.
    Stop(One),
}

async fn get_bytes(
    engine: &Engine,
    job: &ClaimedJob,
    item: &Evidence,
    file: &EvidenceFile,
    source: &Bytes<'_>,
) -> Result<Got, DbError> {
    match source {
        Bytes::Staged => {
            let mut conn = engine.db().conn().await?;
            Ok(
                match evidence::staged_bytes(&mut conn, item.guild, file.id).await? {
                    Some(bytes) => Got::Bytes(bytes),
                    // Swept, or never staged: nothing left to copy.
                    None => Got::Stop(One::Failed),
                },
            )
        }
        Bytes::Cdn(url) => Ok(
            match engine
                .discord()
                .download_attachment(url, MAX_COPY_BYTES)
                .await
            {
                Outcome::Done(bytes) => Got::Bytes(bytes),
                Outcome::Refused(_) => Got::Stop(One::Failed),
                Outcome::RetryAfter(d) => Got::Stop(One::Stop(Handled::Settle(retry_in(job, d)))),
                _ => Got::Again,
            },
        ),
    }
}

/// Gets one file's bytes and uploads them as a plain message with one
/// spoilered attachment. Every upload is preceded by a fresh destination
/// check and a durably reserved try (three per file); a rate limit stops
/// the run with its delay; a run never starts a call it has no time to
/// finish. A run that ends because a download failed costs the job a
/// counted attempt, so a download that keeps failing settles the file
/// through the job's budget. Only Discord's CDN hosts are downloaded from.
async fn copy_one(
    engine: &Engine,
    job: &ClaimedJob,
    item: &Evidence,
    file: &EvidenceFile,
) -> Result<One, DbError> {
    let source = match file.original_url.as_deref() {
        Some(url) if HOSTS.iter().any(|h| url.starts_with(h)) => Bytes::Cdn(url),
        Some(_) => return Ok(One::Failed),
        None => Bytes::Staged,
    };
    let discord = engine.discord();
    let nonce = Nonce::new(file.id.to_u22());
    let empty = Msg::default();
    // Out of time: start again in a new run, which is free after progress
    // and counted after a failed download (else it never runs out).
    let out_of_time = |download_failed: bool| {
        One::Stop(Handled::Settle(if download_failed {
            retry(job, "download_failed")
        } else {
            JobResult::Continue
        }))
    };
    let mut downloads = 0;
    let mut download_failed = false;
    loop {
        if job.time_left() < DOWNLOAD_ROOM + CHECK_ROOM + UPLOAD_ROOM + MARGIN {
            return Ok(out_of_time(download_failed));
        }
        if downloads == DOWNLOAD_TRIES {
            return Ok(One::Failed);
        }
        downloads += 1;
        let bytes = match get_bytes(engine, job, item, file, &source).await? {
            Got::Bytes(bytes) => bytes,
            Got::Stop(one) => return Ok(one),
            Got::Again => {
                download_failed = true;
                continue;
            }
        };
        download_failed = false;
        if job.time_left() < CHECK_ROOM + UPLOAD_ROOM + MARGIN {
            return Ok(out_of_time(false));
        }
        let channel = match destination(engine, item).await? {
            Destination::Ok(channel) => channel,
            other => return Ok(One::Destination(other)),
        };
        if reserve(engine, job, item, file).await?.is_none() {
            return Ok(One::Failed);
        }
        let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        let upload = [OutgoingFile {
            name: format!("SPOILER_{}", file.filename),
            content_type: file.content_type.clone(),
            bytes,
        }];
        match discord.send(channel, &empty, &upload, &nonce).await {
            Outcome::Done(sent) => {
                let Some((_, attachment)) = sent.attachments.first() else {
                    return Ok(One::Failed);
                };
                return Ok(One::Copied(
                    sent.channel_id,
                    sent.message_id,
                    *attachment,
                    size,
                ));
            }
            Outcome::Refused(_) => return Ok(One::Failed),
            Outcome::RetryAfter(d) => {
                release(engine, job, item, file).await?;
                return Ok(One::Stop(Handled::Settle(retry_in(job, d))));
            }
            Outcome::NotSent(_) => release(engine, job, item, file).await?,
            // May have created the copy: the try stays counted, and the
            // next one reuses the nonce while the window lasts.
            Outcome::Unknown(_) => {}
        }
    }
}

/// Retry after Discord's requested wait, or the kind's backoff when that
/// is longer.
fn retry_in(job: &ClaimedJob, d: Duration) -> JobResult {
    JobResult::Retry {
        at: quack_db::jobs::RunAt::In(d.max(job.kind.policy().backoff(job.attempt))),
        error: "rate_limited".to_owned(),
    }
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
    evidence::fail_pending_files(&mut tx, item.guild, item.id, not_staff_only).await?;
    fence(&mut tx, job).await?;
    tx.commit().await
}
