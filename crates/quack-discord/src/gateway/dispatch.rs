//! Bounded hand-off from the shard loop to handlers (§11.9).
//!
//! Interactions go to their own queue, drained into at most 256 concurrent
//! handler tasks (the bot bounds the work after each acknowledgement on
//! its own, so a slow handler never holds up another's acknowledgement).
//! Server lifecycle events (`READY`, `GUILD_CREATE`, `GUILD_DELETE`,
//! `CHANNEL_DELETE`) have their own ordered queue of 4,096, so a flood of
//! messages never drops them. Other events go to 4 queues of 1,024 chosen by server id,
//! each handled in order, so one server's events stay ordered. The shard
//! loop never waits: a full queue drops the event and counts
//! `quack.queue.dropped`.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

use quack_telemetry::labels::{GatewayEvent, QueueName};
use quack_telemetry::{metrics, spans};
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;
use tracing::Instrument as _;

use super::event::Event;

/// How many ordered event queues there are.
pub const EVENT_QUEUES: usize = 4;
/// Each event queue's bound.
pub const EVENT_QUEUE_LEN: usize = 1_024;
/// The lifecycle queue's bound: room for a `GUILD_CREATE` from every
/// server one shard can hold, and more.
pub(crate) const LIFECYCLE_QUEUE_LEN: usize = 4_096;
/// The interaction queue's bound.
pub const INTERACTION_QUEUE_LEN: usize = 256;
/// Most interaction handlers in flight at once, acknowledgement and work
/// together; the bot runs at most 64 of them past their acknowledgement.
pub const MAX_INTERACTION_HANDLERS: usize = 256;

const EVENTS: QueueName = QueueName::new("gateway.events");
const LIFECYCLE: QueueName = QueueName::new("gateway.lifecycle");
const INTERACTIONS: QueueName = QueueName::new("gateway.interactions");

/// One bounded queue with a depth counter (a tokio sender can't report its
/// length without keeping the channel open).
struct Queue {
    tx: mpsc::Sender<Event>,
    depth: Arc<AtomicUsize>,
}

pub(crate) struct Dispatcher {
    events: Vec<Queue>,
    lifecycle: Queue,
    interactions: Queue,
    stopping: Arc<AtomicBool>,
    consumers: JoinSet<()>,
}

fn queue(len: usize) -> (Queue, mpsc::Receiver<Event>, Arc<AtomicUsize>) {
    let (tx, rx) = mpsc::channel(len);
    let depth = Arc::new(AtomicUsize::new(0));
    (
        Queue {
            tx,
            depth: depth.clone(),
        },
        rx,
        depth,
    )
}

impl Dispatcher {
    /// Starts the consumer tasks. Must run inside a Tokio runtime.
    pub(crate) fn start<H, F>(handler: Arc<H>) -> Self
    where
        H: Fn(Event) -> F + Send + Sync + 'static,
        F: Future<Output = ()> + Send + 'static,
    {
        let stopping = Arc::new(AtomicBool::new(false));
        let mut consumers = JoinSet::new();
        let mut events = Vec::with_capacity(EVENT_QUEUES);
        let mut depths = Vec::with_capacity(EVENT_QUEUES);
        for _ in 0..EVENT_QUEUES {
            let (q, rx, depth) = queue(EVENT_QUEUE_LEN);
            consumers.spawn(ordered(
                rx,
                handler.clone(),
                depth.clone(),
                stopping.clone(),
            ));
            depths.push(depth);
            events.push(q);
        }
        let (lifecycle, rx, lifecycle_depth) = queue(LIFECYCLE_QUEUE_LEN);
        consumers.spawn(ordered(
            rx,
            handler.clone(),
            lifecycle_depth.clone(),
            stopping.clone(),
        ));
        metrics().queues.observe_depth(LIFECYCLE, move || {
            lifecycle_depth.load(Ordering::Relaxed) as u64
        });
        let (interactions, rx, interaction_depth) = queue(INTERACTION_QUEUE_LEN);
        consumers.spawn(pooled(
            rx,
            handler,
            interaction_depth.clone(),
            stopping.clone(),
        ));
        metrics().queues.observe_depth(EVENTS, move || {
            depths
                .iter()
                .map(|d| d.load(Ordering::Relaxed) as u64)
                .sum()
        });
        metrics().queues.observe_depth(INTERACTIONS, move || {
            interaction_depth.load(Ordering::Relaxed) as u64
        });
        Self {
            events,
            lifecycle,
            interactions,
            stopping,
            consumers,
        }
    }

