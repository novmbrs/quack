//! The `close_ticket` job (§13.2.4). Each step records its completion
//! column, so a retry resumes where the last run stopped; the budget's
//! finalization always reaches `closed`, so the member's slot is freed.

use quack_core::ids::{DmId, GuildId};
use quack_core::modlog::{ModLogEntry, ModLogLinks, TicketClosedDetail};
use quack_core::modules::{TicketCloseReason, TicketState};
use quack_db::dms::{self, DmOwner};
use quack_db::jobs::{self, ClaimedJob, Handled, JobResult, NewJob};
use quack_db::messages::{self, Owner};
use quack_db::tickets::{self, Ticket};
use quack_db::{DbError, Tx, guilds};
use quack_engine::Engine;
use quack_engine::modlog::{self, By};
use quack_engine::port::{Fresh, Membership, Outcome, RefusalKind};
use quack_telemetry::metrics;
use time::OffsetDateTime;

use crate::jobs::{Payload, begin, dead, finish, retry};
use crate::transcript::{self, HISTORY_PAGE, HISTORY_PAGES, Header, History};
use quack_engine::span;

/// Runs the close steps that are not done yet.
pub(crate) async fn run(engine: &Engine, job: &ClaimedJob) -> Result<Handled, DbError> {
    let (Some(guild), Ok(p)) = (job.guild, job.payload::<Payload>()) else {
        return Ok(dead("bad_payload"));
    };
    span("job close_ticket", guild, async {
        let Some(mut t) = load(engine, guild, p).await? else {
            return Ok(dead("ticket_missing"));
        };
        if matches!(t.state, TicketState::Opening | TicketState::Open) {
            return Ok(Handled::Settle(JobResult::Done));
        }
        if t.state == TicketState::Closing {
            // 1. Freeze.
            if t.frozen_at.is_none() {
                if let Some(code) = freeze(engine, &t).await {
                    return Ok(retry(job, code));
                }
                let mut tx = begin(engine, job).await?;
                tickets::mark_frozen(&mut tx, guild, t.id).await?;
                tx.commit().await?;
            }
            // 2. Transcript.
            if t.transcript_saved_at.is_none() && !open_failed(&t) {
                let history = match history(engine, &t).await {
                    Ok(history) => history,
                    Err(code) => return Ok(retry(job, code)),
                };
                let name = member_name(engine, &t).await;
                save(engine, job, &mut t, history.as_ref(), name, false).await?;
            }
            // 3. Close.
            let mut tx = begin(engine, job).await?;
            close(&mut tx, &t).await?;
            tx.commit().await?;
            t = load(engine, guild, p).await?.ok_or(DbError::NotFound)?;
        }
        // 4. Delete the thread.
        delete_thread(engine, job, &t).await
    })
    .await
}

async fn load(engine: &Engine, guild: GuildId, p: Payload) -> Result<Option<Ticket>, DbError> {
    let mut conn = engine.db().conn().await?;
    tickets::get(&mut conn, guild, p.ticket_id).await
}

fn open_failed(t: &Ticket) -> bool {
    t.close_reason == Some(TicketCloseReason::OpenFailed)
}

/// Step 1: archive and lock the thread so members can no longer post.
/// `None` when done (a gone thread counts as frozen); else the code to
/// retry with. Any other refusal is never taken as frozen: the job
/// retries, and its finalization keeps the thread and records the close
/// as partial.
async fn freeze(engine: &Engine, t: &Ticket) -> Option<String> {
    let thread = t.thread?;
    let discord = engine.discord();
    match discord.set_thread_state(thread, true, true).await {
        Outcome::Done(()) => None,
        Outcome::Refused(r) if r.kind == RefusalKind::UnknownChannel => None,
        // An archived thread takes no other change until it is unarchived
        // (Discord answers 400): unarchive and lock it in one request, then
        // archive it.
        Outcome::Refused(r) if r.kind == RefusalKind::Other => {
            match discord.set_thread_state(thread, false, true).await {
                Outcome::Done(()) => match discord.set_thread_state(thread, true, true).await {
                    Outcome::Done(()) => None,
                    other => Some(other.code()),
                },
                other => Some(other.code()),
            }
        }
        other => Some(other.code()),
    }
}

