//! The bot: one value the binary builds and the gateway feeds.

use std::sync::Arc;
use std::time::Instant;

use quack_discord::gateway::Event;
use quack_discord::interaction::Interaction;
use quack_engine::Engine;
use quack_telemetry::labels::{Attribute as _, InteractionOutcome};
use quack_telemetry::{metrics, spans};
use tracing::Instrument as _;

use crate::ack::{Finish, Pending};
use crate::dedupe::Dedupe;
use crate::modules::Module;
use crate::respond::Responder;
use crate::{gateway, router};

/// What every handler shares.
pub(crate) struct Shared {
    /// The engine.
    pub(crate) engine: Engine,
    /// Where answers go.
    pub(crate) responder: Arc<dyn Responder>,
    /// The registered optional modules.
    pub(crate) modules: Vec<Arc<dyn Module>>,
    /// Interaction ids seen in the last 15 minutes.
    pub(crate) dedupe: Arc<Dedupe>,
    /// One channel creation per server at a time (§13.1.4).
    pub(crate) creating: tokio::sync::Mutex<()>,
    /// Bounds the work after acknowledgements ([`MAX_WORK`] at once), so a
    /// slow handler never holds up another interaction's acknowledgement.
    pub(crate) work: Arc<tokio::sync::Semaphore>,
}

/// Most interactions doing their work (after the acknowledgement) at once;
/// more wait, already acknowledged.
pub const MAX_WORK: usize = 64;

/// Turns Discord interactions and gateway events into engine calls and
/// replies (§11). Cheap to clone.
#[derive(Clone)]
pub struct Bot {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for Bot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bot")
            .field("modules", &self.shared.modules.len())
            .finish_non_exhaustive()
    }
}

impl Bot {
    /// A bot over `engine`, answering through `responder`, routing module
    /// components and events to `modules` (empty until the module crates
    /// are connected; see [`crate::modules`]).
    pub fn new(
        engine: Engine,
        responder: Arc<dyn Responder>,
        modules: Vec<Arc<dyn Module>>,
    ) -> Self {
        let dedupe = Arc::new(Dedupe::default());
        dedupe.observe();
        Self {
            shared: Arc::new(Shared {
                engine,
                responder,
                modules,
                dedupe,
                creating: tokio::sync::Mutex::new(()),
                work: Arc::new(tokio::sync::Semaphore::new(MAX_WORK)),
            }),
        }
    }

    /// Handles one gateway event (§11.9); interactions go to
    /// [`Bot::on_interaction`]. The gateway's dispatcher calls this off the
    /// shard loop, in a `gateway {event}` span, one server's events in
    /// order.
    pub async fn on_event(&self, event: Event) {
        match event {
            Event::Interaction(ix) => self.on_interaction(*ix).await,
            event => gateway::handle(&self.shared, &event).await,
        }
    }

    /// Handles one interaction (§11.3): drops a repeated delivery, then
    /// acknowledges, authorizes, runs the engine, and answers, in one
    /// `interaction {kind} {command}` root span.
    pub async fn on_interaction(&self, ix: Interaction) {
        let received = Instant::now();
        if !self.shared.dedupe.first(ix.id, received) {
            tracing::info!(
                interaction.id = ix.id,
                "dropped a repeated interaction delivery"
            );
            return;
        }
        let kind = ix.telemetry_kind();
        let command = ix.command_name();
        let span = spans::interaction(
            kind,
            command,
            ix.guild_id.map(quack_core::ids::GuildId::get),
            ix.invoker.as_ref().map_or(0, |p| p.account.id.get()),
            ix.id,
        );
        let finish = router::route(Pending::new(&ix, &self.shared, received))
            .instrument(span.clone())
            .await;
        let outcome = match finish {
            Finish::Done => InteractionOutcome::Done,
            Finish::Refused(reason) => {
                metrics().interactions.error(kind, command, reason);
                InteractionOutcome::Refused
            }
            Finish::Failed(reason) => {
                metrics().interactions.error(kind, command, reason);
                spans::record_error(&span, reason.as_str());
                InteractionOutcome::Failed
            }
        };
        spans::record(&span, outcome);
        metrics()
            .interactions
            .finished(kind, command, outcome, received.elapsed());
    }
}
