//! Quack's Discord adapter, built on Twilight 0.18.
//!
//! - [`DiscordClient`]: the REST client. Every method returns a typed
//!   [`Outcome`] (done, refused with a reason, retry after, not sent, or
//!   unknown), waits for Twilight's rate limiter only within a budget, and
//!   never retries on its own.
//! - [`render`]: the neutral `quack_text::Msg` → Components V2, and modals.
//! - [`custom_id`]: the versioned custom-id scheme, legacy ids included.
//! - [`interaction`]: a raw interaction → one typed [`interaction::Interaction`].
//! - [`gateway`]: the shard runner with bounded dispatch, typed events, and
//!   connection state.
//! - [`cache`]: the guild cache and the bounded member cache, producing
//!   quack-core facts; [`permissions`] computes Discord permissions.
//! - [`commands`]: command definitions and registration.
//! - [`facts`]: server, member, and channel facts from the caches or REST
//!   under the freshness rules of §2.
//!
//! The engine's `Discord` port is implemented in the binary on top of this
//! crate, because this crate must not depend on `quack-engine` (§17.2).

pub mod cache;
pub mod commands;
pub mod custom_id;
pub mod facts;
pub mod gateway;
pub mod interaction;
pub mod model;
pub mod permissions;
pub mod render;
pub mod rest;

mod convert;

#[cfg(test)]
mod fixtures;

pub use rest::{
    ClientConfig, ClientError, DiscordClient, NotSentCause, Outcome, Refusal, RefusalKind,
    UnknownCause,
};
