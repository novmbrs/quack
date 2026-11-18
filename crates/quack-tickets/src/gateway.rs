//! Gateway events the tickets module handles (§11.9): the journal from
//! `MESSAGE_CREATE`, a deleted thread, a missed deletion found on
//! `GUILD_CREATE`, and possibly missed events after an invalid session.

use std::collections::BTreeSet;
use std::sync::Mutex;
use std::time::Duration;

use quack_core::ids::{AccountId, ChannelId, GuildId, MessageId};
use quack_core::modules::TicketState;
use quack_db::jobs::{self, NewJob};
use quack_db::messages::{self, Owner};
use quack_db::tickets::{self, JournalFile, JournalMessage, Ticket};
use quack_db::{DbError, TimeCursor};
use quack_engine::Engine;
use quack_engine::port::{Outcome, RefusalKind};
use time::OffsetDateTime;

use crate::obs::span;

/// The Discord dispatch events the tickets module wants: `MESSAGE_CREATE`
/// (the journal), `THREAD_DELETE`, `GUILD_CREATE` (missed thread
/// deletions), and `READY` (after an invalid session). Intents: `GUILDS`,
/// `GUILD_MESSAGES`, `MESSAGE_CONTENT`.
pub const GATEWAY_EVENTS: &[&str] = &["MESSAGE_CREATE", "THREAD_DELETE", "GUILD_CREATE", "READY"];

/// Tries of one journal insert before the gap is recorded.
const JOURNAL_TRIES: u32 = 3;
/// Threads whose gap couldn't be recorded yet, at most.
const PENDING_GAPS: usize = 10_000;

/// A message created in a channel, as the journal takes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalEntry {
    /// The server.
    pub guild: GuildId,
    /// The channel or thread it was posted in.
    pub channel: ChannelId,
    /// The channel's parent when `channel` is a thread; `None` for a
    /// channel (the journal ignores it without a query).
    pub parent: Option<ChannelId>,
    /// The message.
    pub message: MessageId,
    /// Its author.
    pub author: AccountId,
    /// The author's display name.
    pub author_name: String,
    /// Its text.
    pub text: String,
    /// Its files.
    pub files: Vec<JournalFileEntry>,
    /// When it was sent.
    pub sent_at: OffsetDateTime,
}

/// One file of a journaled message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalFileEntry {
    /// The file name.
    pub name: String,
    /// Its size in bytes.
    pub size: u64,
}

/// The ticket journal's gateway side. One per process: it remembers the
/// threads whose gap it couldn't record while the database refused
/// writes, and records them once it accepts writes again.
#[derive(Debug, Default)]
pub struct Journal {
    pending: Mutex<BTreeSet<(GuildId, ChannelId)>>,
}

impl Journal {
    /// A journal with nothing pending.
    pub fn new() -> Self {
        Self::default()
    }

    /// `MESSAGE_CREATE`: journals a message posted in the thread of a live,
    /// not yet frozen ticket (`ON CONFLICT DO NOTHING`, so a repeated event
    /// is harmless). An insert that fails three times marks the ticket's
    /// journal as gapped.
    pub async fn on_message(&self, engine: &Engine, entry: &JournalEntry) {
        if entry.parent.is_none() {
            return;
        }
        span("ticket_journal", entry.guild, async {
            for attempt in 1..=JOURNAL_TRIES {
                match journal(engine, entry).await {
                    Ok(()) => {
                        self.flush(engine).await;
                        return;
                    }
                    Err(error) if attempt == JOURNAL_TRIES => {
                        tracing::warn!(%error, channel_id = entry.channel.get(), "journal insert failed");
                    }
                    Err(_) => tokio::time::sleep(Duration::from_millis(50 * u64::from(attempt))).await,
                }
            }
            self.gap(engine, entry.guild, entry.channel).await;
        })
        .await;
    }

    /// The bot dropped a gateway event for `channel` (a full queue): if it
    /// is a ticket thread, its journal may be missing a message.
    pub async fn on_event_dropped(&self, engine: &Engine, guild: GuildId, channel: ChannelId) {
        self.gap(engine, guild, channel).await;
    }

    /// Threads whose gap is still to be recorded.
    pub fn pending(&self) -> usize {
        self.pending.lock().map_or(0, |p| p.len())
    }

    async fn gap(&self, engine: &Engine, guild: GuildId, thread: ChannelId) {
        if mark_gap(engine, guild, thread).await.is_err()
            && let Ok(mut pending) = self.pending.lock()
            && pending.len() < PENDING_GAPS
        {
            pending.insert((guild, thread));
        }
    }

    /// Records the gaps remembered while the database refused writes.
    async fn flush(&self, engine: &Engine) {
        let waiting: Vec<_> = match self.pending.lock() {
            Ok(mut p) if !p.is_empty() => std::mem::take(&mut *p).into_iter().collect(),
            _ => return,
        };
        for (guild, thread) in waiting {
            if mark_gap(engine, guild, thread).await.is_err()
                && let Ok(mut pending) = self.pending.lock()
            {
                pending.insert((guild, thread));
            }
        }
    }
}

