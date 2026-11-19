//! The optional honeypot module (§13.4): a trap channel where a post from
//! a member who is not staff adds a case under the honeypot's rule through
//! the normal case path.
//!
//! Plain service functions over the [`quack_engine::Engine`]:
//!
//! - **Bot (gateway, [`GATEWAY_EVENTS`]):** [`on_message`] for every
//!   `MESSAGE_CREATE` (it returns at once, without a query, outside the
//!   honeypot channel).
//! - **API and `/setup`:** [`settings`], [`update_settings`],
//!   [`use_default_rule`] ("Turn on" with no rule), and [`hooks`] for
//!   `quack_engine::settings::set_enabled`.
//! - **Binary:** [`jobs`] registers `honeypot_case` and `honeypot_cleanup`
//!   with the runner; [`recover`] runs once at startup.
//!
//! Hits are durable rows inserted with their job, so a restart loses none;
//! a burst from one member becomes one case; a post is deleted only after
//! the case saved it as evidence.

mod case_job;
mod cleanup_job;
mod intake;
mod jobs;
mod settings;

pub use intake::{GATEWAY_EVENTS, Intake, PostFacts, on_message, recover};
pub use jobs::jobs;
pub use settings::{
    HoneypotSettingsInput, HoneypotSettingsView, hooks, settings, update_settings, use_default_rule,
};
