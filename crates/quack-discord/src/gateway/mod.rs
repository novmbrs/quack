//! The gateway runner (§11.9): one shard with the design's intents,
//! automatic resume and reconnect (Twilight's), caches updated in the shard
//! loop, and bounded dispatch of typed [`Event`]s to a handler.
//!
//! The shard loop never waits on a handler. Connection state and heartbeat
//! latency are exposed through [`GatewayStatus`] for readiness and metrics.

mod dispatch;
mod event;

#[cfg(test)]
mod tests;

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use quack_telemetry::labels::GatewayConnectionEvent;
use quack_telemetry::metrics;
use tokio_util::sync::CancellationToken;
use twilight_gateway::{
    CloseFrame, Config, Event as Raw, EventTypeFlags, Intents, Shard, ShardId, StreamExt as _,
};

use twilight_gateway::error::ReceiveMessageErrorType;

use crate::cache::{GuildCache, MemberCache, feed_members};

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

/// How long shutdown waits for Discord to confirm the close.
const CLOSE_WAIT: Duration = Duration::from_secs(5);

/// The events Twilight deserializes; everything else is skipped cheaply.
pub(crate) fn wanted() -> EventTypeFlags {
    EventTypeFlags::READY
        | EventTypeFlags::RESUMED
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
    /// Another close code Twilight does not reconnect after (for example
    /// 4004, authentication failed).
    #[error("Discord closed the gateway and Quack can't reconnect (close code {0:?})")]
    Closed(Option<u16>),
}

/// The gateway's connection state, shared with readiness checks.
#[derive(Debug, Default)]
pub struct GatewayStatus {
    connected: AtomicBool,
    latency_micros: AtomicU64,
}

impl GatewayStatus {
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
                        code => GatewayError::Closed(code),
                    });
                }
                Some(Err(error)) => {
                    if let Some(event) = recover(&error) {
                        hand_off(&dispatcher, event, &on_dropped);
                    } else {
                        tracing::warn!(%error, "couldn't read a gateway event");
                    }
                    continue;
                }
                Some(Ok(raw)) => raw,
            };
            if let Raw::GatewayClose(frame) = &raw {
                close_code = frame.as_ref().map(|f| f.code);
            }
            self.track(&raw);
            if let Some(event) = event::map(&raw, sessions) {
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
        dispatcher.stop().await;
        result
    }

    /// Updates the caches and the connection state for one event.
    fn track(&self, raw: &Raw) {
        self.guilds.update(raw);
        feed_members(&self.members, raw);
        let gateway = &metrics().gateway;
        match raw {
            Raw::Ready(_) => {
                self.status.set_connected(true);
                gateway.connection(GatewayConnectionEvent::Identify);
            }
            Raw::Resumed => {
                self.status.set_connected(true);
                gateway.connection(GatewayConnectionEvent::Resume);
            }
            Raw::GatewayReconnect | Raw::GatewayClose(_) => {
                self.status.set_connected(false);
                gateway.connection(GatewayConnectionEvent::Reconnect);
            }
            Raw::GatewayInvalidateSession(_) => {
                self.status.set_connected(false);
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
