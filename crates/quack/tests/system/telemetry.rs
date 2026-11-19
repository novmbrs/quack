//! The telemetry the whole system exports: every story runs again with
//! in-memory exporters, then every catalog instrument must have reported
//! with non-zero values, only its declared attribute keys, and no id or
//! free text as an attribute value. The exceptions are the instruments
//! only `serve`'s startup feeds (checked by `tests/serve.rs`) and the ones
//! only a live Discord connection or an infrastructure fault records,
//! listed with why. Spans must arrive too, with their attributes.

use std::collections::BTreeSet;
use std::time::Duration;

use anyhow::{Context as _, Result};
use axum::http::{Method, StatusCode};
use opentelemetry::trace::{SpanId, SpanKind};
use opentelemetry_sdk::trace::SpanData;
use quack_core::ids::MessageId;
use quack_db::jobs::RunnerConfig;
use quack_telemetry::catalog::{self, InstrumentKind};
use quack_testkit::fake::{Call, Script};
use quack_testkit::{ADMIN, GUILD, MEMBER, MEMBER_2, MODERATOR, OWNER, sf};
use serde_json::json;

use crate::capture::{self, Point, SERVE_STARTUP};
use crate::common::{System, case_add, ix, moderator};

/// Instruments recorded only on paths the fake Discord port replaces.
const NEEDS_LIVE_DISCORD: &[(&str, &str)] = &[
    ("http.client.request.duration", "the REST transport"),
    (
        "quack.discord.rate_limit.hits",
        "the REST transport's limiter",
    ),
    (
        "quack.discord.rate_limit.wait",
        "the REST transport's limiter",
    ),
    ("quack.gateway.event.duration", "the gateway's dispatcher"),
    ("quack.gateway.heartbeat.latency", "the gateway connection"),
    ("quack.gateway.connection.events", "the gateway connection"),
    ("quack.gateway.connected", "the gateway connection"),
    ("quack.gateway.guilds", "the gateway's server cache"),
];

/// Instruments recorded only when the infrastructure misbehaves, which
/// the stories don't stage; the telemetry crate's own tests check their
/// shape.
const NEEDS_A_FAULT: &[(&str, &str)] = &[
    ("quack.job.lease_expired", "a job outliving its lease"),
    ("db.client.connection.timeouts", "an exhausted pool"),
    ("quack.db.transaction.retries", "a serialization failure"),
    (
        "quack.cache.evictions",
        "100,000 interactions or 15 idle minutes",
    ),
    ("quack.queue.dropped", "a full queue"),
];

#[test]
fn every_instrument_reports_with_bounded_attributes() {
    let captured = capture::capture();
    crate::moderation::warning_timeout_ban_appeal_accepted_and_ban_undone().unwrap();
    crate::dm_failed::a_refused_dm_reads_the_same_everywhere().unwrap();
    crate::tickets::a_ticket_opens_journals_closes_and_keeps_its_transcript().unwrap();
    crate::honeypot::a_burst_in_the_honeypot_is_one_case_and_the_posts_are_cleaned_up().unwrap();
    crate::activity::edits_and_deletes_are_posted_and_unknown_deletes_flush_at_shutdown().unwrap();
    crate::shutdown::shutdown_drains_what_it_can_and_a_restart_resolves_the_rest().unwrap();
    detours().unwrap();
    captured.flush();
    let metrics = captured.metrics();

    let skipped: BTreeSet<&str> = SERVE_STARTUP
        .iter()
        .copied()
        .chain(
            NEEDS_LIVE_DISCORD
                .iter()
                .chain(NEEDS_A_FAULT)
                .map(|(name, _)| *name),
        )
        .collect();
    let mut problems = Vec::new();
    for spec in catalog::INSTRUMENTS {
        let Some(points) = metrics.get(spec.name) else {
            if !skipped.contains(spec.name) {
                problems.push(format!("{}: never reported", spec.name));
            }
            continue;
        };
        if skipped.contains(spec.name) && !SERVE_STARTUP.contains(&spec.name) {
            problems.push(format!("{}: reported, so take it off its list", spec.name));
        }
        let sums_up = matches!(
            spec.kind,
            InstrumentKind::Counter | InstrumentKind::ObservableCounter | InstrumentKind::Histogram
        );
        if sums_up && points.iter().all(|p| p.value <= 0.0) {
            problems.push(format!("{}: only zeros", spec.name));
        }
        let keys: BTreeSet<&str> = points
            .iter()
            .flat_map(|p| &p.attributes)
            .map(|(key, _)| key.as_str())
            .collect();
        if !spec.attributes.is_empty() && keys.is_empty() {
            problems.push(format!("{}: no attributes", spec.name));
        }
        for point in points {
            problems.extend(bad_attributes(spec.name, spec.attributes, point));
        }
    }
    assert!(problems.is_empty(), "{problems:#?}");
    spans_arrive_without_text(&captured.spans());
}

