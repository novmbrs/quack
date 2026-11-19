//! Span constructors for each unit of work, with their standard attributes.
//!
//! Each constructor returns a `tracing` span whose exported name follows
//! the design (§18.4: `interaction {kind} {command}`, `job {kind}`,
//! `{method} {route}`, …) and whose attributes follow OpenTelemetry
//! conventions where one exists. Ids are
//! recorded as strings so trace UIs show snowflakes exactly. Enter the span
//! (or `.instrument(span)` a future) to make it the parent of nested work.

use std::fmt::Display;
use std::time::Duration;

use opentelemetry::KeyValue;
use opentelemetry::trace::Status;
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt as _;

use crate::keys;
use crate::labels::{
    Attribute, CommandName, DbCollection, DbOperation, GatewayEvent, HttpMethod, HttpRoute,
    InteractionKind, JobKind, UrlTemplate,
};
use crate::propagation::parse_trace_context;

/// A domain event recorded on a span.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpanEvent {
    /// The interaction was acknowledged.
    Acked,
    /// The final reply was sent.
    Replied,
    /// The interaction was answered with a refusal.
    Refused,
    /// A case was committed.
    CaseCreated,
    /// A case was voided.
    CaseVoided,
    /// An action attempt finished.
    ActionAttempt,
    /// A DM reached its final state.
    DmOutcome,
    /// An appeal changed state.
    AppealTransition,
    /// Evidence was saved.
    EvidenceCaptured,
    /// A tracked message was brought up to date.
    MessageSynced,
    /// A moderation-log entry was written.
    ModLogEntry,
    /// A call waited for a rate limit.
    RateLimited,
}

impl SpanEvent {
    /// The exported event name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Acked => "acked",
            Self::Replied => "replied",
            Self::Refused => "refused",
            Self::CaseCreated => "case.created",
            Self::CaseVoided => "case.voided",
            Self::ActionAttempt => "action.attempt",
            Self::DmOutcome => "dm.outcome",
            Self::AppealTransition => "appeal.transition",
            Self::EvidenceCaptured => "evidence.captured",
            Self::MessageSynced => "message.synced",
            Self::ModLogEntry => "mod_log.entry",
            Self::RateLimited => "rate_limited",
        }
    }
}

/// The root span for one Discord interaction. Record
/// `quack.interaction.outcome` with [`record`] at the end.
pub fn interaction(
    kind: InteractionKind,
    command: CommandName,
    guild_id: Option<u64>,
    actor_id: u64,
    interaction_id: u64,
) -> Span {
    tracing::info_span!(
        parent: None,
        "interaction",
        otel.name = %format_args!("interaction {kind} {command}"),
        otel.kind = "server",
        quack.interaction.kind = kind.as_str(),
        quack.command = command.as_str(),
        quack.guild.id = guild_id.map(display),
        quack.actor.id = %actor_id,
        quack.interaction.id = %interaction_id,
    )
}

/// The span for handling one gateway event.
pub fn gateway(event: GatewayEvent, guild_id: Option<u64>) -> Span {
    tracing::info_span!(
        "gateway",
        otel.name = %format_args!("gateway {event}"),
        otel.kind = "consumer",
        quack.gateway.event = event.as_str(),
        quack.guild.id = guild_id.map(display),
    )
}

/// The server span for one API request (`{method} {route}`, a new root:
/// an incoming `traceparent` is ignored). Record the status with
/// [`record_status`] when the response is ready.
pub fn http(method: HttpMethod, route: HttpRoute) -> Span {
    tracing::info_span!(
        parent: None,
        "http",
        otel.name = %format_args!("{method} {route}"),
        otel.kind = "server",
        http.request.method = method.as_str(),
        http.route = route.as_str(),
    )
}

