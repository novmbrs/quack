//! The gateway runner (§11.9): one shard with the design's intents,
//! automatic resume and reconnect (Twilight's), caches updated in the shard
//! loop, and bounded dispatch of typed [`Event`]s to a handler.
//!
//! The shard loop never waits on a handler. Connection state and heartbeat
//! latency are exposed through [`GatewayStatus`] for readiness and metrics;
//! drops, failed reconnect attempts, and the session coming back are logged
//! once each (see the `link` module).

mod dispatch;
mod event;
mod link;

#[cfg(test)]
mod tests;

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use quack_telemetry::labels::GatewayConnectionEvent;
use quack_telemetry::metrics;
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;
use twilight_gateway::{
    CloseFrame, Config, Event as Raw, EventTypeFlags, Intents, Shard, ShardId, StreamExt as _,
};

use twilight_gateway::error::ReceiveMessageErrorType;

use crate::cache::{GuildCache, MemberCache, count_members, feed_members};
use crate::model::MemberCount;
use crate::rest::{DiscordClient, Outcome};
use link::{Change, Link};

pub use dispatch::{
    EVENT_QUEUE_LEN, EVENT_QUEUES, INTERACTION_QUEUE_LEN, MAX_INTERACTION_HANDLERS,
};
pub use event::{BanChange, Event};

/// The intents Quack connects with (fixed, §11.9). `GUILD_MEMBERS` and
/// `MESSAGE_CONTENT` are privileged.
pub const INTENTS: Intents = Intents::GUILDS
    .union(Intents::GUILD_MEMBERS)
    .union(Intents::GUILD_MODERATION)
    .union(Intents::GUILD_MESSAGES)
    .union(Intents::MESSAGE_CONTENT);

/// Discord's close code for intents the application may not use.
pub const DISALLOWED_INTENTS: u16 = 4014;

/// Discord's close code for a token it doesn't accept.
pub const AUTHENTICATION_FAILED: u16 = 4004;

/// How long shutdown waits for Discord to confirm the close.
const CLOSE_WAIT: Duration = Duration::from_secs(5);

/// The events Twilight deserializes; everything else is skipped cheaply.
pub(crate) fn wanted() -> EventTypeFlags {
    EventTypeFlags::READY
        | EventTypeFlags::RESUMED
        | EventTypeFlags::GATEWAY_HELLO
        | EventTypeFlags::GATEWAY_HEARTBEAT_ACK
        | EventTypeFlags::GATEWAY_INVALIDATE_SESSION
        | EventTypeFlags::GATEWAY_RECONNECT
        | EventTypeFlags::GUILD_CREATE
        | EventTypeFlags::GUILD_UPDATE
        | EventTypeFlags::GUILD_DELETE
        | EventTypeFlags::UNAVAILABLE_GUILD
        | EventTypeFlags::ROLE_CREATE
        | EventTypeFlags::ROLE_UPDATE
        | EventTypeFlags::ROLE_DELETE
        | EventTypeFlags::CHANNEL_CREATE
        | EventTypeFlags::CHANNEL_UPDATE
        | EventTypeFlags::CHANNEL_DELETE
        | EventTypeFlags::THREAD_CREATE
        | EventTypeFlags::THREAD_UPDATE
        | EventTypeFlags::THREAD_DELETE
        | EventTypeFlags::THREAD_LIST_SYNC
        | EventTypeFlags::MEMBER_ADD
        | EventTypeFlags::MEMBER_UPDATE
        | EventTypeFlags::MEMBER_REMOVE
        | EventTypeFlags::MESSAGE_CREATE
        | EventTypeFlags::MESSAGE_UPDATE
        | EventTypeFlags::MESSAGE_DELETE
        | EventTypeFlags::MESSAGE_DELETE_BULK
        | EventTypeFlags::GUILD_AUDIT_LOG_ENTRY_CREATE
        | EventTypeFlags::INTERACTION_CREATE
}

/// An interaction from a frame Twilight couldn't deserialize (see
/// [`crate::interaction::recover_dispatch`]).
fn recover(error: &twilight_gateway::error::ReceiveMessageError) -> Option<Event> {
    match error.kind() {
        ReceiveMessageErrorType::Deserializing { event } => {
            crate::interaction::recover_dispatch(event).map(|i| Event::Interaction(Box::new(i)))
        }
        _ => None,
    }
}

