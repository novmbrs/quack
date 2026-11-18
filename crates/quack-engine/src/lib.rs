//! Quack's use cases and job handlers.
//!
//! Applying a rule, voiding, retrying and undoing actions, evidence, the case
//! DM, appeals, the moderation log and log channel, settings, statistics, and
//! `/case add` drafts. It defines the `Discord` port, the only boundary that
//! tests replace, and talks to PostgreSQL through `quack-db` directly.
