//! Tests over every message: snapshots, Discord limits, the glossary lint,
//! "couldn't" versus "couldn't confirm", escaping, and catalog coverage.

#![allow(unreachable_pub, reason = "test helpers shared between test modules")]

mod card_snapshots;
mod coverage;
mod escape;
mod fixtures;
mod lint;
mod samples;
mod snapshots;
