//! Turning gateway events into posts (§13.3.2). Every handler reads only
//! the config cache and the message cache and never waits: it queues the
//! post or drops it.

use std::time::Instant;

use quack_core::ids::{ChannelId, GuildId, MessageId};
use quack_core::modules::{ActivityEvent, redact_secrets};
use quack_db::activity_log::ActivityLogSettings;
use quack_text::activity::{ActivityPost, Content, FileLink, MessageFacts};
use quack_text::model::{Actor, ChannelRef};
use time::OffsetDateTime;

use crate::batches::Added;
use crate::cache::Cached;
use crate::events::{
    BanChange, BanEntry, ChannelChange, FileFacts, MemberJoined, MemberLeft, MessageCreated,
    MessageUpdated,
};
use crate::log::{ActivityLog, lock, route};

impl ActivityLog {
    /// `MESSAGE_CREATE` in a text channel or thread: caches the message
    /// (redacted, and only what the server's privacy flags show). Quack's
    /// own messages, webhooks', and those in a route channel are cached as
    /// tombstones so their edits and deletes are skipped, not reported.
    pub fn on_message_created(&self, m: &MessageCreated) {
        let Some(s) = self.enabled(m.guild) else {
            return;
        };
        let cached = Cached {
            author: m.author.clone(),
            kind: m.author_kind,
            in_route_channel: is_route_channel(&s, m.channel),
            text: s.include_message_content.then(|| redact(&m.text)),
            files: if s.include_file_names {
                m.files.clone()
            } else {
                Vec::new()
            },
            embeds: if s.include_embed_types {
                m.embeds.clone()
            } else {
                Vec::new()
            },
        };
        lock(&self.inner.cache).put((m.guild, m.channel, m.id), cached, Instant::now());
    }

    /// `MESSAGE_UPDATE`: posts when Discord marked it edited and its text
    /// or files changed. A field missing from the update is unknown, not
    /// empty; known fields merge into the cached copy one by one.
    pub fn on_message_updated(&self, u: &MessageUpdated) {
        let Some(s) = self.enabled(u.guild) else {
            return;
        };
        let key = (u.guild, u.channel, u.id);
        let now = Instant::now();
        let old = lock(&self.inner.cache).get(&key, now);
        let Some(merged) = merge(old.as_ref(), u, &s) else {
            return;
        };
        lock(&self.inner.cache).put(key, merged.clone(), now);
        if merged.tombstone() || !u.edited || !changed(old.as_ref(), u) {
            return;
        }
        let Some(_) = route(&s, ActivityEvent::MessageEdited) else {
            return;
        };
        let before = match &old {
            Some(c) => content(&s, c.text.as_deref()),
            None => Content::Unknown,
        };
        let after = match &u.text {
            Some(t) => content(&s, Some(&redact(t))),
            None if s.include_message_content => Content::Unknown,
            None => Content::Hidden,
        };
        let post = ActivityPost::MessageEdited {
            message: facts(&s, u.channel, u.id, &merged),
            before,
            after,
        };
        self.enqueue(u.guild, ActivityEvent::MessageEdited, post);
    }

    /// `MESSAGE_DELETE`: a cached message posts at once with its author and
    /// text; an uncached one joins its channel's batch.
    pub fn on_message_deleted(&self, guild: GuildId, channel: ChannelId, id: MessageId) {
        let Some(s) = self.enabled(guild) else {
            return;
        };
        let now = Instant::now();
        let cached = lock(&self.inner.cache).take(&(guild, channel, id), now);
        if route(&s, ActivityEvent::MessageDeleted).is_none() || is_route_channel(&s, channel) {
            return;
        }
        match cached {
            Some(c) if c.tombstone() => {}
            Some(c) => {
                let post = ActivityPost::MessageDeleted {
                    message: facts(&s, channel, id, &c),
                    content: content(&s, c.text.as_deref()),
                };
                self.enqueue(guild, ActivityEvent::MessageDeleted, post);
            }
            None => {
                let added = lock(&self.inner.batches).add((guild, channel), id, now);
                match added {
                    Added::Batched => {}
                    Added::Full(ids) => self.post_unknown(guild, channel, ids),
                    Added::Unbatched => self.post_unknown(guild, channel, vec![id]),
                }
            }
        }
    }

