//! The seam where the optional modules plug in (§13.1.6).
//!
//! Each module crate (`quack-tickets`, `quack-activity-log`,
//! `quack-honeypot`) exports plain service functions over the engine; the
//! adapters here implement [`Module`] for each by converting the gateway
//! [`Event`] and the component call into those functions' inputs. The
//! binary builds them with [`tickets`], [`honeypot`], and [`activity_log`]
//! and passes them to [`crate::Bot::new`]; the router sends a module's
//! buttons and the gateway events it wants to it. The module areas of
//! `/setup` are part of the setup panel itself
//! (`handlers::setup_modules`), which calls the same crates.
//!
//! Without a registered module its buttons answer
//! `interaction.expired_button`.

mod activity_log;
mod honeypot;
mod tickets;

pub use activity_log::activity_log;
pub use honeypot::honeypot;
pub use tickets::{ThreadLookup, TicketGaps, tickets};

use quack_core::ids::{AccountId, ChannelId, GuildId};
use quack_core::modules::ModuleName;
use quack_discord::custom_id::CustomId;
use quack_discord::gateway::Event;
use quack_discord::interaction::{FieldValue, SelectChoice, Source};
use quack_engine::{BoxFuture, Engine, Principal};
use quack_text::Msg;
use quack_text::actions::InputId;

use crate::ack::Failure;

/// A module's button, select, or modal submit, after the ack (§13.1.6
/// `ComponentCall`).
#[derive(Clone, Debug)]
pub struct ComponentCall<'a> {
    /// The server, when the click happened in one.
    pub guild: Option<GuildId>,
    /// Where it happened.
    pub channel: Option<ChannelId>,
    /// Who clicked.
    pub actor: AccountId,
    /// The clicker as a principal built from the payload (§2), when the
    /// click happened in a server.
    pub principal: Option<Principal>,
    /// The interaction id: the operation key of any write it makes.
    pub interaction_id: u64,
    /// The decoded custom id.
    pub id: CustomId,
    /// The chosen values of a select.
    pub values: &'a [SelectChoice],
    /// A modal's submitted values.
    pub fields: &'a [(InputId, FieldValue)],
    /// Where the component was.
    pub source: Source,
}

/// One optional module as the bot routes to it. Routing is static: what a
/// module owns never depends on state.
pub trait Module: Send + Sync + 'static {
    /// The module.
    fn name(&self) -> ModuleName;
    /// Whether this custom id is one of its components (only tickets has
    /// any).
    fn owns(&self, _id: &CustomId) -> bool {
        false
    }
    /// Whether it wants this gateway event.
    fn wants(&self, event: &Event) -> bool;
    /// Handles a component or modal after the ack and returns the answer
    /// or the refusal. The module authorizes the clicker itself (§2).
    fn on_component<'a>(
        &'a self,
        _engine: &'a Engine,
        _call: ComponentCall<'a>,
    ) -> BoxFuture<'a, Result<Msg, Failure>> {
        Box::pin(async { Err(crate::router::expired()) })
    }
    /// Handles one gateway event it wants, after the core handling.
    fn on_event<'a>(&'a self, engine: &'a Engine, event: &'a Event) -> BoxFuture<'a, ()>;
}

/// Whether a custom id belongs to a module by its code (the ticket
/// buttons), whether or not a module is registered.
pub(crate) const fn is_module_id(id: &CustomId) -> bool {
    matches!(
        id,
        CustomId::OpenTicket | CustomId::CloseTicket(_) | CustomId::FixTicketAccess(_)
    )
}