/// The live ticket of this thread in this server.
async fn ticket_of(
    engine: &Engine,
    guild: GuildId,
    thread: ChannelId,
) -> Result<Option<Ticket>, DbError> {
    let mut conn = engine.db().conn().await?;
    Ok(tickets::by_thread(&mut conn, thread)
        .await?
        .filter(|t| t.guild == guild && t.state.is_live()))
}

async fn journal(engine: &Engine, e: &JournalEntry) -> Result<(), DbError> {
    let Some(t) = ticket_of(engine, e.guild, e.channel).await? else {
        return Ok(());
    };
    let message = JournalMessage {
        message: e.message,
        author: e.author,
        author_name: if e.author_name.is_empty() {
            e.author.get().to_string()
        } else {
            e.author_name.clone()
        },
        text: e.text.clone(),
        files: e
            .files
            .iter()
            .map(|f| JournalFile {
                name: f.name.clone(),
                size: f.size,
            })
            .collect(),
        sent_at: e.sent_at,
    };
    let mut conn = engine.db().conn().await?;
    tickets::add_journal_message(&mut conn, e.guild, t.id, &message).await?;
    Ok(())
}

async fn mark_gap(engine: &Engine, guild: GuildId, thread: ChannelId) -> Result<(), DbError> {
    let Some(t) = ticket_of(engine, guild, thread).await? else {
        return Ok(());
    };
    let mut conn = engine.db().conn().await?;
    tickets::mark_journal_gaps(&mut conn, guild, t.id).await?;
    Ok(())
}

/// `READY` after an invalid session: events may have been missed, so every
/// live ticket whose journal is still open is marked gapped.
pub async fn on_invalid_session(engine: &Engine) -> Result<u64, DbError> {
    let mut conn = engine.db().conn().await?;
    tickets::mark_live_journal_gaps(&mut conn).await
}

/// `THREAD_DELETE`: a ticket's thread was deleted. An `open` ticket closes
/// (Quack closes it); an `opening` one fails to open. Either way its close
/// job runs (and finds the thread gone). Returns whether a ticket changed.
pub async fn on_thread_deleted(
    engine: &Engine,
    guild: GuildId,
    thread: ChannelId,
) -> Result<bool, DbError> {
    span("ticket_thread_deleted", guild, async {
        let Some(t) = ticket_of(engine, guild, thread).await? else {
            return Ok(false);
        };
        let mut tx = engine.db().begin().await?;
        let closing = match t.state {
            TicketState::Open => tickets::request_close(&mut tx, guild, t.id, None).await?,
            TicketState::Opening => {
                tickets::fail_open(&mut tx, guild, t.id).await? == Some(TicketState::Closing)
            }
            TicketState::Closing | TicketState::Closed => false,
        };
        if closing {
            jobs::enqueue(&mut tx, &NewJob::close_ticket(guild, t.id)).await?;
            messages::bump(&mut tx, guild, Owner::Ticket(t.id)).await?;
        }
        tx.commit().await?;
        if closing {
            tracing::info!(ticket_id = %t.id.get(), "ticket thread deleted; closing");
        }
        Ok(closing)
    })
    .await
}

/// `GUILD_CREATE`: an `opening` or `open` ticket whose thread is not among
/// the event's active threads, and that Discord confirms gone (404), was
/// deleted while Quack was away (§13.1.5). Returns how many tickets closed.
pub async fn on_guild_create(
    engine: &Engine,
    guild: GuildId,
    active_threads: &[ChannelId],
) -> Result<u64, DbError> {
    span("ticket_reconcile", guild, async {
        let mut closed = 0;
        for state in [TicketState::Opening, TicketState::Open] {
            let mut cursor: Option<TimeCursor> = None;
            loop {
                let page = {
                    let mut conn = engine.db().conn().await?;
                    tickets::list(&mut conn, guild, Some(state), cursor, 100).await?
                };
                for t in &page {
                    let Some(thread) = t.thread.filter(|th| !active_threads.contains(th)) else {
                        continue;
                    };
                    let gone = match engine.discord().channel_facts(guild, thread).await {
                        Outcome::Done(None) => true,
                        Outcome::Refused(r) => r.kind == RefusalKind::UnknownChannel,
                        _ => false,
                    };
                    if gone && on_thread_deleted(engine, guild, thread).await? {
                        closed += 1;
                    }
                }
                match page.last() {
                    Some(last) if page.len() == 100 => {
                        cursor = Some(TimeCursor {
                            created_at: last.created_at,
                            id: last.id.get(),
                        });
                    }
                    _ => break,
                }
            }
        }
        Ok(closed)
    })
    .await
}