    /// `MESSAGE_DELETE_BULK`: one post with the count and what Quack had
    /// seen (at most 20 excerpts).
    pub fn on_messages_bulk_deleted(&self, guild: GuildId, channel: ChannelId, ids: &[MessageId]) {
        let Some(s) = self.enabled(guild) else {
            return;
        };
        let now = Instant::now();
        let mut known = Vec::new();
        {
            let mut cache = lock(&self.inner.cache);
            for id in ids {
                if let Some(c) = cache.take(&(guild, channel, *id), now)
                    && !c.tombstone()
                {
                    known.push((c.author.clone(), content(&s, c.text.as_deref())));
                }
            }
        }
        if route(&s, ActivityEvent::MessagesBulkDeleted).is_none() || is_route_channel(&s, channel)
        {
            return;
        }
        let post = ActivityPost::MessagesBulkDeleted {
            channel: channel_ref(channel),
            ids: ids.to_vec(),
            known,
            at: OffsetDateTime::now_utc(),
        };
        self.enqueue(guild, ActivityEvent::MessagesBulkDeleted, post);
    }

    /// Posts uncached delete batches whose window ended (every open batch
    /// when `all`, at shutdown). Batches of a server whose module is off
    /// are dropped.
    pub fn flush_batches(&self, all: bool) {
        let due = lock(&self.inner.batches).due(Instant::now(), all);
        for ((guild, channel), ids) in due {
            if self
                .enabled(guild)
                .is_some_and(|s| route(&s, ActivityEvent::MessageDeleted).is_some())
            {
                self.post_unknown(guild, channel, ids);
            }
        }
    }

    fn post_unknown(&self, guild: GuildId, channel: ChannelId, ids: Vec<MessageId>) {
        let at = OffsetDateTime::now_utc();
        let channel = channel_ref(channel);
        let post = match ids.as_slice() {
            [] => return,
            [id] => ActivityPost::MessageDeletedUnknown {
                channel,
                id: *id,
                at,
            },
            _ => ActivityPost::MessagesDeletedUnknown { channel, ids, at },
        };
        self.enqueue(guild, ActivityEvent::MessageDeleted, post);
    }

    /// `GUILD_MEMBER_ADD`.
    pub fn on_member_joined(&self, e: &MemberJoined) {
        self.simple(e.guild, ActivityEvent::MemberJoined, || {
            ActivityPost::MemberJoined {
                member: e.member.clone(),
                account_created: e.account_created,
            }
        });
    }

    /// `GUILD_MEMBER_REMOVE`.
    pub fn on_member_left(&self, e: &MemberLeft) {
        self.simple(e.guild, ActivityEvent::MemberLeft, || {
            ActivityPost::MemberLeft {
                username: e.username.clone(),
                id: e.id,
            }
        });
    }

    /// A ban or unban in the audit log by someone other than Quack.
    pub fn on_ban(&self, e: &BanEntry) {
        if e.by_quack {
            return;
        }
        let reason = e.reason.as_deref().map(redact);
        let by = Actor::Person(e.executor.clone());
        let (event, post) = match e.change {
            BanChange::Banned => (
                ActivityEvent::MemberBanned,
                ActivityPost::MemberBanned {
                    member: e.member.clone(),
                    by,
                    reason,
                },
            ),
            BanChange::Unbanned => (
                ActivityEvent::MemberUnbanned,
                ActivityPost::MemberUnbanned {
                    member: e.member.clone(),
                    by,
                    reason,
                },
            ),
        };
        self.simple(e.guild, event, || post);
    }