/// A receive error as a closed kind and, for a frame that couldn't be
/// read, its event name (`t`) when it is a plain event name. Nothing else
/// of the frame is ever logged.
pub(crate) fn describe(kind: &ReceiveMessageErrorType) -> (&'static str, Option<String>) {
    match kind {
        ReceiveMessageErrorType::Deserializing { event } => ("deserializing", event_name(event)),
        ReceiveMessageErrorType::Reconnect => ("reconnect", None),
        #[allow(
            unreachable_patterns,
            reason = "Compression exists only with a compression feature"
        )]
        _ => ("compression", None),
    }
}

/// The `t` field of a raw gateway frame, when it looks like an event name
/// (`MESSAGE_CREATE`).
pub(crate) fn event_name(frame: &str) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct Envelope {
        t: Option<String>,
    }
    let name = serde_json::from_str::<Envelope>(frame).ok()?.t?;
    (name.len() <= 64 && name.bytes().all(|b| b.is_ascii_uppercase() || b == b'_')).then_some(name)
}

/// Queues an event; a full queue drops it and tells `on_dropped`.
fn hand_off(dispatcher: &dispatch::Dispatcher, event: Event, on_dropped: &impl Fn(&Event)) {
    if let Some(dropped) = dispatcher.dispatch(event) {
        tracing::warn!(
            event = dropped.name(),
            "dropped a gateway event: queue full"
        );
        on_dropped(&dropped);
    }
}

/// Why the gateway stopped for good.
#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    /// Close code 4014: the privileged intents are not enabled.
    #[error(
        "Discord refused Quack's gateway intents (close code 4014). Turn on the Server Members \
         and Message Content intents for the application in the Discord developer portal; an \
         application in 100 or more servers also needs Discord's approval for them"
    )]
    DisallowedIntents,
    /// Close code 4004: Discord doesn't accept the bot token.
    #[error(
        "Discord rejected the bot token (close code 4004). Set QUACK_DISCORD_TOKEN to the \
         application's current bot token from the Discord developer portal"
    )]
    AuthenticationFailed,
    /// Another close code Twilight does not reconnect after (for example
    /// 4013, invalid intents).
    #[error("Discord closed the gateway and Quack can't reconnect ({})", close_code(*.0))]
    Closed(Option<u16>),
    /// Discord recommends more than one shard (about 2,500 servers); Quack
    /// runs one (§11.9, §20.4).
    #[error(
        "Discord recommends {0} gateway shards for this application, but Quack runs one shard \
         (up to about 2,500 servers)"
    )]
    TooManyShards(u32),
    /// `GET /gateway/bot` failed, so the shard count is unknown.
    #[error("couldn't read the gateway settings from Discord ({0})")]
    ShardCheck(String),
}

/// A close code for a log line or an error message.
fn close_code(code: Option<u16>) -> String {
    code.map_or_else(|| "no close code".to_owned(), |c| format!("close code {c}"))
}

/// Logs and counts one connection change; returns how long the shard loop
/// waits before reading again. `why` says what happened, in plain words.
fn report(status: &GatewayStatus, change: Change, why: &str) -> Duration {
    let gateway = &metrics().gateway;
    match change {
        Change::Connected => tracing::info!("connected to the gateway"),
        Change::Back {
            resumed,
            down,
            failures,
        } => {
            let down_ms = u64::try_from(down.as_millis()).unwrap_or(u64::MAX);
            if resumed {
                tracing::info!(down_ms, failures, "resumed the gateway session");
            } else {
                tracing::info!(
                    down_ms,
                    failures,
                    "reconnected to the gateway with a new session"
                );
            }
        }
        Change::Dropped => {
            status.drops.fetch_add(1, Ordering::Relaxed);
            gateway.connection(GatewayConnectionEvent::Reconnect);
            tracing::info!(why, "the gateway connection dropped; reconnecting");
        }
        Change::Failed { failures, wait } => {
            status.connect_failures.fetch_add(1, Ordering::Relaxed);
            gateway.connection(GatewayConnectionEvent::ConnectFailed);
            let wait_ms = u64::try_from(wait.as_millis()).unwrap_or(u64::MAX);
            if failures >= link::WARN_AFTER {
                tracing::warn!(
                    failures,
                    why,
                    wait_ms,
                    "still can't reconnect to the gateway"
                );
            } else {
                tracing::info!(
                    failures,
                    why,
                    wait_ms,
                    "couldn't reconnect to the gateway; trying again"
                );
            }
            return wait;
        }
        Change::Nothing => {}
    }
    Duration::ZERO
}

