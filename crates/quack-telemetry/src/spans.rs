//! Span constructors for each unit of work, with their standard attributes.
//!
//! Each constructor returns a `tracing` span whose exported name follows
//! the charter (`interaction {kind} {command}`, `job {kind}`, …) and whose
//! attributes follow OpenTelemetry conventions where one exists. Ids are
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
    /// A case was committed.
    CaseCreated,
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
    /// A call waited for a rate limit.
    RateLimited,
}

impl SpanEvent {
    /// The exported event name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Acked => "acked",
            Self::Replied => "replied",
            Self::CaseCreated => "case.created",
            Self::ActionAttempt => "action.attempt",
            Self::DmOutcome => "dm.outcome",
            Self::AppealTransition => "appeal.transition",
            Self::EvidenceCaptured => "evidence.captured",
            Self::MessageSynced => "message.synced",
            Self::RateLimited => "rate_limited",
        }
    }
}

/// The root span for one Discord interaction.
pub fn interaction(
    kind: InteractionKind,
    command: CommandName,
    guild_id: Option<u64>,
    actor_id: u64,
) -> Span {
    tracing::info_span!(
        "interaction",
        otel.name = %format_args!("interaction {kind} {command}"),
        otel.kind = "server",
        quack.interaction.kind = kind.as_str(),
        quack.command = command.as_str(),
        quack.guild.id = guild_id.map(display),
        quack.actor.id = %actor_id,
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

/// The server span for one API request. Record the status with
/// [`record_status`] when the response is ready.
pub fn http(method: HttpMethod, route: HttpRoute) -> Span {
    tracing::info_span!(
        "http",
        otel.name = %format_args!("http {method} {route}"),
        otel.kind = "server",
        http.request.method = method.as_str(),
        http.route = route.as_str(),
    )
}

/// The span for one run of a job: a new trace root, linked to the trace that
/// enqueued it when `enqueued_by` holds a stored trace context (see
/// [`current_trace_context`](crate::current_trace_context)).
pub fn job(
    kind: JobKind,
    job_id: impl Display,
    attempt: u32,
    queue_wait: Duration,
    enqueued_by: Option<&str>,
) -> Span {
    let span = tracing::info_span!(
        parent: None,
        "job",
        otel.name = %format_args!("job {kind}"),
        otel.kind = "consumer",
        messaging.operation.type = "process",
        quack.job.kind = kind.as_str(),
        quack.job.id = %job_id,
        quack.job.attempt = i64::from(attempt),
        quack.job.queue_wait = queue_wait.as_secs_f64(),
    );
    if let Some(context) = enqueued_by.and_then(parse_trace_context) {
        span.add_link(context);
    }
    span
}

/// The client span for one PostgreSQL query.
pub fn db(operation: DbOperation, table: DbCollection) -> Span {
    tracing::info_span!(
        "db",
        otel.name = %format_args!("db {operation} {table}"),
        otel.kind = "client",
        db.system.name = "postgresql",
        db.operation.name = operation.as_str(),
        db.collection.name = table.as_str(),
        db.query.summary = %format_args!("{operation} {table}"),
    )
}

/// The client span for one Discord REST call. Record the status with
/// [`record_status`], the outcome with [`record`], and any rate-limit wait
/// with [`record_rate_limit_wait`].
pub fn discord(method: HttpMethod, route: UrlTemplate) -> Span {
    tracing::info_span!(
        "discord",
        otel.name = %format_args!("discord {method} {route}"),
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
