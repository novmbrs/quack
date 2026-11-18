//! Metrics for cases, actions, DMs, appeals, evidence, the moderation log,
//! and tracked messages.

use std::time::Duration;

use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Histogram, Meter, ObservableGauge};

use super::observe::{Source, counter, histogram, u64_gauge};
use crate::catalog;
use crate::labels::{
    ActionKind, ActionState, AppealTransition, AttemptOutcome, Attribute, CaseSource, DmKind,
    DmOutcome, MessageKind, ModLogKind, ModLogResult, Outcome, Reason, VoidCause,
};

/// The `quack.action.kind` attribute for a case's action, `none` for a warning.
fn case_action(action: Option<ActionKind>) -> KeyValue {
    KeyValue::new(ActionKind::KEY, action.map_or("none", ActionKind::as_str))
}

/// Case metrics: created, refused, voided, creation time, and the level
/// chosen.
pub struct CaseMetrics {
    created: Counter<u64>,
    refused: Counter<u64>,
    voided: Counter<u64>,
    create_duration: Histogram<f64>,
    level: Histogram<f64>,
}

impl CaseMetrics {
    pub(crate) fn new(meter: &Meter) -> Self {
        Self {
            created: counter(meter, &catalog::CASES_CREATED),
            refused: counter(meter, &catalog::CASES_REFUSED),
            voided: counter(meter, &catalog::CASES_VOIDED),
            create_duration: histogram(meter, &catalog::CASE_CREATE_DURATION),
            level: histogram(meter, &catalog::CASE_LEVEL),
        }
    }

    /// Records a committed case: where it came from, its level's action
    /// (`None` for a warning), the 1-based level number, and how long
    /// creating it took.
    pub fn created(
        &self,
        source: CaseSource,
        action: Option<ActionKind>,
        level: u32,
        duration: Duration,
    ) {
        let action = case_action(action);
        self.created.add(1, &[source.kv(), action.clone()]);
        self.create_duration
            .record(duration.as_secs_f64(), &[source.kv()]);
        self.level.record(f64::from(level), &[action]);
    }

    /// Counts a refused case creation (a member check, a missing
    /// capability, the 2FA rule, or Quack's own access), by reason.
    pub fn refused(&self, source: CaseSource, reason: Reason) {
        self.refused.add(1, &[source.kv(), reason.kv()]);
    }

    /// Counts a voided case.
    pub fn voided(&self, cause: VoidCause) {
        self.voided.add(1, &[cause.kv()]);
    }
}

/// Action metrics: attempts, terminal states, retries, completion time,
/// failures, and skipped undo actions.
pub struct ActionMetrics {
    attempts: Counter<u64>,
    finished: Counter<u64>,
    completion: Histogram<f64>,
    retries: Counter<u64>,
    undo_skipped: Counter<u64>,
    failed: Source<Vec<(ActionKind, u64)>>,
    _failed_gauge: ObservableGauge<u64>,
}

impl ActionMetrics {
    pub(crate) fn new(meter: &Meter) -> Self {
        let failed: Source<Vec<(ActionKind, u64)>> = Source::new();
        let source = failed.clone();
        let failed_gauge = u64_gauge(meter, &catalog::ACTIONS_FAILED, move |observer| {
            for (kind, count) in source.read().unwrap_or_default() {
                observer.observe(count, &[kind.kv()]);
            }
        });
        Self {
            attempts: counter(meter, &catalog::ACTION_ATTEMPTS),
            finished: counter(meter, &catalog::ACTIONS_FINISHED),
            completion: histogram(meter, &catalog::ACTION_COMPLETION_DURATION),
            retries: counter(meter, &catalog::ACTION_RETRIES),
            undo_skipped: counter(meter, &catalog::ACTION_UNDO_SKIPPED),
            failed,
            _failed_gauge: failed_gauge,
        }
    }

    /// Counts one settled attempt at an action. Pass the failure reason
    /// when the attempt was refused.
    pub fn attempt(&self, kind: ActionKind, outcome: AttemptOutcome, reason: Option<Reason>) {
        match reason {
            Some(reason) => self
                .attempts
                .add(1, &[kind.kv(), outcome.kv(), reason.kv()]),
            None => self.attempts.add(1, &[kind.kv(), outcome.kv()]),
        }
    }

    /// Counts an action reaching `done`, `failed`, `cancelled`, or
    /// `skipped`.
    pub fn finished(&self, kind: ActionKind, state: ActionState) {
        self.finished.add(1, &[kind.kv(), state.kv()]);
    }

    /// Records the time from case creation until the action was done.
    pub fn completed(&self, kind: ActionKind, since_case_created: Duration) {
        self.completion
            .record(since_case_created.as_secs_f64(), &[kind.kv()]);
    }

    /// Counts an attempt scheduled again after a retryable failure.
    pub fn retried(&self, kind: ActionKind, reason: Reason) {
        self.retries.add(1, &[kind.kv(), reason.kv()]);
    }

    /// Counts an undo action left in place because another valid case
    /// still applies.
    pub fn undo_skipped(&self, kind: ActionKind) {
        self.undo_skipped.add(1, &[kind.kv()]);
    }

