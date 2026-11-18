//! Settling an attempt (§5.3 settle rows, §5.4 step 3) and finalizing an
//! action whose job budget is spent (§16.5).

use std::time::Duration;

use quack_core::action::{
    ActionFailure, ActionSkipReason, ActionState, AttemptOutcome, AttemptResult, Failure,
    SettleFacts, may_have_applied, settle,
};
use quack_core::ids::ActionId;
use quack_db::actions::{self, Action};
use quack_db::cases::{self, Case};
use quack_db::jobs::{ClaimedJob, Handled, JobResult, RunAt, settle as settle_job};
use quack_db::messages::{self, Owner};
use quack_db::{DbError, Tx};
use quack_telemetry::metrics;
use quack_telemetry::spans::SpanEvent;
use time::OffsetDateTime;

use super::{log_done, log_failed, log_skipped, round};
use crate::Engine;
use crate::obs::{self, event, kv};

/// Metrics for one settled attempt or state change, recorded only after
/// the transaction commits.
#[derive(Clone, Copy, Debug)]
pub(super) struct Recorded {
    kind: quack_core::action::ActionKind,
    state: ActionState,
    attempt: Option<(AttemptOutcome, Option<Failure>, bool)>,
    completed: Option<Duration>,
}

impl Recorded {
    /// Records the metrics and the `action.attempt` span event.
    pub(super) fn record(&self) {
        let m = &metrics().actions;
        let kind = obs::action_kind(self.kind);
        if let Some((outcome, failure, rate_limited)) = self.attempt {
            m.attempt(
                kind,
                obs::attempt_outcome(outcome),
                failure.map(|f| obs::action_failure(f.reason)),
            );
            if self.state == ActionState::Pending {
                m.retried(kind, obs::retry_reason(outcome, rate_limited));
            }
        }
        if let Some(state) = obs::action_state(self.state) {
            m.finished(kind, state);
        }
        if self.state == ActionState::Skipped {
            m.undo_skipped(kind);
        }
        if let Some(took) = self.completed {
            m.completed(kind, took);
        }
    }
}

/// A state change without an attempt (start rows, finalize).
pub(super) const fn apply_settled(
    kind: quack_core::action::ActionKind,
    state: ActionState,
    attempt: Option<(AttemptOutcome, Option<Failure>, bool)>,
    completed: Option<Duration>,
) -> Recorded {
    Recorded {
        kind,
        state,
        attempt,
        completed,
    }
}

/// The settle transaction (§5.4 step 3): the case, then the action; the
/// action must still be `running` with the attempt this run opened. Any
/// mismatch rolls back: whoever holds the job now settles it.
pub(super) async fn settle_attempt(
    engine: &Engine,
    job: &ClaimedJob,
    id: ActionId,
    number: u32,
    result: AttemptResult,
) -> Result<Handled, DbError> {
    let mut tx = engine.db().begin().await?;
    let Some(guild) = job.guild else {
        return Err(DbError::Stale);
    };
    let unlocked = actions::get(&mut tx, guild, id)
        .await?
        .ok_or(DbError::NotFound)?;
    let case = cases::lock(&mut tx, guild, unlocked.case)
        .await?
        .ok_or(DbError::NotFound)?;
    let action = actions::lock(&mut tx, guild, id)
        .await?
        .ok_or(DbError::NotFound)?;
    let open = actions::open_attempt(&mut tx, id).await?;
    if action.state != ActionState::Running || open != Some(number) {
        tx.rollback().await?;
        return Err(DbError::Stale);
    }
    let (recorded, next) = write_settlement(&mut tx, &case, &action, number, result).await?;
    let job_result = match next {
        Some(delay) => JobResult::Retry {
            at: RunAt::In(delay),
            error: result.outcome().as_str().to_owned(),
        },
        None => JobResult::Done,
    };
    settle_job(&mut tx, job, &job_result).await?;
    tx.commit().await?;
    recorded.record();
    Ok(Handled::Settled(job_result))
}

/// Settles an attempt an earlier run left open, as `interrupted`, inside
/// the start transaction (§5.4 step 1.4).
pub(super) async fn interrupted(
    tx: &mut Tx,
    case: &Case,
    action: &Action,
) -> Result<Recorded, DbError> {
    let number = actions::open_attempt(tx, action.id)
        .await?
        .ok_or(DbError::Stale)?;
    write_settlement(tx, case, action, number, AttemptResult::Interrupted)
        .await
        .map(|(recorded, _)| recorded)
}

