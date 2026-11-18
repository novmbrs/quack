//! Span and log tests against the real subscriber stack with in-memory
//! exporters.

use opentelemetry::trace::{SpanId, SpanKind, Status};
use opentelemetry::{KeyValue, Value};
use opentelemetry_sdk::logs::{InMemoryLogExporter, SdkLoggerProvider};
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider, SpanData};

use super::*;
use crate::config::TelemetryConfig;
use crate::labels::{InteractionOutcome, JobKind};
use crate::propagation::current_trace_context;
use crate::subscriber::{self, ExportTargets};

struct Harness {
    spans: InMemorySpanExporter,
    logs: InMemoryLogExporter,
    traces: SdkTracerProvider,
    log_provider: SdkLoggerProvider,
}

impl Harness {
    fn new() -> Self {
        let spans = InMemorySpanExporter::default();
        let logs = InMemoryLogExporter::default();
        let traces = SdkTracerProvider::builder()
            .with_simple_exporter(spans.clone())
            .build();
        let log_provider = SdkLoggerProvider::builder()
            .with_simple_exporter(logs.clone())
            .build();
        Self {
            spans,
            logs,
            traces,
            log_provider,
        }
    }

    fn run(&self, f: impl FnOnce()) {
        let config = TelemetryConfig::new("quack-test", "0.0.0");
        let subscriber = subscriber::build(
            &config,
            Some(ExportTargets {
                traces: &self.traces,
                logs: &self.log_provider,
            }),
        )
        .unwrap();
        tracing::subscriber::with_default(subscriber, f);
        self.traces.force_flush().unwrap();
        self.log_provider.force_flush().unwrap();
    }

    fn span(&self, name: &str) -> SpanData {
        self.spans
            .get_finished_spans()
            .unwrap()
            .into_iter()
            .find(|span| span.name == name)
            .unwrap_or_else(|| panic!("no span named {name:?}"))
    }
}

fn attribute(span: &SpanData, key: &str) -> Option<Value> {
    span.attributes
        .iter()
        .find(|kv| kv.key.as_str() == key)
        .map(|kv| kv.value.clone())
}

#[test]
fn interaction_span_has_name_kind_attributes_and_events() {
    let harness = Harness::new();
    harness.run(|| {
        let span = interaction(
            InteractionKind::SlashCommand,
            CommandName::new("case add"),
            Some(1_234_567_890_123_456_789),
            42,
            1_300_000_000_000_000_001,
        );
        event(&span, SpanEvent::Acked, vec![]);
        record(&span, InteractionOutcome::Done);
        event(
            &span,
            SpanEvent::CaseCreated,
            vec![KeyValue::new("quack.case.number", 12)],
        );
        event(&span, SpanEvent::Refused, vec![]);
    });

    let span = harness.span("interaction slash_command case add");
    assert_eq!(span.span_kind, SpanKind::Server);
    assert_eq!(
        attribute(&span, "quack.guild.id"),
        Some(Value::from("1234567890123456789"))
    );
    assert_eq!(attribute(&span, "quack.actor.id"), Some(Value::from("42")));
    assert_eq!(
        attribute(&span, "quack.interaction.id"),
        Some(Value::from("1300000000000000001"))
    );
    assert_eq!(span.parent_span_id, SpanId::INVALID);
    assert_eq!(
        attribute(&span, "quack.command"),
        Some(Value::from("case add"))
    );
    assert_eq!(
        attribute(&span, "quack.interaction.outcome"),
        Some(Value::from("done"))
    );
    let events: Vec<_> = span.events.events.iter().map(|e| e.name.as_ref()).collect();
    assert_eq!(events, ["acked", "case.created", "refused"]);
}

#[test]
fn child_spans_nest_under_their_parent() {
    let harness = Harness::new();
    harness.run(|| {
        let parent = interaction(
            InteractionKind::Component,
            CommandName::new("case view"),
            None,
            1,
            2,
        );
        let _entered = parent.enter();
        let query = db(DbOperation::Select, DbCollection::new("cases"));
        let call = discord(
            HttpMethod::Put,
            UrlTemplate::new("/guilds/{guild.id}/bans/{user.id}"),
        );
        record_status(&call, 204);
        record_error(&query, "statement timeout");
        drop((query, call));
    });

    let parent = harness.span("interaction component case view");
    let query = harness.span("SELECT cases");
    let call = harness.span("PUT /guilds/{guild.id}/bans/{user.id}");
    for child in [&query, &call] {
        assert_eq!(child.parent_span_id, parent.span_context.span_id());
        assert_eq!(
            child.span_context.trace_id(),
            parent.span_context.trace_id()
        );
        assert_eq!(child.span_kind, SpanKind::Client);
    }
    assert_eq!(
        attribute(&query, "db.system.name"),
        Some(Value::from("postgresql"))
    );
    assert_eq!(query.status, Status::error("statement timeout"));
    assert_eq!(
        attribute(&call, "http.response.status_code"),
        Some(Value::I64(204))
    );
    assert_eq!(
        attribute(&call, "server.address"),
        Some(Value::from("discord.com"))
    );
}