    /// Installs the supplier of how many actions of each kind are failed now.
    pub fn observe_failed(
        &self,
        supplier: impl Fn() -> Vec<(ActionKind, u64)> + Send + Sync + 'static,
    ) {
        self.failed.set(supplier);
    }
}

/// DM metrics: outcomes and send time.
pub struct DmMetrics {
    outcomes: Counter<u64>,
    send_duration: Histogram<f64>,
}

impl DmMetrics {
    pub(crate) fn new(meter: &Meter) -> Self {
        Self {
            outcomes: counter(meter, &catalog::DM_OUTCOMES),
            send_duration: histogram(meter, &catalog::DM_SEND_DURATION),
        }
    }

    /// Counts a DM reaching its final state. Pass the reason for failed or
    /// unknown.
    pub fn outcome(&self, kind: DmKind, outcome: DmOutcome, reason: Option<Reason>) {
        match reason {
            Some(reason) => self
                .outcomes
                .add(1, &[kind.kv(), outcome.kv(), reason.kv()]),
            None => self.outcomes.add(1, &[kind.kv(), outcome.kv()]),
        }
    }

    /// Records how long one send took.
    pub fn sent_in(&self, kind: DmKind, duration: Duration) {
        self.send_duration
            .record(duration.as_secs_f64(), &[kind.kv()]);
    }
}

/// Appeal metrics: transitions, time to decision, and appeals open.
pub struct AppealMetrics {
    transitions: Counter<u64>,
    decision: Histogram<f64>,
    open: Source<u64>,
    _open_gauge: ObservableGauge<u64>,
}

impl AppealMetrics {
    pub(crate) fn new(meter: &Meter) -> Self {
        let open: Source<u64> = Source::new();
        let source = open.clone();
        let open_gauge = u64_gauge(meter, &catalog::APPEALS_OPEN, move |observer| {
            if let Some(count) = source.read() {
                observer.observe(count, &[]);
            }
        });
        Self {
            transitions: counter(meter, &catalog::APPEAL_TRANSITIONS),
            decision: histogram(meter, &catalog::APPEAL_DECISION_DURATION),
            open,
            _open_gauge: open_gauge,
        }
    }

    /// Counts an appeal change, including the member submitting it.
    pub fn transition(&self, transition: AppealTransition) {
        self.transitions.add(1, &[transition.kv()]);
    }

    /// Records the time from submission to a staff decision (accept, reject,
    /// or close).
    pub fn decided(&self, decision: AppealTransition, since_submitted: Duration) {
        self.decision
            .record(since_submitted.as_secs_f64(), &[decision.kv()]);
    }

    /// Installs the supplier of how many appeals are waiting for staff.
    pub fn observe_open(&self, supplier: impl Fn() -> u64 + Send + Sync + 'static) {
        self.open.set(supplier);
    }
}

/// Evidence metrics: captures, files, and bytes.
pub struct EvidenceMetrics {
    captures: Counter<u64>,
    files: Counter<u64>,
    bytes: Counter<u64>,
}

impl EvidenceMetrics {
    pub(crate) fn new(meter: &Meter) -> Self {
        Self {
            captures: counter(meter, &catalog::EVIDENCE_CAPTURES),
            files: counter(meter, &catalog::EVIDENCE_FILES),
            bytes: counter(meter, &catalog::EVIDENCE_BYTES),
        }
    }

    /// Records one finished capture with the files and bytes it copied.
    pub fn captured(&self, outcome: Outcome, files: u64, bytes: u64) {
        self.captures.add(1, &[outcome.kv()]);
        self.files.add(files, &[]);
        self.bytes.add(bytes, &[]);
    }
}

/// Moderation-log metrics: entries written and log-channel posts.
pub struct ModLogMetrics {
    entries: Counter<u64>,
    posts: Counter<u64>,
}

impl ModLogMetrics {
    pub(crate) fn new(meter: &Meter) -> Self {
        Self {
            entries: counter(meter, &catalog::MODLOG_ENTRIES),
            posts: counter(meter, &catalog::MODLOG_POSTS),
        }
    }

    /// Counts an entry written, with the result it records.
    pub fn entry(&self, kind: ModLogKind, result: ModLogResult) {
        self.entries.add(1, &[kind.kv(), result.kv()]);
    }

    /// Counts a post to the log channel.
    pub fn posted(&self, outcome: Outcome) {
        self.posts.add(1, &[outcome.kv()]);
    }
}

/// Tracked-message metrics: syncs and lag.
pub struct TrackedMessageMetrics {
    syncs: Counter<u64>,
    lag: Histogram<f64>,
}

impl TrackedMessageMetrics {
    pub(crate) fn new(meter: &Meter) -> Self {
        Self {
            syncs: counter(meter, &catalog::MESSAGE_SYNCS),
            lag: histogram(meter, &catalog::MESSAGE_SYNC_LAG),
        }
    }

    /// Counts one sync run.
    pub fn synced(&self, kind: MessageKind, outcome: Outcome) {
        self.syncs.add(1, &[kind.kv(), outcome.kv()]);
    }

    /// Records the time from a change to the message showing it.
    pub fn lag(&self, kind: MessageKind, lag: Duration) {
        self.lag.record(lag.as_secs_f64(), &[kind.kv()]);
    }
}
