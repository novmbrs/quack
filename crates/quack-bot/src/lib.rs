//! Quack's interaction and gateway event routing (§11).
//!
//! [`Bot`] turns decoded Discord interactions and gateway events into
//! engine calls and replies:
//!
//! - Every interaction is acknowledged first (a deferral, a modal, an
//!   autocomplete answer, or an immediate message) through one helper that
//!   records `quack.interaction.ack.duration`; the engine is reachable only
//!   after the ack, except the documented prefilled-modal read (§11.3).
//! - The invoker is authorized from the payload on every interaction,
//!   component clicks included, before any staff data is shown; staff
//!   answers are ephemeral.
//! - Engine errors become `quack-text` messages; unknown, expired, and
//!   Go-era custom ids get their designed answers.
//! - Core gateway events bootstrap, reconcile, and leave servers; module
//!   components and events go to registered [`Module`]s.

mod ack;
mod auth;
mod bot;
mod card;
mod dedupe;
mod errors;
mod forms;
mod gateway;
mod handlers;
pub mod modules;
mod respond;
mod router;

pub use bot::Bot;
pub use modules::{ComponentCall, Module};
pub use respond::{CommandMentions, DiscordResponder, Responder};