/// The thread's history, newest first, at most 10 pages of 100, returned
/// oldest first. `Ok(None)` when Discord refuses (the thread is gone or
/// unreadable); `Err` to retry.
async fn history(engine: &Engine, t: &Ticket) -> Result<Option<History>, String> {
    let Some(thread) = t.thread else {
        return Ok(None);
    };
    let mut out = History::default();
    let mut before = None;
    for _ in 0..HISTORY_PAGES {
        let page = match engine
            .discord()
            .fetch_history(thread, before, HISTORY_PAGE)
            .await
        {
            Outcome::Done(page) => page,
            Outcome::Refused(_) => return Ok(None),
            other => return Err(other.code()),
        };
        let short = page.len() < usize::from(HISTORY_PAGE);
        before = page.last().map(|m| m.id);
        out.messages.extend(page);
        if short {
            out.reached_first = true;
            break;
        }
    }
    out.messages.sort_by_key(|m| m.id);
    Ok(Some(out))
}

/// The member's display name for the transcript header, when Discord says.
async fn member_name(engine: &Engine, t: &Ticket) -> Option<String> {
    match engine
        .discord()
        .member(t.guild, t.member, Fresh::Cached)
        .await
    {
        Outcome::Done(Membership::Present(m)) => Some(m.display_name().to_owned()),
        _ => None,
    }
}

/// Builds and saves the transcript once (it also records
/// `transcript_saved_at`). A transcript is only saved for a frozen ticket.
/// `history_unavailable` marks it partial whatever the journal says
/// (finalization never read the thread).
async fn save(
    engine: &Engine,
    job: &ClaimedJob,
    t: &mut Ticket,
    history: Option<&History>,
    member_name: Option<String>,
    history_unavailable: bool,
) -> Result<(), DbError> {
    let mut tx = begin(engine, job).await?;
    // Re-read inside the fence: the freeze may have just been recorded.
    *t = tickets::lock(&mut tx, t.guild, t.id)
        .await?
        .ok_or(DbError::NotFound)?;
    if t.transcript_saved_at.is_some() || t.state != TicketState::Closing {
        return tx.commit().await;
    }
    t.journal_gaps |= history_unavailable;
    let server = guilds::get(&mut tx, t.guild)
        .await?
        .map_or_default(|g| g.name);
    let built = transcript::build(
        &mut tx,
        t,
        Header {
            server: &server,
            member_name,
            closed: OffsetDateTime::now_utc(),
        },
        history,
    )
    .await?;
    let retention = retention_days(&mut tx, t.guild).await?;
    let saved = tickets::save_transcript(
        &mut tx,
        t.guild,
        t.id,
        &built.content,
        built.partial,
        retention,
    )
    .await?;
    tx.commit().await?;
    if saved {
        let bytes = u64::try_from(built.content.len()).unwrap_or(u64::MAX);
        metrics().tickets.transcript_saved(bytes);
        tracing::info!(ticket_id = %t.id.get(), bytes, partial = ?built.partial, "transcript saved");
    }
    Ok(())
}

async fn retention_days(conn: &mut quack_db::PgConnection, guild: GuildId) -> Result<u16, DbError> {
    Ok(tickets::settings(conn, guild)
        .await?
        .map_or(quack_core::limits::DEFAULT_TICKET_RETENTION_DAYS, |s| {
            s.transcript_retention_days
        }))
}

