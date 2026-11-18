//! Quack's pure moderation domain.
//!
//! Ids, the glossary types (rule, level, case, action, outcome, appeal), rule
//! definitions and their validation, escalation, the permission model and
//! target checks, the case/action/DM/appeal state machines, moderation-log
//! entry kinds, limits, and domain errors. No I/O, no async, no SQL, and no
//! Discord types, so everything here is cheap to test exhaustively.
