//! The server activity log's adapter (§13.3): decodes gateway events into
//! the module's event types and hands them to the process's
//! [`ActivityLog`], whose handlers never wait.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use quack_activity_log::{
    ActivityLog, AuthorKind, BanChange, BanEntry, ChannelChange, FileFacts, MemberJoined,
    MemberLeft, MessageCreated, MessageUpdated,
};
use quack_core::ids::{AccountId, GuildId};
use quack_core::modules::ModuleName;
use quack_discord::gateway::{self, Event};
use quack_discord::model::{Channel, Message};
use quack_engine::port::Outcome;
use quack_engine::{BoxFuture, Engine};
use quack_text::model::{ChannelRef, Person};
use time::OffsetDateTime;

use super::Module;

/// Discord's epoch (2015-01-01) in Unix milliseconds.
const DISCORD_EPOCH_MS: i64 = 1_420_070_400_000;

struct Activity {
    log: ActivityLog,
    /// Quack's own account, from `READY` (0 until then).
    quack: AtomicU64,
}

/// The server activity log module over the process's `log`.
pub fn activity_log(log: ActivityLog) -> Arc<dyn Module> {
    Arc::new(Activity {
        log,
        quack: AtomicU64::new(0),
    })
}

/// When an account was created, from its snowflake.
fn created_at(id: AccountId) -> OffsetDateTime {
    let ms = i64::try_from(id.get() >> 22).unwrap_or(0) + DISCORD_EPOCH_MS;
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

fn person(m: &Message) -> Person {
    let a = &m.author;
    Person::new(
        a.id,
        a.global_name.clone().unwrap_or_else(|| a.username.clone()),
    )
}

fn files(m: &Message) -> Vec<FileFacts> {
    m.attachments
        .iter()
        .map(|a| FileFacts {
            name: a.filename.clone(),
            url: a.url.clone(),
            size: a.size,
        })
        .collect()
}

fn embeds(m: &Message) -> Vec<String> {
    m.embeds.iter().map(|e| e.kind.clone()).collect()
}

fn channel_ref(c: &Channel) -> ChannelRef {
    ChannelRef::new(c.id, c.name.clone())
}

impl Activity {
    /// Whether the module is on in `guild`, from the config cache.
    fn enabled(engine: &Engine, guild: GuildId) -> bool {
        engine
            .config_cache()
            .cached(guild)
            .is_some_and(|c| c.activity_log.as_ref().is_some_and(|a| a.enabled))
    }

    /// Quack's account: from `READY`, else from the server's facts.
    async fn quack_id(&self, engine: &Engine, guild: GuildId) -> Option<AccountId> {
        if let Some(id) = AccountId::new(self.quack.load(Ordering::Relaxed)) {
            return Some(id);
        }
        match engine.discord().guild_facts(guild).await {
            Outcome::Done(facts) => facts.quack.map(|q| q.id.account()),
            _ => None,
        }
    }

    async fn author_kind(&self, engine: &Engine, guild: GuildId, m: &Message) -> AuthorKind {
        if m.webhook {
            AuthorKind::Webhook
        } else if self.quack_id(engine, guild).await == Some(m.author.id) {
            AuthorKind::Quack
        } else if m.author.bot {
            AuthorKind::Bot
        } else {
            AuthorKind::Member
        }
    }

    async fn message(&self, engine: &Engine, m: &Message, edit: bool) {
        let Some(guild) = m.guild_id else { return };
        if !Self::enabled(engine, guild) {
            return;
        }
        let kind = self.author_kind(engine, guild, m).await;
        if edit {
            self.log.on_message_updated(&MessageUpdated {
                guild,
                channel: m.channel_id,
                id: m.id,
                author: Some((person(m), kind)),
                edited: m.edited_at.is_some(),
                text: Some(m.content.clone()),
                files: Some(files(m)),
                embeds: Some(embeds(m)),
            });
        } else {
            self.log.on_message_created(&MessageCreated {
                guild,
                channel: m.channel_id,
                id: m.id,
                author: person(m),
                author_kind: kind,
                text: m.content.clone(),
                files: files(m),
                embeds: embeds(m),
            });
        }
    }

    async fn event(&self, engine: &Engine, event: &Event) {
        let log = &self.log;
        match event {
            Event::Ready { quack_id, .. } => self.quack.store(quack_id.get(), Ordering::Relaxed),
            Event::MessageCreate(m) => self.message(engine, m, false).await,
            Event::MessageUpdate(m) => self.message(engine, m, true).await,
            Event::MessageDelete {
                guild_id: Some(guild),
                channel_id,
                message_id,
            } => log.on_message_deleted(*guild, *channel_id, *message_id),
            Event::MessageDeleteBulk {
                guild_id: Some(guild),
                channel_id,
                message_ids,
            } => log.on_messages_bulk_deleted(*guild, *channel_id, message_ids),
            Event::MemberAdd { guild_id, member } => log.on_member_joined(&MemberJoined {
                guild: *guild_id,
                member: Person::new(
                    member.id.account(),
                    member
                        .global_name
                        .clone()
                        .unwrap_or_else(|| member.username.clone()),
                ),
                account_created: created_at(member.id.account()),
            }),
            Event::MemberRemove { guild_id, account } => log.on_member_left(&MemberLeft {
                guild: *guild_id,
                username: account.username.clone(),
                id: account.id,
            }),
            Event::Ban {
                guild_id,
                change,
                target,
                executor: Some(executor),
                reason,
            } => {
                let by_quack = self.quack_id(engine, *guild_id).await == Some(*executor);
                log.on_ban(&BanEntry {
                    guild: *guild_id,
                    change: match change {
                        gateway::BanChange::Banned => BanChange::Banned,
                        gateway::BanChange::Unbanned => BanChange::Unbanned,
                    },
                    member: Person {
                        id: *target,
                        name: None,
                    },
                    executor: Person {
                        id: *executor,
                        name: None,
                    },
                    by_quack,
                    reason: reason.clone(),
                });
            }
            Event::GuildUpdate { guild } => log.on_server_changed(guild.id),
            Event::ChannelCreate(c) | Event::ChannelUpdate(c) | Event::ChannelDelete(c) => {
                let Some(guild) = c.guild_id else { return };
                let change = match event {
                    Event::ChannelCreate(_) => ChannelChange::Created(channel_ref(c)),
                    Event::ChannelUpdate(_) => ChannelChange::Updated(channel_ref(c)),
                    _ => ChannelChange::Deleted(c.id, c.name.clone()),
                };
                log.on_channel(guild, &change);
            }
            Event::GuildDelete {
                guild_id,
                unavailable: false,
            } => log.forget_guild(*guild_id),
            _ => {}
        }
    }
}

impl Module for Activity {
    fn name(&self) -> ModuleName {
        ModuleName::ActivityLog
    }

    fn wants(&self, event: &Event) -> bool {
        matches!(
            event,
            Event::Ready { .. }
                | Event::MessageCreate(_)
                | Event::MessageUpdate(_)
                | Event::MessageDelete { .. }
                | Event::MessageDeleteBulk { .. }
                | Event::MemberAdd { .. }
                | Event::MemberRemove { .. }
                | Event::Ban { .. }
                | Event::GuildUpdate { .. }
                | Event::ChannelCreate(_)
                | Event::ChannelUpdate(_)
                | Event::ChannelDelete(_)
                | Event::GuildDelete { .. }
        )
    }

    fn on_event<'a>(&'a self, engine: &'a Engine, event: &'a Event) -> BoxFuture<'a, ()> {
        Box::pin(self.event(engine, event))
    }
}