/// Waits `wait` (nothing when zero); false when shutdown came first.
async fn pause(wait: Duration, shutdown: &CancellationToken) -> bool {
    if wait.is_zero() {
        return true;
    }
    tokio::select! {
        biased;
        () = shutdown.cancelled() => false,
        () = tokio::time::sleep(wait) => true,
    }
}

/// What a frame means for the connection, and why, for [`report`].
fn connection_change(link: &mut Link, raw: &Raw, now: Instant) -> (Change, String) {
    match raw {
        Raw::Ready(_) => (link.session(false, now), String::new()),
        Raw::Resumed => (link.session(true, now), String::new()),
        Raw::GatewayHello(_) => (
            link.opened(now),
            "the connection ended without a close frame".to_owned(),
        ),
        Raw::GatewayReconnect => (
            link.closed(now),
            "Discord asked Quack to reconnect".to_owned(),
        ),
        Raw::GatewayInvalidateSession(_) => {
            (link.closed(now), "Discord ended the session".to_owned())
        }
        Raw::GatewayClose(frame) => (
            link.closed(now),
            format!("closed ({})", close_code(frame.as_ref().map(|f| f.code))),
        ),
        _ => (Change::Nothing, String::new()),
    }
}

/// Checks `GET /gateway/bot` before connecting: more than one recommended
/// shard stops `serve` (§11.9).
pub async fn check_shards(client: &DiscordClient) -> Result<(), GatewayError> {
    match client.gateway_bot().await {
        Outcome::Done(info) if info.shards <= 1 => Ok(()),
        Outcome::Done(info) => Err(GatewayError::TooManyShards(info.shards)),
        Outcome::Refused(r) => Err(GatewayError::ShardCheck(r.machine_code())),
        Outcome::RetryAfter(d) => Err(GatewayError::ShardCheck(format!("rate limited for {d:?}"))),
        Outcome::NotSent(c) => Err(GatewayError::ShardCheck(format!("not sent: {c:?}"))),
        Outcome::Unknown(c) => Err(GatewayError::ShardCheck(format!("no answer: {c:?}"))),
    }
}

/// The gateway's connection state, shared with readiness checks and ops.
#[derive(Debug, Default)]
pub struct GatewayStatus {
    connected: AtomicBool,
    latency_micros: AtomicU64,
    identifies: AtomicU64,
    resumes: AtomicU64,
    drops: AtomicU64,
    connect_failures: AtomicU64,
    invalid_sessions: AtomicU64,
}

/// How often each connection change happened since the process started
/// (the same events as `quack.gateway.connection.events`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConnectionCounts {
    /// New sessions (`READY`).
    pub identifies: u64,
    /// Resumed sessions.
    pub resumes: u64,
    /// Live connections that dropped.
    pub drops: u64,
    /// Failed attempts to reconnect.
    pub connect_failures: u64,
    /// Sessions Discord invalidated.
    pub invalid_sessions: u64,
}

impl GatewayStatus {
    /// The connection changes counted so far.
    pub fn counts(&self) -> ConnectionCounts {
        ConnectionCounts {
            identifies: self.identifies.load(Ordering::Relaxed),
            resumes: self.resumes.load(Ordering::Relaxed),
            drops: self.drops.load(Ordering::Relaxed),
            connect_failures: self.connect_failures.load(Ordering::Relaxed),
            invalid_sessions: self.invalid_sessions.load(Ordering::Relaxed),
        }
    }

    /// Whether the session is `READY` or `RESUMED` right now.
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    /// The latest heartbeat round trip, once one was acknowledged.
    pub fn heartbeat_latency(&self) -> Option<Duration> {
        match self.latency_micros.load(Ordering::Relaxed) {
            0 => None,
            micros => Some(Duration::from_micros(micros)),
        }
    }