    /// `GUILD_UPDATE`.
    pub fn on_server_changed(&self, guild: GuildId) {
        self.simple(guild, ActivityEvent::ServerChanged, || {
            ActivityPost::ServerChanged
        });
    }

    /// `CHANNEL_CREATE`, `CHANNEL_UPDATE`, `CHANNEL_DELETE`.
    pub fn on_channel(&self, guild: GuildId, change: &ChannelChange) {
        self.simple(guild, ActivityEvent::ChannelChanged, || match change {
            ChannelChange::Created(c) => ActivityPost::ChannelCreated(c.clone()),
            ChannelChange::Updated(c) => ActivityPost::ChannelChanged(c.clone()),
            ChannelChange::Deleted(_, name) => ActivityPost::ChannelDeleted(name.clone()),
        });
    }

    fn simple(&self, guild: GuildId, event: ActivityEvent, post: impl FnOnce() -> ActivityPost) {
        if self
            .enabled(guild)
            .is_some_and(|s| route(&s, event).is_some())
        {
            self.enqueue(guild, event, post());
        }
    }
}

fn redact(text: &str) -> String {
    redact_secrets(text).into_owned()
}

fn is_route_channel(s: &ActivityLogSettings, channel: ChannelId) -> bool {
    s.routes.iter().any(|(_, c)| *c == channel)
}

fn channel_ref(id: ChannelId) -> ChannelRef {
    ChannelRef { id, name: None }
}

/// What a post shows of a message's text under the privacy flags.
fn content(s: &ActivityLogSettings, text: Option<&str>) -> Content {
    match text {
        _ if !s.include_message_content => Content::Hidden,
        None => Content::Unknown,
        Some("") => Content::Empty,
        Some(t) => Content::Text(t.to_owned()),
    }
}

fn facts(s: &ActivityLogSettings, channel: ChannelId, id: MessageId, c: &Cached) -> MessageFacts {
    MessageFacts {
        author: c.author.clone(),
        channel: channel_ref(channel),
        id,
        at: OffsetDateTime::now_utc(),
        files: if s.include_file_names {
            c.files.iter().map(link).collect()
        } else {
            Vec::new()
        },
        embeds: if s.include_embed_types {
            c.embeds.clone()
        } else {
            Vec::new()
        },
    }
}

fn link(f: &FileFacts) -> FileLink {
    FileLink {
        name: f.name.clone(),
        url: f.url.clone(),
    }
}

/// The cached copy with the update's known fields merged in; `None` when
/// the message is uncached and the update has no author.
fn merge(old: Option<&Cached>, u: &MessageUpdated, s: &ActivityLogSettings) -> Option<Cached> {
    let mut c = match (old, &u.author) {
        (Some(c), _) => c.clone(),
        (None, Some((author, kind))) => Cached {
            author: author.clone(),
            kind: *kind,
            in_route_channel: is_route_channel(s, u.channel),
            text: None,
            files: Vec::new(),
            embeds: Vec::new(),
        },
        (None, None) => return None,
    };
    if let Some(t) = &u.text {
        c.text = s.include_message_content.then(|| redact(t));
    }
    if let Some(f) = &u.files {
        c.files = if s.include_file_names {
            f.clone()
        } else {
            Vec::new()
        };
    }
    if let Some(e) = &u.embeds {
        c.embeds = if s.include_embed_types {
            e.clone()
        } else {
            Vec::new()
        };
    }
    Some(c)
}

/// Whether the text or files changed, as far as Quack can tell. Unknown
/// before (uncached, or text not stored) with a known after counts.
fn changed(old: Option<&Cached>, u: &MessageUpdated) -> bool {
    let text = u.text.as_ref().is_some_and(|t| {
        old.and_then(|c| c.text.as_deref())
            .is_none_or(|before| before != redact(t))
    });
    let files = u.files.as_ref().is_some_and(|f| {
        old.is_none_or(|c| c.files.iter().map(|x| &x.url).ne(f.iter().map(|x| &x.url)))
    });
    text || files
}
