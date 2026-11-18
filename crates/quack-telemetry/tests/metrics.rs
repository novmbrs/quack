//! Every catalog instrument is exported with its declared name, type, unit,
//! attributes, and buckets, and nothing else is.

use std::collections::{BTreeSet, HashMap};
use std::time::Duration;

use opentelemetry::KeyValue;
use opentelemetry::metrics::MeterProvider as _;
use opentelemetry_sdk::metrics::data::{
    AggregatedMetrics, Metric, MetricData, ResourceMetrics, ScopeMetrics,
};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};
use quack_telemetry::catalog::{self, InstrumentKind};
use quack_telemetry::labels::{
    ActionKind, ActionState, ActivityEvent, ActivityStage, AppealTransition, AttemptOutcome,
    CacheName, CaseSource, CommandName, DbCollection, DbErrorClass, DbOperation, DmKind, DmOutcome,
    GatewayConnectionEvent, GatewayEvent, HoneypotOutcome, HttpMethod, HttpRoute, InteractionKind,
    InteractionOutcome, JobKind, JobOutcome, MessageKind, ModLogKind, ModLogResult, Outcome,
    PoolName, QueueName, RateLimitClass, RateLimitScope, Reason, RestOutcome, SignInOutcome,
    TransactionOutcome, UrlTemplate, VoidCause,
};
use quack_telemetry::{JobQueueSample, Metrics, PoolSample};

const MS: Duration = Duration::from_millis(1);

/// Calls every record method and installs every supplier once.
fn exercise(m: &Metrics) {
    exercise_moderation(m);
    exercise_platform(m);
}

fn exercise_moderation(m: &Metrics) {
    let command = CommandName::new("case add");
    m.interactions
        .acked(InteractionKind::SlashCommand, command, 40 * MS);
    m.interactions.finished(
        InteractionKind::SlashCommand,
        command,
        InteractionOutcome::Refused,
        120 * MS,
    );
    m.interactions
        .error(InteractionKind::SlashCommand, command, Reason::NotAllowed);

    m.cases
        .created(CaseSource::Discord, Some(ActionKind::Timeout), 2, 30 * MS);
    m.cases.created(CaseSource::Dashboard, None, 1, 20 * MS);
    m.cases.refused(CaseSource::Honeypot, Reason::RoleHierarchy);
    m.cases.voided(VoidCause::AppealAccepted);

    m.actions
        .attempt(ActionKind::Ban, AttemptOutcome::Done, None);
    m.actions.attempt(
        ActionKind::Kick,
        AttemptOutcome::Refused,
        Some(Reason::RoleHierarchy),
    );
    m.actions.finished(ActionKind::Unban, ActionState::Skipped);
    m.actions.completed(ActionKind::Ban, 900 * MS);
    m.actions.retried(ActionKind::Timeout, Reason::RateLimited);
    m.actions.undo_skipped(ActionKind::Unban);
    m.actions.observe_failed(|| vec![(ActionKind::Kick, 3)]);

    m.dms.outcome(DmKind::Case, DmOutcome::Sent, None);
    m.dms.outcome(
        DmKind::AppealUpdate,
        DmOutcome::Failed,
        Some(Reason::DmsClosed),
    );
    m.dms.sent_in(DmKind::Case, 300 * MS);

    m.appeals.transition(AppealTransition::Submit);
    m.appeals
        .decided(AppealTransition::Accept, Duration::from_secs(7200));
    m.appeals.observe_open(|| 4);

    m.evidence.captured(Outcome::Done, 2, 4096);
    m.modlog
        .entry(ModLogKind::new("case_created"), ModLogResult::Ok);
    m.modlog.posted(Outcome::Failed);
    m.messages.synced(MessageKind::CaseReceipt, Outcome::Done);
    m.messages.lag(MessageKind::TicketPost, 2 * MS);

    m.tickets.opened();
    m.tickets.closed();
    m.tickets.transcript_saved(20_000);
    m.tickets.observe_open(|| 2);
    m.activity_log
        .event(ActivityEvent::new("message_delete"), ActivityStage::Posted);
    m.honeypot.hit(HoneypotOutcome::Actioned);
    m.honeypot.cleanup(Outcome::Done);
}

