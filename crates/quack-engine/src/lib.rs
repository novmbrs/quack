//! Quack's use cases and job handlers.
//!
//! Applying a rule, voiding, retrying and undoing actions, evidence, the case
//! DM, appeals, the moderation log and log channel, settings, statistics, and
//! `/case add` drafts. It defines the [`Discord`] port, the only boundary that
//! tests replace, and talks to PostgreSQL through `quack-db` directly.
//!
//! Every use case is a plain async function taking the [`Engine`], the
//! acting [`Principal`] (or member), and typed facts. Every job handler
//! implements its kind's step list from the design literally: it records
//! that an effect started before calling Discord, reads Discord before
//! acting where the design says so, resolves each port [`Outcome`] through
//! `quack-core`'s transition functions, and writes only by compare-and-set
//! inside fenced transactions.

pub mod actions;
pub mod cache;
pub mod cases;
mod channels;
pub mod dms;
mod engine;
pub mod evidence;
pub mod jobs;
pub mod messages;
pub mod modlog;
mod obs;
mod opkey;
pub mod port;
mod principal;

pub use cases::CaseRef;
pub use engine::{Engine, EngineConfig, Links};
pub use opkey::OpKey;
pub use port::{BoxFuture, Discord, Outcome};
pub use principal::{Origin, Principal, PrincipalError};