    fn set_connected(&self, connected: bool) {
        self.connected.store(connected, Ordering::Release);
    }
}

/// One shard and the caches it feeds.
pub struct Gateway {
    shard: Shard,
    guilds: Arc<GuildCache>,
    members: Arc<MemberCache>,
    status: Arc<GatewayStatus>,
}

impl std::fmt::Debug for Gateway {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gateway").finish_non_exhaustive()
    }
}

impl Gateway {
    /// A gateway for the bot token (without `Bot `), feeding these caches.
    /// Nothing connects until [`Gateway::run`].
    pub fn new(token: String, guilds: Arc<GuildCache>, members: Arc<MemberCache>) -> Self {
        let config = Config::new(token, INTENTS);
        Self {
            shard: Shard::with_config(ShardId::ONE, config),
            guilds,
            members,
            status: Arc::new(GatewayStatus::default()),
        }
    }

    /// The connection state, for `/readyz` and metrics.
    pub fn status(&self) -> Arc<GatewayStatus> {
        self.status.clone()
    }

    /// Runs until `shutdown` is cancelled (then closes normally, drops
    /// queued events, and waits for in-flight handlers) or Discord closes
    /// the gateway for good. `on_dropped` runs in the shard loop for each
    /// event a full queue dropped (a ticket thread's journal gap, §11.9);
    /// keep it cheap.
    pub async fn run<H, F, D>(
        mut self,
        handler: H,
        on_dropped: D,
        shutdown: CancellationToken,
    ) -> Result<(), GatewayError>
    where
        H: Fn(Event) -> F + Send + Sync + 'static,
        F: Future<Output = ()> + Send + 'static,
        D: Fn(&Event) + Send + Sync + 'static,
    {
        let dispatcher = dispatch::Dispatcher::start(Arc::new(handler));
        self.guilds.observe();
        self.members.observe();
        let status = Arc::downgrade(&self.status);
        metrics()
            .gateway
            .observe_connected(move || status.upgrade().is_some_and(|s| s.is_connected()));
        let guilds = Arc::downgrade(&self.guilds);
        metrics()
            .gateway
            .observe_guilds(move || guilds.upgrade().map_or(0, |g| g.guild_count() as u64));
        let mut sessions = 0_u32;
        let mut close_code = None;
        let mut link = Link::new(Instant::now());
        let result = loop {
            let item = tokio::select! {
                biased;
                () = shutdown.cancelled() => break Ok(()),
                item = self.shard.next_event(wanted()) => item,
            };
            let raw = match item {
                None => {
                    break Err(match close_code {
                        Some(DISALLOWED_INTENTS) => GatewayError::DisallowedIntents,
                        Some(AUTHENTICATION_FAILED) => GatewayError::AuthenticationFailed,
                        code => GatewayError::Closed(code),
                    });
                }
                Some(Err(error)) => {
                    let wait = if let Some(event) = recover(&error) {
                        hand_off(&dispatcher, event, &on_dropped);
                        Duration::ZERO
                    } else {
                        self.unreadable(&error, &mut link)
                    };
                    if !pause(wait, &shutdown).await {
                        break Ok(());
                    }
                    continue;
                }
                Some(Ok(raw)) => raw,
            };
            if let Raw::GatewayClose(frame) = &raw {
                close_code = frame.as_ref().map(|f| f.code);
            }
            self.track(&raw);
            let (change, why) = connection_change(&mut link, &raw, Instant::now());
            if !pause(report(&self.status, change, &why), &shutdown).await {
                break Ok(());
            }
            if let Some(event) = event::map(&raw, sessions) {
                let count = self.member_count(&event);
                let event = event.with_members(count);
                if matches!(event, Event::Ready { .. }) {
                    sessions += 1;
                }
                hand_off(&dispatcher, event, &on_dropped);
            }
        };
        if result.is_ok() {
            self.close().await;
        }
        self.status.set_connected(false);
        self.guilds.untrust(false);
        dispatcher.stop().await;
        result
    }