fn exercise_platform(m: &Metrics) {
    m.jobs.enqueued(JobKind::SendDm);
    m.jobs.started(JobKind::SendDm, 5 * MS);
    m.jobs
        .finished(JobKind::SendDm, JobOutcome::Retry, 250 * MS);
    m.jobs.lease_expired(JobKind::CloseTicket);
    m.jobs.observe_backlog(|| {
        vec![JobQueueSample {
            kind: JobKind::RunAction,
            ready: 7,
            oldest_ready_age: Duration::from_secs(3),
            dead: 1,
        }]
    });

    let bans = UrlTemplate::new("/guilds/{guild.id}/bans/{user.id}");
    m.discord
        .request(HttpMethod::Put, bans, Some(204), RestOutcome::Done, 80 * MS);
    m.discord
        .request(HttpMethod::Put, bans, None, RestOutcome::Unknown, 5000 * MS);
    m.discord
        .rate_limited(bans, RateLimitScope::Route, 1200 * MS);

    m.gateway.event(GatewayEvent::new("MESSAGE_CREATE"), 3 * MS);
    m.gateway.heartbeat(45 * MS);
    m.gateway.connection(GatewayConnectionEvent::Resume);
    m.gateway.observe_connected(|| true);
    m.gateway.observe_guilds(|| 12);

    let route = HttpRoute::new("/api/guilds/{guild_id}/cases");
    {
        let _in_flight = m.http.in_flight(HttpMethod::Get);
        m.http.request(HttpMethod::Get, route, 200, 15 * MS);
    }
    m.http.sign_in(SignInOutcome::Done);
    m.http.rate_limited(RateLimitClass::new("write"));
    m.http.observe_sessions(|| 9);

    let main = PoolName::new("main");
    m.db.query(
        DbOperation::Select,
        DbCollection::new("cases"),
        2 * MS,
        None,
    );
    m.db.query(
        DbOperation::Update,
        DbCollection::new("jobs"),
        MS,
        Some(DbErrorClass::SerializationFailure),
    );
    m.db.connection_wait(main, MS);
    m.db.connection_timeout(main);
    m.db.transaction(TransactionOutcome::Commit, 4 * MS);
    m.db.transaction_retry(DbErrorClass::Deadlock);
    m.db.observe_pools(move || {
        vec![PoolSample {
            pool: main,
            max: 10,
            idle: 6,
            used: 4,
        }]
    });

    let rules = CacheName::new("rules");
    m.caches.lookup(rules, true);
    m.caches.lookup(rules, false);
    m.caches.evicted(rules, 2);
    m.caches.observe_size(rules, || 30);
    let events = QueueName::new("gateway.events");
    m.queues.dropped(events, 1);
    m.queues.observe_depth(events, || 5);
}

fn kind_of(metric: &Metric) -> &'static str {
    fn shape<T>(data: &MetricData<T>) -> &'static str {
        match data {
            MetricData::Gauge(_) => "gauge",
            MetricData::Sum(sum) if sum.is_monotonic() => "counter",
            MetricData::Sum(_) => "updowncounter",
            MetricData::Histogram(_) => "histogram",
            MetricData::ExponentialHistogram(_) => "exponential",
        }
    }
    match metric.data() {
        AggregatedMetrics::F64(data) => shape(data),
        AggregatedMetrics::U64(data) => shape(data),
        AggregatedMetrics::I64(data) => shape(data),
    }
}

fn expected_shape(kind: InstrumentKind) -> &'static str {
    match kind {
        InstrumentKind::Counter | InstrumentKind::ObservableCounter => "counter",
        InstrumentKind::UpDownCounter | InstrumentKind::ObservableUpDownCounter => "updowncounter",
        InstrumentKind::Histogram => "histogram",
        InstrumentKind::Gauge => "gauge",
    }
}

