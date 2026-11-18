//! The tickets module's adapter (§13.2): the ticket buttons, the journal
//! from `MESSAGE_CREATE`, deleted threads, reconciliation on
//! `GUILD_CREATE`, and the journal gap after an invalid session or a
//! dropped event.

use std::sync::Arc;

use quack_core::ids::{ChannelId, GuildId};
use quack_core::modules::ModuleName;
use quack_discord::custom_id::CustomId;
use quack_discord::gateway::Event;
use quack_discord::model::Message;
use quack_engine::{BoxFuture, Engine};
use quack_telemetry::labels::Reason;
use quack_text::error::ErrorFacts;
use quack_text::{Msg, Tone};
use quack_tickets::{Journal, JournalEntry, JournalFileEntry, TicketError};
use tokio::sync::Notify;

use super::{ComponentCall, Module};
use crate::ack::Failure;
use crate::errors;

/// Tells a thread from a channel without waiting (the guild cache):
/// `Some(Some(parent))` for a thread, `Some(None)` for a channel, `None`
/// when the cache doesn't know (the journal then looks the thread up).
pub type ThreadLookup = Arc<dyn Fn(GuildId, ChannelId) -> Option<Option<ChannelId>> + Send + Sync>;

struct Tickets {
    journal: Arc<Journal>,
    threads: ThreadLookup,
}

/// The tickets module over the process's `journal`; `threads` keeps the
/// journal from querying for messages outside threads.
pub fn tickets(journal: Arc<Journal>, threads: ThreadLookup) -> Arc<dyn Module> {
    Arc::new(Tickets { journal, threads })
}

/// The dropped-event hook (§11.9): a dropped `MESSAGE_CREATE` may leave a
/// ticket's journal incomplete. [`TicketGaps::on_dropped`] runs in the
/// shard loop, so it only remembers the channel and wakes a task that
/// records the gap.
#[derive(Clone)]
pub struct TicketGaps {
    journal: Arc<Journal>,
    wake: Arc<Notify>,
}

impl std::fmt::Debug for TicketGaps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TicketGaps")
            .field("pending", &self.journal.pending())
            .finish_non_exhaustive()
    }
}

impl TicketGaps {
    /// The hook over the process's `journal`.
    pub fn new(journal: Arc<Journal>) -> Self {
        Self {
            journal,
            wake: Arc::new(Notify::new()),
        }
    }

    /// Called for every dropped event; cheap and never waits.
    pub fn on_dropped(&self, event: &Event) {
        if let Event::MessageCreate(m) = event
            && let Some(guild) = m.guild_id
        {
            self.journal.remember_gap(guild, m.channel_id);
            self.wake.notify_one();
        }
    }

    /// Records remembered gaps whenever one arrives; runs until dropped.
    pub async fn run(self, engine: Engine) {
        loop {
            self.wake.notified().await;
            self.journal.record_pending(&engine).await;
        }
    }
}

/// A message as the journal takes it.
fn entry(m: &Message, guild: GuildId, parent: Option<ChannelId>) -> JournalEntry {
    let author = &m.author;
    JournalEntry {
        guild,
        channel: m.channel_id,
        parent,
        message: m.id,
        author: author.id,
        author_name: author
            .global_name
            .clone()
            .unwrap_or_else(|| author.username.clone()),
        text: m.content.clone(),
        files: m
            .attachments
            .iter()
            .map(|a| JournalFileEntry {
                name: a.filename.clone(),
                size: a.size,
            })
            .collect(),
        sent_at: m.created_at,
    }
}

/// A ticket error as the person's answer.
fn failure(error: &TicketError) -> Failure {
    if let Some(text) = error.text() {
        return Failure::refused(text, Reason::Conflict);
    }
    match error {
        TicketError::Refused(d) => errors::domain(*d, &ErrorFacts::default()),
        TicketError::NotFound | TicketError::NotAllowed | TicketError::TranscriptUnavailable => {
            Failure::refused(quack_text::error::expired_button(), Reason::Invalid)
        }
        TicketError::DiscordUnavailable => errors::discord_unavailable(),
        TicketError::Db(e) => errors::db(e),
    }
}

impl Tickets {
    async fn component(&self, engine: &Engine, call: ComponentCall<'_>) -> Result<Msg, Failure> {
        let Some(guild) = call.guild else {
            return Err(crate::router::expired());
        };
        match call.id {
            CustomId::OpenTicket => {
                let answer = quack_tickets::open(engine, guild, call.actor.member())
                    .await
                    .map_err(|e| failure(&e))?;
                Ok(Msg::text(Tone::Neutral, answer.text()))
            }
            CustomId::CloseTicket(ticket) => {
                let principal = call.principal.ok_or_else(errors::discord_unavailable)?;
                let answer = quack_tickets::close(engine, &principal, ticket)
                    .await
                    .map_err(|e| failure(&e))?;
                Ok(Msg::text(Tone::Neutral, answer.text()))
            }
            CustomId::FixTicketAccess(ticket) => {
                let principal = call.principal.ok_or_else(errors::discord_unavailable)?;
                let answer = quack_tickets::fix_access(engine, &principal, ticket)
                    .await
                    .map_err(|e| failure(&e))?;
                let tone = match answer {
                    quack_tickets::FixAnswer::Done { .. } => Tone::Success,
                    quack_tickets::FixAnswer::Failed { .. } => Tone::Danger,
                };
                Ok(Msg::text(tone, answer.text()))
            }
            _ => Err(crate::router::expired()),
        }
    }

    async fn event(&self, engine: &Engine, event: &Event) {
        let result = match event {
            Event::MessageCreate(m) => {
                let Some(guild) = m.guild_id else { return };
                // A channel the cache knows is not a thread can't hold a
                // ticket; an unknown one is looked up.
                let parent = match (self.threads)(guild, m.channel_id) {
                    Some(parent) => parent,
                    None => Some(m.channel_id),
                };
                if parent.is_some() {
                    self.journal
                        .on_message(engine, &entry(m, guild, parent))
                        .await;
                }
                Ok(())
            }
            Event::ThreadDelete {
                guild_id,
                thread_id,
                ..
            } => quack_tickets::on_thread_deleted(engine, *guild_id, *thread_id)
                .await
                .map(|_| ()),
            Event::GuildCreate {
                guild, thread_ids, ..
            } => quack_tickets::on_guild_create(engine, guild.id, thread_ids)
                .await
                .map(|_| ()),
            Event::Ready {
                missed_events: true,
                ..
            } => quack_tickets::on_invalid_session(engine).await.map(|_| ()),
            _ => Ok(()),
        };
        if let Err(error) = result {
            tracing::warn!(event = event.name(), %error, "tickets couldn't handle a gateway event");
        }
    }
}

impl Module for Tickets {
    fn name(&self) -> ModuleName {
        ModuleName::Tickets
    }

    fn owns(&self, id: &CustomId) -> bool {
        super::is_module_id(id)
    }

    fn wants(&self, event: &Event) -> bool {
        match event {
            Event::MessageCreate(m) => m.guild_id.is_some(),
            Event::ThreadDelete { .. } | Event::GuildCreate { .. } => true,
            Event::Ready { missed_events, .. } => *missed_events,
            _ => false,
        }
    }

    fn on_component<'a>(
        &'a self,
        engine: &'a Engine,
        call: ComponentCall<'a>,
    ) -> BoxFuture<'a, Result<Msg, Failure>> {
        Box::pin(self.component(engine, call))
    }

    fn on_event<'a>(&'a self, engine: &'a Engine, event: &'a Event) -> BoxFuture<'a, ()> {
        Box::pin(self.event(engine, event))
    }
}