    /// A receive error that isn't a recoverable interaction: logs it, marks
    /// what it broke, and returns the wait before the next read.
    fn unreadable(
        &self,
        error: &twilight_gateway::error::ReceiveMessageError,
        link: &mut Link,
    ) -> Duration {
        let (kind, event) = describe(error.kind());
        self.lost(kind, event.as_deref());
        let now = Instant::now();
        if let ReceiveMessageErrorType::Reconnect = error.kind() {
            // A failed connection attempt: its cause is a network or TLS
            // error and holds no frame, so it is safe to log.
            let cause = std::error::Error::source(error)
                .map_or_else(|| "unknown".to_owned(), ToString::to_string);
            return report(&self.status, link.connect_failed(now), &cause);
        }
        // Never the error itself: Twilight's message holds the whole frame
        // (interaction tokens, message text).
        tracing::warn!(kind, event, "couldn't read a gateway event");
        if kind == "deserializing" {
            Duration::ZERO
        } else {
            // A compressed frame Twilight couldn't read closes the connection.
            report(
                &self.status,
                link.closed(now),
                "an unreadable compressed frame",
            )
        }
    }

    /// A frame Twilight couldn't read. An unreadable event other than a
    /// message or an interaction may have been a cache update the resume
    /// won't replay: the cache isn't trusted until a new session. A broken
    /// connection (compression, reconnect) isn't trusted until it is back.
    fn lost(&self, kind: &str, event: Option<&str>) {
        if kind == "deserializing" {
            let harmless = event.is_some_and(|e| {
                e.starts_with("MESSAGE_") || e.starts_with("TYPING") || e == "INTERACTION_CREATE"
            });
            if !harmless {
                self.guilds.untrust(true);
            }
        } else {
            self.status.set_connected(false);
            self.guilds.untrust(false);
        }
    }

    /// The member count to send with an event of `guild`, read now.
    fn member_count(&self, event: &Event) -> Option<MemberCount> {
        if !matches!(
            event,
            Event::GuildCreate { .. } | Event::MemberAdd { .. } | Event::MemberRemove { .. }
        ) {
            return None;
        }
        let guild = event.guild_id()?;
        self.guilds.member_count(guild).map(|members| MemberCount {
            members,
            at: OffsetDateTime::now_utc(),
        })
    }

    /// Updates the caches and the connection state for one event.
    fn track(&self, raw: &Raw) {
        self.guilds.update(raw);
        count_members(&self.guilds, &self.members, raw);
        feed_members(&self.members, raw);
        let gateway = &metrics().gateway;
        match raw {
            Raw::Ready(_) => {
                self.status.set_connected(true);
                self.status.identifies.fetch_add(1, Ordering::Relaxed);
                gateway.connection(GatewayConnectionEvent::Identify);
            }
            Raw::Resumed => {
                self.status.set_connected(true);
                self.status.resumes.fetch_add(1, Ordering::Relaxed);
                gateway.connection(GatewayConnectionEvent::Resume);
            }
            // Drops are counted once each by `report`.
            Raw::GatewayReconnect | Raw::GatewayClose(_) => self.status.set_connected(false),
            Raw::GatewayInvalidateSession(_) => {
                self.status.set_connected(false);
                self.status.invalid_sessions.fetch_add(1, Ordering::Relaxed);
                gateway.connection(GatewayConnectionEvent::InvalidSession);
            }
            Raw::GatewayHeartbeatAck => {
                if let Some(latency) = self.shard.latency().recent().first() {
                    let micros = u64::try_from(latency.as_micros())
                        .unwrap_or(u64::MAX)
                        .max(1);
                    self.status.latency_micros.store(micros, Ordering::Relaxed);
                    gateway.heartbeat(*latency);
                }
            }
            _ => {}
        }
    }

    /// Sends a normal close frame and reads until Discord confirms it (at
    /// most 5 seconds). Events read meanwhile are not handed out.
    async fn close(&mut self) {
        self.shard.close(CloseFrame::NORMAL);
        let _ = tokio::time::timeout(CLOSE_WAIT, async {
            while let Some(item) = self.shard.next_event(EventTypeFlags::empty()).await {
                if matches!(item, Ok(Raw::GatewayClose(_))) {
                    break;
                }
            }
        })
        .await;
    }
}
