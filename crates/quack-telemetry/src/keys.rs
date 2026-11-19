//! Attribute keys that are not tied to a typed label.
//!
//! Semantic-convention keys come from `opentelemetry-semantic-conventions`;
//! Quack's own keys start with `quack.`.

use opentelemetry_semantic_conventions::attribute;

/// HTTP response status code (semconv), on HTTP metrics and spans.
pub const HTTP_RESPONSE_STATUS_CODE: &str = attribute::HTTP_RESPONSE_STATUS_CODE;
/// Database system (semconv); always `postgresql`.
pub const DB_SYSTEM_NAME: &str = attribute::DB_SYSTEM_NAME;
/// Service version (semconv), on `quack.build.info`.
pub const SERVICE_VERSION: &str = attribute::SERVICE_VERSION;

/// Discord guild id, on spans and logs only.
pub const GUILD_ID: &str = "quack.guild.id";
/// Discord user id of whoever triggered the work, on spans and logs only.
pub const ACTOR_ID: &str = "quack.actor.id";
/// Job id, on job spans only.
pub const JOB_ID: &str = "quack.job.id";
/// Job attempt number (1-based), on job spans only.
pub const JOB_ATTEMPT: &str = "quack.job.attempt";
/// A job's budget, on job spans only.
pub const JOB_MAX_ATTEMPTS: &str = "quack.job.max_attempts";
/// Discord interaction id, on interaction spans only.
pub const INTERACTION_ID: &str = "quack.interaction.id";
/// Seconds the job waited after it was ready, on job spans only.
pub const JOB_QUEUE_WAIT: &str = "quack.job.queue_wait";
/// Seconds spent waiting for a Discord rate limit, on Discord spans only.
pub const RATE_LIMIT_WAIT: &str = "quack.discord.rate_limit.wait";