/// Attribute keys on every data point, and histogram bounds if any.
fn points(metric: &Metric) -> (Vec<BTreeSet<String>>, Option<Vec<f64>>) {
    fn collect<T>(data: &MetricData<T>) -> (Vec<BTreeSet<String>>, Option<Vec<f64>>) {
        let keys = |attrs: &mut dyn Iterator<Item = &KeyValue>| {
            attrs.map(|kv| kv.key.to_string()).collect::<BTreeSet<_>>()
        };
        match data {
            MetricData::Gauge(g) => (
                g.data_points().map(|p| keys(&mut p.attributes())).collect(),
                None,
            ),
            MetricData::Sum(s) => (
                s.data_points().map(|p| keys(&mut p.attributes())).collect(),
                None,
            ),
            MetricData::Histogram(h) => (
                h.data_points().map(|p| keys(&mut p.attributes())).collect(),
                h.data_points().next().map(|p| p.bounds().collect()),
            ),
            MetricData::ExponentialHistogram(_) => (Vec::new(), None),
        }
    }
    match metric.data() {
        AggregatedMetrics::F64(data) => collect(data),
        AggregatedMetrics::U64(data) => collect(data),
        AggregatedMetrics::I64(data) => collect(data),
    }
}

/// Records through a fresh provider with the catalog view and returns one
/// collection. Whatever `record` returns stays alive until after the flush.
fn export<T>(record: impl FnOnce(&opentelemetry::metrics::Meter) -> T) -> Vec<ResourceMetrics> {
    let exporter = InMemoryMetricExporter::default();
    let provider = SdkMeterProvider::builder()
        .with_reader(PeriodicReader::builder(exporter.clone()).build())
        .with_view(catalog::view)
        .build();
    let keep = record(&provider.meter("quack"));
    let flushed = provider.force_flush();
    drop(keep);
    assert!(flushed.is_ok(), "flush failed: {flushed:?}");
    exporter.get_finished_metrics().unwrap_or_default()
}

fn by_name(batches: &[ResourceMetrics]) -> HashMap<&str, &Metric> {
    batches
        .iter()
        .flat_map(ResourceMetrics::scope_metrics)
        .flat_map(ScopeMetrics::metrics)
        .map(|metric| (metric.name(), metric))
        .collect()
}

#[test]
fn every_catalog_instrument_is_exported_with_its_declared_shape() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .build()
        .unwrap();
    let batches = export(|meter| {
        let metrics = Metrics::new(meter, "1.2.3");
        metrics.runtime.observe_tokio(runtime.handle().clone());
        exercise(&metrics);
        metrics
    });
    let exported = by_name(&batches);

    let declared: BTreeSet<_> = catalog::INSTRUMENTS.iter().map(|s| s.name).collect();
    let actual: BTreeSet<_> = exported.keys().copied().collect();
    let missing: Vec<_> = declared.difference(&actual).collect();
    let extra: Vec<_> = actual.difference(&declared).collect();
    assert!(missing.is_empty(), "never exported: {missing:?}");
    assert!(
        extra.is_empty(),
        "exported but not in the catalog: {extra:?}"
    );

    for spec in catalog::INSTRUMENTS {
        let metric = exported[spec.name];
        assert_eq!(
            kind_of(metric),
            expected_shape(spec.kind),
            "{} type",
            spec.name
        );
        assert_eq!(metric.unit(), spec.unit, "{} unit", spec.name);
        assert_eq!(
            metric.description(),
            spec.description,
            "{} description",
            spec.name
        );
        let (attribute_sets, bounds) = points(metric);
        assert!(
            !attribute_sets.is_empty(),
            "{} has no data points",
            spec.name
        );
        for keys in attribute_sets {
            for key in &keys {
                assert!(
                    spec.attributes.contains(&key.as_str()),
                    "{} carries undeclared attribute {key}",
                    spec.name
                );
            }
        }
        if spec.kind == InstrumentKind::Histogram {
            assert_eq!(
                bounds.as_deref(),
                Some(spec.buckets),
                "{} buckets",
                spec.name
            );
        }
    }
}

#[test]
fn the_view_drops_attributes_the_catalog_does_not_declare() {
    let spec = catalog::find("quack.cases.created").unwrap();
    let batches = export(|meter| {
        meter
            .u64_counter(spec.name)
            .with_unit(spec.unit)
            .with_description(spec.description)
            .build()
            .add(
                1,
                &[
                    KeyValue::new("quack.case.source", "dashboard"),
                    KeyValue::new("quack.guild.id", "1234567890"),
                ],
            );
    });
    let (attribute_sets, _) = points(by_name(&batches)[spec.name]);
    assert_eq!(
        attribute_sets,
        vec![BTreeSet::from(["quack.case.source".to_owned()])]
    );
}