#[test]
fn job_span_starts_a_new_trace_linked_to_the_enqueuer() {
    let harness = Harness::new();
    harness.run(|| {
        let stored = {
            let enqueuer = http(
                HttpMethod::Post,
                HttpRoute::new("/api/guilds/{guild_id}/cases"),
            );
            let _entered = enqueuer.enter();
            current_trace_context().unwrap()
        };
        let run = job(&JobRun {
            kind: JobKind::RunAction,
            job_id: "0190d0c4-0000-7000-8000-000000000000",
            attempt: 2,
            max_attempts: 8,
            queue_wait: Duration::from_millis(1500),
            guild_id: Some(99),
            enqueued_by: Some(&stored),
        });
        drop(run);
    });

    let enqueuer = harness.span("POST /api/guilds/{guild_id}/cases");
    let run = harness.span("job run_action");
    assert_eq!(run.span_kind, SpanKind::Consumer);
    assert_eq!(run.parent_span_id, SpanId::INVALID);
    assert_ne!(
        run.span_context.trace_id(),
        enqueuer.span_context.trace_id()
    );
    let link = &run.links.links[0];
    assert_eq!(
        link.span_context.trace_id(),
        enqueuer.span_context.trace_id()
    );
    assert_eq!(link.span_context.span_id(), enqueuer.span_context.span_id());
    assert_eq!(attribute(&run, "quack.job.attempt"), Some(Value::I64(2)));
    assert_eq!(
        attribute(&run, "quack.job.max_attempts"),
        Some(Value::I64(8))
    );
    assert_eq!(attribute(&run, "quack.guild.id"), Some(Value::from("99")));
    assert_eq!(
        attribute(&run, "quack.job.queue_wait"),
        Some(Value::F64(1.5))
    );
}

#[test]
fn job_span_ignores_a_malformed_stored_context() {
    let harness = Harness::new();
    harness.run(|| {
        drop(job(&JobRun {
            kind: JobKind::Housekeeping,
            job_id: 7,
            attempt: 1,
            max_attempts: 1,
            queue_wait: Duration::ZERO,
            guild_id: None,
            enqueued_by: Some("garbage"),
        }));
    });
    let run = harness.span("job housekeeping");
    assert_eq!(run.links.links.len(), 0);
    assert_eq!(attribute(&run, "quack.guild.id"), None);
}

#[test]
fn use_case_spans_nest_and_carry_domain_events() {
    let harness = Harness::new();
    harness.run(|| {
        let root = http(
            HttpMethod::Post,
            HttpRoute::new("/api/guilds/{guild_id}/cases/{case_ref}/void"),
        );
        let _entered = root.enter();
        let span = use_case("void_case", Some(5));
        event(
            &span,
            SpanEvent::CaseVoided,
            vec![KeyValue::new("cause", "staff")],
        );
        event(
            &span,
            SpanEvent::ModLogEntry,
            vec![KeyValue::new("kind", "case_voided")],
        );
        drop(span);
    });
    let root = harness.span("POST /api/guilds/{guild_id}/cases/{case_ref}/void");
    assert_eq!(root.span_kind, SpanKind::Server);
    let span = harness.span("use_case void_case");
    assert_eq!(span.span_kind, SpanKind::Internal);
    assert_eq!(span.parent_span_id, root.span_context.span_id());
    assert_eq!(attribute(&span, "quack.guild.id"), Some(Value::from("5")));
    let events: Vec<_> = span.events.events.iter().map(|e| e.name.as_ref()).collect();
    assert_eq!(events, ["case.voided", "mod_log.entry"]);
}

#[test]
fn span_event_names() {
    let all = [
        (SpanEvent::Acked, "acked"),
        (SpanEvent::Replied, "replied"),
        (SpanEvent::Refused, "refused"),
        (SpanEvent::CaseCreated, "case.created"),
        (SpanEvent::CaseVoided, "case.voided"),
        (SpanEvent::ActionAttempt, "action.attempt"),
        (SpanEvent::DmOutcome, "dm.outcome"),
        (SpanEvent::AppealTransition, "appeal.transition"),
        (SpanEvent::EvidenceCaptured, "evidence.captured"),
        (SpanEvent::MessageSynced, "message.synced"),
        (SpanEvent::ModLogEntry, "mod_log.entry"),
        (SpanEvent::RateLimited, "rate_limited"),
    ];
    for (event, name) in all {
        assert_eq!(event.as_str(), name);
    }
}

#[test]
fn logs_carry_the_active_trace_and_span() {
    let harness = Harness::new();
    harness.run(|| {
        let span = gateway(GatewayEvent::new("MESSAGE_DELETE"), Some(9));
        let _entered = span.enter();
        tracing::info!(deleted = 3, "summarized deletions");
    });

    let span = harness.span("gateway MESSAGE_DELETE");
    let logs = harness.logs.get_emitted_logs().unwrap();
    let log = logs
        .iter()
        .find(|log| {
            log.record
                .body()
                .is_some_and(|body| format!("{body:?}").contains("summarized deletions"))
        })
        .unwrap();
    let context = log.record.trace_context().unwrap();
    assert_eq!(context.trace_id, span.span_context.trace_id());
    assert_eq!(context.span_id, span.span_context.span_id());
}