/// What a job span records about the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobRun<'a, Id> {
    /// The job's kind.
    pub kind: JobKind,
    /// The job's id.
    pub job_id: Id,
    /// This run's attempt number (1-based).
    pub attempt: u32,
    /// The job's budget.
    pub max_attempts: u32,
    /// How long the job was ready before this run started.
    pub queue_wait: Duration,
    /// The job's server; `None` for housekeeping.
    pub guild_id: Option<u64>,
    /// The stored trace context of whoever enqueued it (see
    /// [`current_trace_context`](crate::current_trace_context)).
    pub enqueued_by: Option<&'a str>,
}

/// The span for one run of a job: a new trace root, linked to the trace that
/// enqueued it when the run carries a stored trace context. Record
/// `quack.job.outcome` with [`record`] at the end.
pub fn job<Id: Display>(run: &JobRun<'_, Id>) -> Span {
    let kind = run.kind;
    let span = tracing::info_span!(
        parent: None,
        "job",
        otel.name = %format_args!("job {kind}"),
        otel.kind = "consumer",
        messaging.operation.type = "process",
        quack.job.kind = kind.as_str(),
        quack.job.id = %run.job_id,
        quack.job.attempt = i64::from(run.attempt),
        quack.job.max_attempts = i64::from(run.max_attempts),
        quack.job.queue_wait = run.queue_wait.as_secs_f64(),
        quack.guild.id = run.guild_id.map(display),
    );
    let enqueued_by = run.enqueued_by;
    if let Some(context) = enqueued_by.and_then(parse_trace_context) {
        span.add_link(context);
    }
    span
}

/// The span for one engine use case (`use_case {name}`, e.g.
/// `use_case apply_rule`), the parent of its queries and Discord calls and
/// the span its domain events go on. `name` is a static name chosen in
/// code.
pub fn use_case(name: &'static str, guild_id: Option<u64>) -> Span {
    tracing::info_span!(
        "use_case",
        otel.name = %format_args!("use_case {name}"),
        otel.kind = "internal",
        quack.guild.id = guild_id.map(display),
    )
}

/// The client span for one PostgreSQL query (`{operation} {table}`, e.g.
/// `UPDATE case_counters`).
pub fn db(operation: DbOperation, table: DbCollection) -> Span {
    tracing::info_span!(
        "db",
        otel.name = %format_args!("{operation} {table}"),
        otel.kind = "client",
        db.system.name = "postgresql",
        db.operation.name = operation.as_str(),
        db.collection.name = table.as_str(),
        db.query.summary = %format_args!("{operation} {table}"),
    )
}

/// The client span for one Discord REST call (`{method} {url.template}`,
/// e.g. `PUT /guilds/{guild_id}/bans/{user_id}`). Record the status with
/// [`record_status`], the outcome with [`record`], and any rate-limit wait
/// with [`record_rate_limit_wait`].
pub fn discord(method: HttpMethod, route: UrlTemplate) -> Span {
    tracing::info_span!(
        "discord",
        otel.name = %format_args!("{method} {route}"),
        otel.kind = "client",
        http.request.method = method.as_str(),
        url.template = route.as_str(),
        server.address = "discord.com",
    )
}

/// Sets a typed attribute (an outcome, a reason) on `span`.
pub fn record<A: Attribute>(span: &Span, value: A) {
    span.set_attribute(A::KEY, value.as_str());
}

/// Sets `http.response.status_code` on `span`.
pub fn record_status(span: &Span, status: u16) {
    span.set_attribute(keys::HTTP_RESPONSE_STATUS_CODE, i64::from(status));
}

/// Sets how long `span` waited for a Discord rate limit, in seconds.
pub fn record_rate_limit_wait(span: &Span, wait: Duration) {
    span.set_attribute(keys::RATE_LIMIT_WAIT, wait.as_secs_f64());
}

/// Marks `span` as failed with a short, id-free description.
pub fn record_error(span: &Span, description: impl Into<String>) {
    span.set_status(Status::error(description.into()));
}

/// Adds a domain event to `span`. Events stay on the trace; use `tracing`
/// events for things that should also be logs.
pub fn event(span: &Span, event: SpanEvent, attributes: Vec<KeyValue>) {
    span.add_event(event.as_str(), attributes);
}

#[cfg(test)]
mod tests;