    /// Queues an event without waiting; gives it back when its queue is
    /// full (it was dropped and counted).
    pub(crate) fn dispatch(&self, event: Event) -> Option<Event> {
        let (q, name) = if matches!(event, Event::Interaction(_)) {
            (&self.interactions, INTERACTIONS)
        } else if matches!(
            event,
            Event::Ready { .. }
                | Event::GuildCreate { .. }
                | Event::GuildDelete { .. }
                | Event::ChannelDelete(_)
        ) {
            (&self.lifecycle, LIFECYCLE)
        } else {
            let index = event.guild_id().map_or(0, |g| {
                usize::try_from(g.get() % EVENT_QUEUES as u64).unwrap_or(0)
            });
            match self.events.get(index) {
                Some(q) => (q, EVENTS),
                None => return Some(event),
            }
        };
        q.depth.fetch_add(1, Ordering::Relaxed);
        match q.tx.try_send(event) {
            Ok(()) => None,
            Err(error) => {
                q.depth.fetch_sub(1, Ordering::Relaxed);
                metrics().queues.dropped(name, 1);
                Some(match error {
                    mpsc::error::TrySendError::Full(e) | mpsc::error::TrySendError::Closed(e) => e,
                })
            }
        }
    }

    /// Stops handing out events: queued events are dropped (and counted),
    /// in-flight handlers finish.
    pub(crate) async fn stop(mut self) {
        self.stopping.store(true, Ordering::Release);
        drop(self.events);
        drop(self.lifecycle);
        drop(self.interactions);
        while self.consumers.join_next().await.is_some() {}
    }
}

/// Runs one queue's events in order, each in its own task so a panicking
/// handler is contained.
async fn ordered<H, F>(
    mut rx: mpsc::Receiver<Event>,
    handler: Arc<H>,
    depth: Arc<AtomicUsize>,
    stopping: Arc<AtomicBool>,
) where
    H: Fn(Event) -> F + Send + Sync + 'static,
    F: Future<Output = ()> + Send + 'static,
{
    while let Some(event) = rx.recv().await {
        depth.fetch_sub(1, Ordering::Relaxed);
        if stopping.load(Ordering::Acquire) {
            metrics().queues.dropped(EVENTS, 1);
            continue;
        }
        let name = GatewayEvent::new(event.name());
        let span = spans::gateway(name, event.guild_id().map(quack_core::ids::GuildId::get));
        let started = Instant::now();
        let run = tokio::spawn(handler(event).instrument(span));
        if run.await.is_err() {
            tracing::error!(event = name.to_string(), "a gateway event handler panicked");
        }
        metrics().gateway.event(name, started.elapsed());
    }
}

/// Runs interactions concurrently, at most [`MAX_INTERACTION_HANDLERS`].
async fn pooled<H, F>(
    mut rx: mpsc::Receiver<Event>,
    handler: Arc<H>,
    depth: Arc<AtomicUsize>,
    stopping: Arc<AtomicBool>,
) where
    H: Fn(Event) -> F + Send + Sync + 'static,
    F: Future<Output = ()> + Send + 'static,
{
    let limit = Arc::new(Semaphore::new(MAX_INTERACTION_HANDLERS));
    let mut running = JoinSet::new();
    while let Some(event) = rx.recv().await {
        depth.fetch_sub(1, Ordering::Relaxed);
        if stopping.load(Ordering::Acquire) {
            metrics().queues.dropped(INTERACTIONS, 1);
            continue;
        }
        let Ok(permit) = limit.clone().acquire_owned().await else {
            break;
        };
        let handle = handler(event);
        running.spawn(async move {
            handle.await;
            drop(permit);
        });
        while running.try_join_next().is_some() {}
    }
    while running.join_next().await.is_some() {}
}