/// Finishes the open attempt and applies the settle row of §5.3. Returns
/// the metrics to record and, when the action goes back to `pending`, the
/// wait before its next attempt.
async fn write_settlement(
    tx: &mut Tx,
    case: &Case,
    action: &Action,
    number: u32,
    result: AttemptResult,
) -> Result<(Recorded, Option<Duration>), DbError> {
    let guild = case.guild;
    let now = OffsetDateTime::now_utc();
    let mut outcomes = round(tx, action).await?;
    outcomes.pop();
    let earlier: Vec<AttemptOutcome> = outcomes.into_iter().flatten().collect();
    let s = settle(
        &SettleFacts {
            kind: action.kind,
            case_valid: case.is_valid(),
            attempt: action.attempts,
            earlier: &earlier,
            now,
        },
        result,
    )
    .map_err(|e| DbError::Invalid(e.to_string()))?;
    let discord = result.discord();
    if !actions::finish_attempt(
        tx,
        guild,
        action.id,
        number,
        s.outcome,
        discord.map(|d| d.status),
        discord.and_then(|d| d.code),
    )
    .await?
    {
        return Err(DbError::Stale);
    }
    let (changed, next) = write_state(tx, case, action, number, &s, now).await?;
    if !changed {
        return Err(DbError::Stale);
    }
    if s.state != ActionState::Pending {
        messages::bump(tx, guild, Owner::Case(case.id)).await?;
    }
    event(
        SpanEvent::ActionAttempt,
        vec![
            kv("action.id", action.id.get()),
            kv("kind", action.kind.as_str()),
            kv("attempt", number),
            kv("outcome", s.outcome.as_str()),
            kv("http_status", discord.map_or(0, |d| d.status)),
            kv("discord_code", discord.and_then(|d| d.code).unwrap_or(0)),
        ],
    );
    let rate_limited = matches!(
        result,
        AttemptResult::NotSent {
            retry_after: Some(_)
        }
    );
    let completed = (s.state == ActionState::Done).then(|| obs::since(action.created_at, now));
    Ok((
        apply_settled(
            action.kind,
            s.state,
            Some((s.outcome, s.failure, rate_limited)),
            completed,
        ),
        next,
    ))
}

/// Moves the action to the settlement's state (each a compare-and-set on
/// `running`) and writes its log entry. Returns whether it applied and the
/// wait before the next attempt when it went back to `pending`.
async fn write_state(
    tx: &mut Tx,
    case: &Case,
    action: &Action,
    number: u32,
    s: &quack_core::action::Settlement,
    now: OffsetDateTime,
) -> Result<(bool, Option<Duration>), DbError> {
    let guild = case.guild;
    let mut next = None;
    let changed = match s.state {
        ActionState::Done => {
            let done = actions::mark_done(tx, guild, action.id, s.no_change).await?;
            if done {
                log_done(tx, case, action, number, s.no_change).await?;
            }
            done
        }
        ActionState::Failed => {
            let failure = s
                .failure
                .unwrap_or(Failure::new(ActionFailure::RetriesExhausted));
            let failed =
                actions::mark_failed(tx, guild, action.id, ActionState::Running, &failure).await?;
            if failed {
                log_failed(tx, case, action, &failure, number).await?;
            }
            failed
        }
        ActionState::Cancelled => {
            actions::mark_cancelled(tx, guild, action.id, ActionState::Running).await?
        }
        ActionState::Skipped => {
            let reason = s
                .skip_reason
                .unwrap_or(ActionSkipReason::ChangedOutsideCase);
            let skipped =
                actions::mark_skipped(tx, guild, action.id, ActionState::Running, reason, None)
                    .await?;
            if skipped {
                log_skipped(tx, case, action, reason, None).await?;
            }
            skipped
        }
        ActionState::Pending => {
            let delay = s
                .next_attempt_at
                .map_or(Duration::ZERO, |at| obs::since(now, at));
            next = Some(delay);
            actions::back_to_pending(tx, guild, action.id, delay).await?
        }
        ActionState::Running => false,
    };
    Ok((changed, next))
}

/// Finalize's writes (§16.5): an open attempt becomes `interrupted`; a
/// pending or running action becomes `failed` (`unconfirmed` when any
/// attempt of the round may have applied, else `retries_exhausted`), or
/// `cancelled` when it is an original whose case was voided.
pub(super) async fn write_finalized(
    tx: &mut Tx,
    case: &Case,
    action: &Action,
) -> Result<Option<Recorded>, DbError> {
    let guild = case.guild;
    if !matches!(action.state, ActionState::Pending | ActionState::Running) {
        return Ok(None);
    }
    let mut number = actions::attempts(tx, action.id)
        .await?
        .last()
        .map_or(0, |a| a.number);
    let mut attempt = None;
    if let Some(open) = actions::open_attempt(tx, action.id).await? {
        actions::finish_attempt(
            tx,
            guild,
            action.id,
            open,
            AttemptOutcome::Interrupted,
            None,
            None,
        )
        .await?;
        number = open;
        attempt = Some((AttemptOutcome::Interrupted, None, false));
    }
    let outcomes = round(tx, action).await?;
    let state = if action.kind.is_original() && !case.is_valid() {
        actions::mark_cancelled(tx, guild, action.id, action.state).await?;
        ActionState::Cancelled
    } else {
        let failure = Failure::new(if may_have_applied(&outcomes) {
            ActionFailure::Unconfirmed
        } else {
            ActionFailure::RetriesExhausted
        });
        if actions::mark_failed(tx, guild, action.id, action.state, &failure).await? {
            log_failed(tx, case, action, &failure, number).await?;
        }
        ActionState::Failed
    };
    messages::bump(tx, guild, Owner::Case(case.id)).await?;
    Ok(Some(apply_settled(action.kind, state, attempt, None)))
}