/// Every kind of span the design names arrives, and none carries what a
/// person wrote.
fn spans_arrive_without_text(spans: &[SpanData]) {
    let expected = [
        (SpanKind::Server, "interaction "),
        (SpanKind::Server, "POST /api/guilds/{guild_id}/cases"),
        (SpanKind::Consumer, "job "),
        (SpanKind::Internal, "use_case "),
        (SpanKind::Client, "SELECT "),
    ];
    for (kind, prefix) in expected {
        assert!(
            spans
                .iter()
                .any(|s| s.span_kind == kind && s.name.starts_with(prefix)),
            "no {kind:?} span named {prefix}…"
        );
    }
    let written = [
        "Please don't spam.",
        "Wrong member.",
        "posted my own server",
        "welcome back",
        "free nitro",
    ];
    for span in spans {
        let values = span
            .attributes
            .iter()
            .chain(span.events.iter().flat_map(|e| &e.attributes))
            .map(|kv| kv.value.to_string());
        for value in values {
            assert!(
                !written.iter().any(|w| value.contains(w)),
                "{} carries {value:?}",
                span.name
            );
        }
    }
}

/// What the stories never do: a rate-limited timeout, a refused case,
/// evidence with a file, a cancelled sign-in, and too many sign-in starts.
#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn detours() -> Result<()> {
    let sys = System::boot().await?;
    sys.install().await?;
    let admin = sys.login(ADMIN).await?;
    let created = sys
        .http(
            Method::POST,
            "/api/guilds/1001/rules",
            Some(&admin),
            Some(json!({
                "name": "Spam", "description": "", "reason": "Please don't spam.",
                "appealable": true, "counting_window_days": null, "fields": [],
                "levels": [{"from_case_count": 1,
                            "action": {"kind": "timeout", "duration_seconds": 600},
                            "send_dm": false}]
            })),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.json);
    let rule = created.json["id"].as_str().context("rule id")?.to_owned();

    // Discord rate-limits the timeout once; the action retries.
    sys.fake
        .script(Call::Timeout, Script::RetryAfter(Duration::from_millis(10)));
    sys.send(&ix(
        2,
        moderator(),
        sys.channel,
        &case_add(MEMBER, &rule, None),
    ))
    .await;
    sys.settle().await?;
    // A second timeout, then the first case is voided: its undo is
    // skipped because the second case still applies.
    sys.send(&ix(
        2,
        moderator(),
        sys.channel,
        &case_add(MEMBER, &rule, None),
    ))
    .await;
    sys.settle().await?;
    let moderator_session = sys.login(MODERATOR).await?;
    let voided = sys
        .http(
            Method::POST,
            "/api/guilds/1001/cases/1/void",
            Some(&moderator_session),
            Some(json!({"reason": "Wrong member."})),
        )
        .await;
    assert_eq!(voided.status, StatusCode::OK, "{}", voided.json);
    sys.settle().await?;
    // The owner can't be moderated: refused.
    sys.send(&ix(
        2,
        moderator(),
        sys.channel,
        &case_add(OWNER, &rule, None),
    ))
    .await;

    // A dashboard case with a message carrying a file as evidence, copied
    // to a staff-only evidence channel.
    let copies = sys.fake.world(|w| w.add_text_channel(sys.guild, true));
    sys.db
        .execute(&format!(
            "UPDATE guild_settings SET evidence_channel_id = {copies} WHERE guild_id = {GUILD}"
        ))
        .await?;
    let message: MessageId = sys.fake.world(|w| {
        let file = w.attachment("a.png", Some("image/png"), vec![1, 2, 3]);
        w.post(sys.guild, sys.channel, sf(MEMBER_2), "spam", vec![file])
    });
    let case = sys
        .http(
            Method::POST,
            "/api/guilds/1001/cases",
            Some(&moderator_session),
            Some(json!({
                "member_id": MEMBER_2.to_string(), "rule_id": rule,
                "evidence_links": [format!(
                    "https://discord.com/channels/{GUILD}/{}/{message}", sys.channel
                )]
            })),
        )
        .await;
    assert_eq!(case.status, StatusCode::CREATED, "{}", case.json);
    sys.settle().await?;

    // Sign-in: one cancelled at Discord, then starts until rate limited.
    sys.http(
        Method::GET,
        "/api/auth/callback?error=access_denied",
        None,
        None,
    )
    .await;
    let mut limited = false;
    for _ in 0..40 {
        let start = sys.http(Method::GET, "/api/auth/login", None, None).await;
        if start.status == StatusCode::TOO_MANY_REQUESTS {
            limited = true;
            break;
        }
    }
    assert!(limited, "sign-in starts are rate limited");
    Ok(())
}

