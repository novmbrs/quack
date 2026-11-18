//! The optional server activity log module (§13.3): posts Discord events
//! (edits, deletes, joins, leaves, bans, server and channel changes) to
//! staff-only channels. It is not a searchable archive and writes nothing
//! to the moderation log except its own settings changes.
//!
//! - **Bot (gateway, [`GATEWAY_EVENTS`]):** one [`ActivityLog`] per process;
//!   call its `on_*` handlers with decoded events (they read only in-memory
//!   caches and never wait) and [`ActivityLog::forget_guild`] when Quack
//!   leaves a server.
//! - **Binary:** [`ActivityLog::spawn`] starts the two sender tasks and the
//!   batch ticker; [`ActivityLog::shutdown`] flushes delete batches and
//!   drains the queue within the deadline.
//! - **API and `/setup`:** [`settings`], [`update_settings`], [`route_all`],
//!   and [`hooks`] for `quack_engine::settings::set_enabled`.
//!
//! Delivery is in memory and not durable, by design: a bounded queue with
//! counted drops and a few tries per post. Text is redacted before it is
//! cached or posted.

mod batches;
mod cache;
mod events;
mod intake;
mod log;
mod obs;
mod settings;

pub use events::{
    AuthorKind, BanChange, BanEntry, ChannelChange, FileFacts, GATEWAY_EVENTS, MemberJoined,
    MemberLeft, MessageCreated, MessageUpdated,
};
pub use log::{ActivityLog, Delivered, Limits, SENDERS};
pub use settings::{
    ActivitySettingsInput, ActivitySettingsView, hooks, route_all, settings, update_settings,
};
