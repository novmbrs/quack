//! The optional tickets module (§13.2): private support threads between a
//! member and staff.
//!
//! Plain service functions over the [`quack_engine::Engine`], called with
//! already-decoded inputs:
//!
//! - **Bot (buttons):** [`open`] ("Open a ticket", `to`), [`close`]
//!   ("Close ticket", `tc`), [`fix_access`] ("Restore private access",
//!   `tf`). Each answer has a `text()` for the ephemeral reply.
//! - **Bot (gateway, [`GATEWAY_EVENTS`]):** [`Journal::on_message`] for
//!   `MESSAGE_CREATE`, [`Journal::on_event_dropped`] when the bot's queue
//!   drops an event, [`on_thread_deleted`], [`on_guild_create`], and
//!   [`on_invalid_session`].
//! - **API:** [`settings`], [`update_settings`], [`list`], [`get`],
//!   [`transcript`], and [`close`].
//! - **Binary:** [`jobs`] registers the `open_ticket` and `close_ticket`
//!   handlers with the runner; [`hooks`] is passed to
//!   `quack_engine::settings::set_enabled`; [`OpenGauge`] reports
//!   `quack.tickets.open`.
//!
//! Every non-closed ticket has a job with a budget whose finalization moves
//! it on, so a member's one live slot is always freed.

mod access;
mod close;
mod close_job;
mod error;
mod gateway;
mod gauge;
mod jobs;
mod obs;
mod open;
mod open_job;
mod read;
mod settings;
mod transcript;

pub use access::{FixAnswer, fix_access};
pub use close::{CloseAnswer, close};
pub use error::TicketError;
pub use gateway::{
    GATEWAY_EVENTS, Journal, JournalEntry, JournalFileEntry, on_guild_create, on_invalid_session,
    on_thread_deleted,
};
pub use gauge::OpenGauge;
pub use jobs::jobs;
pub use open::{OpenAnswer, open};
pub use read::{TicketView, TranscriptSummary, TranscriptView, get, list, transcript};
pub use settings::{TicketSettingsInput, TicketSettingsView, hooks, settings, update_settings};