/// Undeclared keys, values outside a closed set, and values that look
/// like an id or a person's text.
fn bad_attributes(name: &str, declared: &[&str], point: &Point) -> Vec<String> {
    let mut problems = Vec::new();
    for (key, value) in &point.attributes {
        if !declared.contains(&key.as_str()) {
            problems.push(format!("{name}: undeclared key {key}"));
        }
        let closed = catalog::ATTRIBUTES
            .iter()
            .find(|a| a.key == key)
            .map_or(&[][..], |a| a.values);
        if !closed.is_empty() && !closed.contains(&value.as_str()) {
            problems.push(format!("{name}: {key}={value:?} is not an allowed value"));
        }
        if looks_like_id_or_text(value) {
            problems.push(format!("{name}: {key}={value:?} looks like an id or text"));
        }
    }
    problems
}

/// Five or more digits in a row (snowflakes, UUIDs, numbers in text),
/// more than three words, or sentence punctuation.
fn looks_like_id_or_text(value: &str) -> bool {
    let digits = value
        .split(|c: char| !c.is_ascii_digit())
        .any(|run| run.len() >= 5);
    let words = value.split_whitespace().count() > 3;
    let punctuation = value.contains(['\'', '"', ',', '!', '?', '@', '#']);
    digits || words || punctuation || value.len() > 64
}

/// A case added in Discord enqueues its DM inside the interaction's trace;
/// the runner's span for the DM job starts its own trace and links back to
/// it (§18.4).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_job_span_links_to_the_interaction_that_enqueued_it() -> Result<()> {
    let captured = capture::capture();
    let sys = System::boot().await?;
    sys.install().await?;
    let admin = sys.login(ADMIN).await?;
    let created = sys
        .http(
            Method::POST,
            "/api/guilds/1001/rules",
            Some(&admin),
            Some(json!({
                "name": "Spam", "description": "", "reason": "Please don't spam.",
                "appealable": true, "counting_window_days": null, "fields": [],
                "levels": [{"from_case_count": 1, "action": null, "send_dm": true}]
            })),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.json);
    let rule = created.json["id"].as_str().context("rule id")?;
    let interaction = ix(2, moderator(), sys.channel, &case_add(MEMBER, rule, None));
    let interaction_id = interaction["id"].as_str().context("id")?.to_owned();
    sys.send(&interaction).await;
    // The real runner, which opens each job's span.
    let runner = quack_engine::jobs::runner(
        sys.app.engine.clone(),
        RunnerConfig::new(4),
        sys.app.module_jobs(),
    );
    runner.start();
    let sent = tokio::time::timeout(Duration::from_secs(10), async {
        while sys.dms(MEMBER).is_empty() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    runner.shutdown(Duration::from_secs(5)).await;
    assert!(sent.is_ok(), "the DM was sent");
    captured.flush();

    let spans = captured.spans();
    let root = spans
        .iter()
        .find(|s| {
            s.name.starts_with("interaction ")
                && s.attributes.iter().any(|kv| {
                    kv.key.as_str() == "quack.interaction.id"
                        && kv.value.to_string() == interaction_id
                })
        })
        .context("the interaction's span")?;
    let trace = root.span_context.trace_id();
    let job = spans
        .iter()
        .find(|s| {
            s.name == "job send_dm"
                && s.links
                    .links
                    .iter()
                    .any(|l| l.span_context.trace_id() == trace)
        })
        .context("a send_dm job span linked to the interaction's trace")?;
    assert_eq!(job.span_kind, SpanKind::Consumer);
    assert_ne!(job.span_context.trace_id(), trace, "a trace of its own");
    assert_eq!(job.parent_span_id, SpanId::INVALID, "a root");
    let linked = job.links.links[0].span_context.span_id();
    assert!(
        spans
            .iter()
            .any(|s| s.span_context.span_id() == linked && s.span_context.trace_id() == trace),
        "the link names a span that was exported"
    );
    Ok(())
}
