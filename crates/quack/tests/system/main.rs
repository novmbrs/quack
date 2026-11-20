//! Whole-system tests: the composed application on PostgreSQL 18 with the
//! fake Discord port, driven through gateway events, interactions, HTTP,
//! and the job queue (see `common`).

#![allow(
    unreachable_pub,
    missing_docs,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    reason = "test stories read top to bottom and fail loudly"
)]

mod activity;
#[path = "../support/telemetry.rs"]
mod capture;
mod common;
mod dm_failed;
mod evidence;
mod honeypot;
mod moderation;
mod shutdown;
mod telemetry;
mod tickets;
