//! The server activity log's process-wide state and delivery (§13.3.2):
//! a bounded in-memory queue of posts (not durable, by design) drained by
//! two sender tasks. Enqueueing never waits: a full queue drops the post
//! and counts it.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use quack_core::ids::{ChannelId, GuildId};
use quack_core::modules::ActivityEvent;
use quack_core::permissions::Permission;
use quack_db::activity_log::ActivityLogSettings;
use quack_engine::Engine;
use quack_engine::channels::{self, ChannelCheck};
use quack_engine::port::{Nonce, Outcome, RefusalKind};
use quack_telemetry::labels::{self, ActivityStage, QueueName};
use quack_telemetry::metrics;
use quack_text::activity::ActivityPost;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::batches::Batches;
use crate::cache::{self, MessageCache};

/// The queue's name in `quack.queue.*`.
const QUEUE: QueueName = QueueName::new("activity_log.posts");
/// Sender tasks.
pub const SENDERS: usize = 2;
/// Tries of one post (the first and 2 more) on `RetryAfter` or `Unknown`.
const TRIES: u32 = 3;
/// The longest a sender waits before trying a post again.
const MAX_WAIT: Duration = Duration::from_secs(5);
/// How long a destination's staff-only check is reused.
const DESTINATION_TTL: Duration = Duration::from_secs(60);
/// How often the batch ticker looks for closed batches.
const TICK: Duration = Duration::from_secs(1);

/// The module's bounds; [`Limits::default`] is the design's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Posts waiting at most (10,000).
    pub queue: usize,
    /// Cached messages at most (50,000).
    pub cache_entries: usize,
    /// Estimated cached bytes at most (64 MiB).
    pub cache_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            queue: 10_000,
            cache_entries: cache::MAX_ENTRIES,
            cache_bytes: cache::MAX_BYTES,
        }
    }
}

/// One post waiting to be delivered.
#[derive(Debug)]
#[allow(
    clippy::struct_field_names,
    reason = "`post.post` reads as the post's content"
)]
pub(crate) struct Post {
    pub guild: GuildId,
    pub event: ActivityEvent,
    pub post: ActivityPost,
    pub nonce: Nonce,
}

/// How a delivery ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivered {
    /// Posted in the routed channel.
    Posted,
    /// The route is gone or the module is off.
    Dropped,
    /// Refused, not a staff-only channel, or out of tries.
    Failed,
}

/// The server activity log: one per process, shared by the gateway
/// handlers (`on_*`, which never wait) and its sender tasks.
#[derive(Clone)]
pub struct ActivityLog {
    pub(crate) inner: Arc<Inner>,
}

impl std::fmt::Debug for ActivityLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActivityLog")
            .field("queued", &self.queued())
            .field("cached", &self.cached())
            .finish_non_exhaustive()
    }
}

pub(crate) struct Inner {
    pub engine: Engine,
    pub cache: Mutex<MessageCache>,
    pub batches: Mutex<Batches>,
    tx: mpsc::Sender<Post>,
    rx: tokio::sync::Mutex<mpsc::Receiver<Post>>,
    destinations: Mutex<HashMap<ChannelId, (Instant, bool)>>,
    next_nonce: AtomicU64,
    /// Per server: posts delivered and posts dropped since start (the
    /// module page's counters; one entry per server that posted).
    counts: Mutex<HashMap<GuildId, (u64, u64)>>,
}

