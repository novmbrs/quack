//! The seam where the optional modules plug in (§13.1.6).
//!
//! **Integration point.** The module crates (`quack-tickets`,
//! `quack-activity-log`, `quack-honeypot`) are connected by implementing
//! [`Module`] for each crate's module value (a thin adapter, because the
//! modules see engine types, not Twilight's) and passing them to
//! [`crate::Bot::new`]. The router then sends a module's component and
//! modal custom ids, its `/setup` area's controls, and the gateway events it
//! wants to it; the binary registers its job kinds with
//! `quack_engine::jobs::handler` (see the binary's `wiring::module_jobs`).
//! Until then no module is registered, and their buttons answer
//! `interaction.expired_button`.

use quack_core::ids::{AccountId, ChannelId, GuildId};
use quack_core::modules::ModuleName;
use quack_discord::custom_id::CustomId;
use quack_discord::gateway::Event;
use quack_discord::interaction::{FieldValue, SelectChoice, Source};
use quack_engine::{BoxFuture, Engine};
use quack_text::Msg;
use quack_text::actions::{InputId, SetupArea};
use quack_text::model::Modal;

/// A module's button, select, or modal submit, after the ack (§13.1.6
/// `ComponentCall`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComponentCall<'a> {
    /// The server, when the click happened in one.
    pub guild: Option<GuildId>,
    /// Where it happened.
    pub channel: Option<ChannelId>,
    /// Who clicked.
    pub actor: AccountId,
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
    /// Its `/setup` area; that area's controls are routed to it.
    fn setup_area(&self) -> SetupArea;
    /// Whether this custom id is one of its components (besides its setup
    /// area's controls).
    fn owns(&self, id: &CustomId) -> bool;
    /// Whether it wants this gateway event.
    fn wants(&self, event: &Event) -> bool;
    /// Handles a component or modal after the ack and returns the answer.
    /// The module authorizes the clicker itself (§2).
    fn on_component<'a>(
        &'a self,
        engine: &'a Engine,
        call: ComponentCall<'a>,
    ) -> BoxFuture<'a, Msg>;
    /// A modal prefilled with stored values (§11.3's exception), or `None`
    /// when `id` doesn't open one. The read must authorize the actor and
    /// fit in 300 ms; `Err` is the refusal shown instead.
    fn prefilled_modal<'a>(
        &'a self,
        engine: &'a Engine,
        guild: GuildId,
        actor: AccountId,
        id: &'a CustomId,
    ) -> Option<BoxFuture<'a, Result<Modal, quack_text::Text>>>;
    /// Handles one gateway event it wants, after the core handling.
    fn on_event<'a>(&'a self, engine: &'a Engine, event: &'a Event) -> BoxFuture<'a, ()>;
}

/// The modules setup areas belong to.
pub(crate) const fn is_module_area(area: SetupArea) -> bool {
    matches!(
        area,
        SetupArea::Tickets | SetupArea::Honeypot | SetupArea::ActivityLog
    )
}

/// Whether a custom id belongs to a module by its code (ticket buttons and
/// the module setup areas), whether or not a module is registered.
pub(crate) const fn is_module_id(id: &CustomId) -> bool {
    match id {
        CustomId::OpenTicket
        | CustomId::CloseTicket(_)
        | CustomId::FixTicketAccess(_)
        | CustomId::SetupWarningModal => true,
        CustomId::Setup(area, _) => is_module_area(*area),
        _ => false,
    }
}
