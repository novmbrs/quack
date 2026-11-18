//! Quack's pure moderation domain.
//!
//! Ids, the glossary types (rule, level, case, action, outcome, appeal), rule
//! definitions and their validation, escalation, the permission model and
//! member checks, the case/action/DM/appeal/evidence/tracked-message state
//! machines, moderation log entry kinds, limits, and domain errors. No I/O,
//! no async, no SQL, no Discord library types, and no clock or randomness:
//! callers pass the current time and freshly generated ids in, so everything
//! here is cheap to test exhaustively.

#[macro_use]
mod macros;

pub mod action;
mod error;
pub mod ids;
pub mod limits;
pub mod permissions;

pub use error::{DomainError, InvalidTransition, UnknownValue};