/// Locks a mutex, recovering from a poisoned one.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl ActivityLog {
    /// The module with the design's bounds. Call [`ActivityLog::spawn`] to
    /// start delivering.
    pub fn new(engine: Engine) -> Self {
        Self::with_limits(engine, Limits::default())
    }

    /// The module with other bounds (tests).
    pub fn with_limits(engine: Engine, limits: Limits) -> Self {
        let (tx, rx) = mpsc::channel(limits.queue.max(1));
        let inner = Arc::new(Inner {
            engine,
            cache: Mutex::new(MessageCache::new(
                limits.cache_entries,
                limits.cache_bytes,
                cache::TTL,
            )),
            batches: Mutex::new(Batches::default()),
            tx,
            rx: tokio::sync::Mutex::new(rx),
            destinations: Mutex::new(HashMap::new()),
            next_nonce: AtomicU64::new(0),
            counts: Mutex::new(HashMap::new()),
        });
        let weak = Arc::downgrade(&inner);
        metrics().caches.observe_size(cache::NAME, move || {
            upgrade(&weak).map_or(0, |i| lock(&i.cache).len() as u64)
        });
        let weak = Arc::downgrade(&inner);
        metrics().queues.observe_depth(QUEUE, move || {
            upgrade(&weak).map_or(0, |i| (i.tx.max_capacity() - i.tx.capacity()) as u64)
        });
        Self { inner }
    }

    /// Posts waiting to be delivered.
    pub fn queued(&self) -> usize {
        self.inner.tx.max_capacity() - self.inner.tx.capacity()
    }

    /// Posts delivered and posts dropped (queue full, route gone) in
    /// `guild` since this process started.
    pub fn counts(&self, guild: GuildId) -> (u64, u64) {
        lock(&self.inner.counts)
            .get(&guild)
            .copied()
            .unwrap_or_default()
    }

    fn count(&self, guild: GuildId, posted: bool) {
        let mut counts = lock(&self.inner.counts);
        let entry = counts.entry(guild).or_default();
        if posted {
            entry.0 += 1;
        } else {
            entry.1 += 1;
        }
    }

    /// Messages in the cache.
    pub fn cached(&self) -> usize {
        lock(&self.inner.cache).len()
    }

    /// Starts the two sender tasks and the batch ticker. Abort them (or
    /// drop the runtime) after [`ActivityLog::shutdown`].
    pub fn spawn(&self) -> Vec<JoinHandle<()>> {
        let mut tasks = Vec::new();
        for _ in 0..SENDERS {
            let log = self.clone();
            tasks.push(tokio::spawn(async move {
                while log.deliver_next_waiting().await.is_some() {}
            }));
        }
        let log = self.clone();
        tasks.push(tokio::spawn(async move {
            loop {
                tokio::time::sleep(TICK).await;
                log.flush_batches(false);
            }
        }));
        tasks
    }

    /// Graceful shutdown (§17.5): posts every open delete batch, then waits
    /// until the queue is empty or `deadline` passes.
    pub async fn shutdown(&self, deadline: Duration) {
        self.flush_batches(true);
        let _ = tokio::time::timeout(deadline, async {
            while self.queued() > 0 {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await;
    }

    /// Delivers one queued post, if any, in this task (tests, shutdown).
    pub async fn deliver_next(&self) -> Option<Delivered> {
        let post = self.inner.rx.lock().await.try_recv().ok()?;
        Some(self.deliver(post).await)
    }

    async fn deliver_next_waiting(&self) -> Option<Delivered> {
        let post = self.inner.rx.lock().await.recv().await?;
        Some(self.deliver(post).await)
    }

    /// Queues a post without waiting; a full queue drops it (counted).
    pub(crate) fn enqueue(&self, guild: GuildId, event: ActivityEvent, post: ActivityPost) {
        let nonce = self.nonce(guild, event);
        let label = event_label(event);
        metrics().activity_log.event(label, ActivityStage::Seen);
        let post = Post {
            guild,
            event,
            post,
            nonce,
        };
        if self.inner.tx.try_send(post).is_err() {
            self.count(guild, false);
            metrics().activity_log.event(label, ActivityStage::Dropped);
            metrics().queues.dropped(QUEUE, 1);
            tracing::debug!(
                guild_id = guild.get(),
                event = event.as_str(),
                "activity post dropped: queue full"
            );
        }
    }

    /// A 25-character nonce unique to this post, fixed across its tries.
    fn nonce(&self, guild: GuildId, event: ActivityEvent) -> Nonce {
        let n = self.inner.next_nonce.fetch_add(1, Ordering::Relaxed);
        let mut h = DefaultHasher::new();
        (guild.get(), event.as_str(), n, std::process::id()).hash(&mut h);
        Nonce::new(format!("al{:016x}{n:07x}", h.finish()))
    }

    /// The module's settings for a server, from the config cache only
    /// (never the database: the event path doesn't wait). `None` while it
    /// is off.
    pub(crate) fn enabled(&self, guild: GuildId) -> Option<ActivityLogSettings> {
        let settings = self
            .inner
            .engine
            .config_cache()
            .cached(guild)
            .and_then(|c| c.activity_log.clone())
            .filter(|s| s.enabled);
        if settings.is_none() {
            self.forget_if_held(guild);
        }
        settings
    }

    async fn deliver(&self, post: Post) -> Delivered {
        let label = event_label(post.event);
        let delivered =
            crate::obs::span("activity_post", post.guild, self.try_deliver(&post)).await;
        let stage = match delivered {
            Delivered::Posted => ActivityStage::Posted,
            Delivered::Dropped => ActivityStage::Dropped,
            Delivered::Failed => ActivityStage::Failed,
        };
        match delivered {
            Delivered::Posted => self.count(post.guild, true),
            Delivered::Dropped => self.count(post.guild, false),
            Delivered::Failed => {}
        }
        metrics().activity_log.event(label, stage);
        delivered
    }

    async fn try_deliver(&self, post: &Post) -> Delivered {
        // The route as it is now: a changed route goes to the new channel.
        let channel = self.enabled(post.guild).and_then(|s| route(&s, post.event));
        let Some(channel) = channel else {
            return Delivered::Dropped;
        };
        match self.destination(post.guild, channel).await {
            Some(true) => {}
            // Not staff-only, gone, or Discord didn't answer.
            Some(false) | None => return Delivered::Failed,
        }
        let msg = quack_text::activity::post(&post.post);
        let discord = self.inner.engine.discord();
        for _ in 0..TRIES {
            match discord.send(channel, &msg, &[], &post.nonce).await {
                Outcome::Done(_) => return Delivered::Posted,
                Outcome::Refused(r) => {
                    if r.kind == RefusalKind::UnknownChannel {
                        self.channel_gone(post.guild, channel).await;
                    }
                    return Delivered::Failed;
                }
                Outcome::RetryAfter(wait) => tokio::time::sleep(wait.min(MAX_WAIT)).await,
                Outcome::NotSent(_) | Outcome::Unknown(_) => {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }
        }
        Delivered::Failed
    }

    /// The staff-only check of a destination (§13.1.3), reused for 60
    /// seconds. `None` when Discord didn't answer.
    async fn destination(&self, guild: GuildId, channel: ChannelId) -> Option<bool> {
        let now = Instant::now();
        if let Some((at, ok)) = lock(&self.inner.destinations).get(&channel).copied()
            && now.duration_since(at) < DESTINATION_TTL
        {
            return Some(ok);
        }
        let needs = Permission::ViewChannel | Permission::SendMessages;
        let ok = match channels::staff_channel(&self.inner.engine, guild, channel, needs).await {
            ChannelCheck::Ok => true,
            ChannelCheck::Problems(_) => false,
            ChannelCheck::Missing => {
                self.channel_gone(guild, channel).await;
                false
            }
            ChannelCheck::Unavailable => return None,
        };
        lock(&self.inner.destinations).insert(channel, (now, ok));
        Some(ok)
    }

    /// The route's channel is gone: reconcile it like `CHANNEL_DELETE`
    /// (the route is removed; with none left the module turns off,
    /// `channel_deleted`).
    async fn channel_gone(&self, guild: GuildId, channel: ChannelId) {
        let engine = &self.inner.engine;
        if let Err(error) = quack_engine::guilds::channel_deleted(engine, guild, channel).await {
            tracing::warn!(%error, channel_id = channel.get(), "clearing a lost route failed");
        }
        if let Err(error) = engine.config_cache().reload(engine.db(), guild).await {
            tracing::warn!(%error, "reloading the config cache failed");
        }
    }

    /// Disabling the module or Quack leaving empties its batches and
    /// cache for that server.
    pub fn forget_guild(&self, guild: GuildId) {
        let removed = lock(&self.inner.cache).forget_guild(guild);
        lock(&self.inner.batches).forget_guild(guild);
        if removed > 0 {
            tracing::debug!(guild_id = guild.get(), removed, "activity cache emptied");
        }
    }

    fn forget_if_held(&self, guild: GuildId) {
        if lock(&self.inner.cache).has_guild(guild) {
            self.forget_guild(guild);
        }
    }
}

fn upgrade(weak: &Weak<Inner>) -> Option<Arc<Inner>> {
    weak.upgrade()
}

/// The channel an event is routed to.
pub(crate) fn route(settings: &ActivityLogSettings, event: ActivityEvent) -> Option<ChannelId> {
    settings
        .routes
        .iter()
        .find(|(e, _)| *e == event)
        .map(|(_, c)| *c)
}

/// The event's metric label.
pub(crate) fn event_label(event: ActivityEvent) -> labels::ActivityEvent {
    labels::ActivityEvent::new(event.as_str())
}
