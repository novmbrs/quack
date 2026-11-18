//! The `open_ticket` job (§13.2.3). Safe to repeat: the thread is looked
//! up by its full name before any create and saved at once, adding the
//! member is idempotent, the welcome reuses its nonce inside the window,
//! and every write is a compare-and-set on `opening`.

use quack_core::ids::{AccountId, ChannelId, GuildId, MessageRowId, TicketId};
use quack_core::limits::NONCE_WINDOW;
use quack_core::modlog::{Empty, ModLogEntry, ModLogLinks, ModLogSource};
use quack_core::modules::{TicketState, ticket_thread_name};
use quack_db::jobs::{ClaimedJob, Handled, JobResult, NewJob};
use quack_db::messages::{self, NewTrackedMessage, Owner};
use quack_db::tickets::{self, Ticket};
use quack_db::{DbError, guilds, jobs};
use quack_engine::Engine;
use quack_engine::modlog::{self, By};
use quack_engine::port::{Nonce, Outcome};
use quack_telemetry::metrics;
use quack_text::model::Person;
use time::OffsetDateTime;

use crate::jobs::{Payload, begin, dead, finish, retry};
use quack_engine::span;

/// How one step ended.
enum Step<T> {
    /// It worked.
    Ok(T),
    /// Discord definitely refused (or the thread is gone): the open fails.
    Fail(&'static str),
    /// Try again later.
    Retry(String),
}

fn step<T>(outcome: Outcome<T>) -> Step<T> {
    match outcome {
        Outcome::Done(value) => Step::Ok(value),
        Outcome::Refused(_) => Step::Fail("refused"),
        other => Step::Retry(other.code()),
    }
}

/// Runs the open steps that are not done yet.
pub(crate) async fn run(engine: &Engine, job: &ClaimedJob) -> Result<Handled, DbError> {
    let (Some(guild), Ok(p)) = (job.guild, job.payload::<Payload>()) else {
        return Ok(dead("bad_payload"));
    };
    span("job open_ticket", guild, async {
        let mut conn = engine.db().conn().await?;
        let Some(t) = tickets::get(&mut conn, guild, p.ticket_id).await? else {
            return Ok(dead("ticket_missing"));
        };
        if t.state != TicketState::Opening {
            return Ok(Handled::Settle(JobResult::Done));
        }
        let installed = guilds::get(&mut conn, guild)
            .await?
            .is_some_and(|g| g.installed);
        drop(conn);
        if !installed {
            return fail(engine, job, guild, p.ticket_id, "quack_not_in_server").await;
        }
        match steps(engine, job, &t).await? {
            Step::Ok(()) => opened(engine, job, &t).await,
            Step::Fail(code) => fail(engine, job, guild, t.id, code).await,
            Step::Retry(code) => Ok(retry(job, code)),
        }
    })
    .await
}

/// Steps 2–4: the thread, the member, the welcome.
async fn steps(engine: &Engine, job: &ClaimedJob, t: &Ticket) -> Result<Step<()>, DbError> {
    let discord = engine.discord();
    let thread = match t.thread {
        Some(thread) => thread,
        None => match thread(engine, job, t).await? {
            Step::Ok(thread) => thread,
            Step::Fail(code) => return Ok(Step::Fail(code)),
            Step::Retry(code) => return Ok(Step::Retry(code)),
        },
    };
    match step(discord.add_thread_member(thread, t.member).await) {
        Step::Ok(()) => {}
        Step::Fail(_) => return Ok(Step::Fail("member_not_added")),
        Step::Retry(c) => return Ok(Step::Retry(c)),
    }
    if t.welcome_posted_at.is_some() {
        return Ok(Step::Ok(()));
    }
    let fresh = t
        .welcome_started_at
        .is_some_and(|at| OffsetDateTime::now_utc() - at < NONCE_WINDOW);
    if !fresh {
        let mut tx = begin(engine, job).await?;
        let started = tickets::begin_welcome(&mut tx, t.guild, t.id).await?;
        tx.commit().await?;
        if started.is_none() {
            return Ok(Step::Retry("state_changed".to_owned()));
        }
    }
    let member = Person {
        id: t.member.account(),
        name: None,
    };
    let msg = quack_text::ticket::welcome(&member, t.id);
    let nonce = Nonce::new(format!("{}w", t.id.to_u22()));
    Ok(match step(discord.send(thread, &msg, &[], &nonce).await) {
        Step::Ok(_) => Step::Ok(()),
        Step::Fail(_) => Step::Fail("welcome_refused"),
        Step::Retry(c) => Step::Retry(c),
    })
}

/// Step 2: adopts Quack's own thread by its full name, or creates it, and
/// saves it at once.
async fn thread(engine: &Engine, job: &ClaimedJob, t: &Ticket) -> Result<Step<ChannelId>, DbError> {
    let discord = engine.discord();
    let name = ticket_thread_name(t.id);
    // The schema keeps an opening ticket's entry channel; only imported
    // tickets may lack one, and none of those is opening.
    let Some(entry) = t.entry_channel else {
        return Ok(Step::Fail("entry_unknown"));
    };
    let found = match step(
        discord
            .find_threads(t.guild, entry, &name, t.created_at)
            .await,
    ) {
        Step::Ok(found) => found,
        Step::Fail(_) => return Ok(Step::Fail("threads_unreadable")),
        Step::Retry(c) => return Ok(Step::Retry(c)),
    };
    let thread = if let Some((oldest, duplicates)) = found.split_first() {
        for duplicate in duplicates {
            // Quack's own duplicates (the lookup returns only private
            // threads Quack created with this exact name and parent); a
            // failure leaves one for staff.
            let _ = discord.delete_channel(*duplicate).await;
        }
        *oldest
    } else {
        match step(discord.create_private_thread(entry, &name).await) {
            Step::Ok(thread) => thread,
            Step::Fail(_) => return Ok(Step::Fail("thread_refused")),
            // The next run finds the thread if the create took effect.
            Step::Retry(c) => return Ok(Step::Retry(c)),
        }
    };
    let mut tx = begin(engine, job).await?;
    let saved = tickets::set_thread(&mut tx, t.guild, t.id, thread).await?;
    tx.commit().await?;
    Ok(if saved {
        Step::Ok(thread)
    } else {
        Step::Retry("state_changed".to_owned())
    })
}

/// Step 5: `open`, the staff post, and the log entry, in one transaction.
async fn opened(engine: &Engine, job: &ClaimedJob, t: &Ticket) -> Result<Handled, DbError> {
    let mut tx = begin(engine, job).await?;
    tickets::mark_welcome_posted(&mut tx, t.guild, t.id).await?;
    if !tickets::mark_open(&mut tx, t.guild, t.id).await? {
        return finish(tx, job, JobResult::Done).await;
    }
    let queue = tickets::settings(&mut tx, t.guild)
        .await?
        .and_then(|s| s.queue_channel);
    if let Some(queue) = queue {
        messages::insert(
            &mut tx,
            &NewTrackedMessage {
                id: MessageRowId::from_uuid(uuid::Uuid::now_v7()),
                guild: t.guild,
                owner: Owner::Ticket(t.id),
                channel: Some(queue),
                private: false,
                interaction_token: None,
            },
        )
        .await?;
    }
    modlog::write(
        &mut tx,
        t.guild,
        ModLogEntry::TicketOpened(Empty {}),
        member_by(t.member.account()),
        ModLogLinks {
            member_id: Some(t.member),
            ticket_id: Some(t.id),
            ..ModLogLinks::default()
        },
    )
    .await?;
    let handled = finish(tx, job, JobResult::Done).await?;
    metrics().tickets.opened();
    tracing::info!(ticket_id = %t.id.get(), "ticket open");
    Ok(handled)
}

/// The member opened it from Discord.
const fn member_by(account: AccountId) -> By {
    By {
        actor: Some(account),
        source: ModLogSource::Discord,
    }
}

/// The open failed (§13.2.2): `closed` without a thread, else `closing`
/// so the close job deletes it.
async fn fail(
    engine: &Engine,
    job: &ClaimedJob,
    guild: GuildId,
    ticket: TicketId,
    code: &str,
) -> Result<Handled, DbError> {
    let mut tx = begin(engine, job).await?;
    let state = tickets::fail_open(&mut tx, guild, ticket).await?;
    if state == Some(TicketState::Closing) {
        jobs::enqueue(&mut tx, &NewJob::close_ticket(guild, ticket)).await?;
    }
    let handled = finish(tx, job, JobResult::Done).await?;
    if state == Some(TicketState::Closed) {
        metrics().tickets.closed();
    }
    tracing::warn!(ticket_id = %ticket.get(), code, "ticket open failed");
    Ok(handled)
}

/// Finalization (budget spent, database only): the open fails.
pub(crate) async fn finalize(engine: &Engine, job: &ClaimedJob) -> Result<Handled, DbError> {
    let (Some(guild), Ok(p)) = (job.guild, job.payload::<Payload>()) else {
        return Ok(dead("bad_payload"));
    };
    span("job open_ticket finalize", guild, async {
        fail(engine, job, guild, p.ticket_id, "retries_exhausted").await
    })
    .await
}
