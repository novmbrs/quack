//! Test support for Quack's crates; used only as a dev-dependency.
//!
//! - [`fake::FakeDiscord`]: a fake `Discord` port with in-memory servers,
//!   members, bans, channels, and messages; every call recorded; any
//!   call's outcome scriptable, including "the call takes effect but its
//!   answer is lost" and "the process dies right here".
//! - [`db::TestDb`]: a fresh, migrated PostgreSQL database per test.
//! - [`Harness`]: one standard server and its people, an engine over the
//!   fake and the database, fixtures, and a deterministic driver that runs
//!   the job queue to quiescence ([`Harness::settle`]).

pub mod db;
pub mod fake;
mod harness;
mod queue;

pub use harness::{
    ADMIN, GUILD, Harness, MEMBER, MEMBER_2, MODERATOR, MODERATOR_2, OWNER, QUACK,
    quack_permissions, rules_for_tests, sf,
};
pub use queue::{
    ENGINE_KINDS, Ran, RunEnd, claim_with, expire_leases_with, fast_forward_with, run_claimed,
    run_claimed_with, run_jobs_with, settle_with, waiting_jobs_with,
};