/// Step 3, in the caller's transaction: `closed`, the member's DM with the
/// transcript, the staff post bumped, and the log entry. The slot is free
/// from here.
async fn close(tx: &mut Tx, t: &Ticket) -> Result<(), DbError> {
    let retention = retention_days(tx, t.guild).await?;
    if !tickets::mark_closed(tx, t.guild, t.id, retention).await? {
        return Ok(());
    }
    if !open_failed(t) {
        let dm = DmId::from_uuid(uuid::Uuid::now_v7());
        dms::insert(tx, dm, t.guild, t.member, DmOwner::TicketClosed(t.id)).await?;
        jobs::enqueue(tx, &NewJob::send_dm(t.guild, dm)).await?;
    }
    messages::bump(tx, t.guild, Owner::Ticket(t.id)).await?;
    let complete = tickets::transcript(tx, t.guild, t.id)
        .await?
        .is_some_and(|tr| tr.partial.is_none());
    modlog::write(
        tx,
        t.guild,
        ModLogEntry::TicketClosed(TicketClosedDetail {
            transcript_complete: complete,
        }),
        // Written by the close job: the actor is whoever asked to close
        // (`None` when Quack closed it because its thread was deleted).
        By {
            actor: t.closed_by,
            source: quack_core::modlog::ModLogSource::Quack,
        },
        ModLogLinks {
            member_id: Some(t.member),
            ticket_id: Some(t.id),
            ..ModLogLinks::default()
        },
    )
    .await?;
    metrics().tickets.closed();
    tracing::info!(ticket_id = %t.id.get(), "ticket closed");
    Ok(())
}

/// Step 4: delete the thread; 404 is done, any other refusal is recorded
/// on the ticket (its staff post says so).
async fn delete_thread(engine: &Engine, job: &ClaimedJob, t: &Ticket) -> Result<Handled, DbError> {
    let pending =
        t.state == TicketState::Closed && t.thread_deleted_at.is_none() && !t.thread_delete_failed;
    let Some(thread) = t.thread.filter(|_| pending) else {
        return Ok(Handled::Settle(JobResult::Done));
    };
    let failed = match engine.discord().delete_channel(thread).await {
        Outcome::Done(()) => false,
        Outcome::Refused(r) if r.kind == RefusalKind::UnknownChannel => false,
        Outcome::Refused(_) => true,
        other => return Ok(retry(job, other.code())),
    };
    let mut tx = begin(engine, job).await?;
    tickets::mark_thread_deleted(&mut tx, t.guild, t.id, failed).await?;
    if failed {
        messages::bump(&mut tx, t.guild, Owner::Ticket(t.id)).await?;
    }
    finish(tx, job, JobResult::Done).await
}

/// Finalization (budget spent, database only): a ticket not yet `closed`
/// freezes its journal, saves a journal-only transcript (partial, history
/// unavailable) if none was saved, and closes; a thread not yet deleted is
/// recorded as not deleted. No ticket stays `closing`.
pub(crate) async fn finalize(engine: &Engine, job: &ClaimedJob) -> Result<Handled, DbError> {
    let (Some(guild), Ok(p)) = (job.guild, job.payload::<Payload>()) else {
        return Ok(dead("bad_payload"));
    };
    span("job close_ticket finalize", guild, async {
        let Some(mut t) = load(engine, guild, p).await? else {
            return Ok(dead("ticket_missing"));
        };
        if t.state == TicketState::Closing {
            let mut tx = begin(engine, job).await?;
            tickets::mark_frozen(&mut tx, guild, t.id).await?;
            tx.commit().await?;
            if !open_failed(&t) {
                save(engine, job, &mut t, None, None, true).await?;
            }
            let mut tx = begin(engine, job).await?;
            close(&mut tx, &t).await?;
            tx.commit().await?;
            t = load(engine, guild, p).await?.ok_or(DbError::NotFound)?;
        }
        let mut tx = begin(engine, job).await?;
        if t.thread.is_some() && t.thread_deleted_at.is_none() && !t.thread_delete_failed {
            tickets::mark_thread_deleted(&mut tx, guild, t.id, true).await?;
            messages::bump(&mut tx, guild, Owner::Ticket(t.id)).await?;
        }
        finish(tx, job, JobResult::Done).await
    })
    .await
}
