//! The `run_action` handler (§5.4): start, read and act, settle; and its
//! finalize when the job's budget is spent.

use quack_core::action::{ActionKind, ActionState, StartDecision, StartFacts, StillApplies, start};
use quack_core::dm::DmKind;
use quack_core::ids::{ActionId, GuildId};
use quack_core::permissions::action_precheck;
use quack_db::actions::{self, Action, Expected};
use quack_db::cases::{self, Case};
use quack_db::jobs::{ClaimedJob, Handled, JobResult, RunAt, settle as settle_job};
use quack_db::messages::{self, Owner};
use quack_db::{DbError, Tx, dms};
use time::OffsetDateTime;

use super::act::{ActFacts, act, gather};
use super::settle::{Recorded, apply_settled, interrupted, settle_attempt, write_finalized};
use super::{Payload, log_failed, log_skipped};
use crate::jobs::{dead, retry};
use crate::{Engine, obs};

/// Runs one `run_action` job (§5.4). Idempotent: every attempt reads
/// Discord before acting, and start and settle are compare-and-set on the
/// action's state and attempt.
pub async fn run(engine: &Engine, job: &ClaimedJob) -> Result<Handled, DbError> {
    let (Some(guild), Ok(payload)) = (job.guild, job.payload::<Payload>()) else {
        return Ok(dead("bad_payload"));
    };
    obs::use_case(
        "run_action",
        Some(guild),
        run_action(engine, job, guild, payload.action_id),
    )
    .await
}

async fn run_action(
    engine: &Engine,
    job: &ClaimedJob,
    guild: GuildId,
    id: ActionId,
) -> Result<Handled, DbError> {
    let (action, case, rule_name) = {
        let mut conn = engine.db().conn().await?;
        let Some(action) = actions::get(&mut conn, guild, id).await? else {
            return Ok(dead("action_missing"));
        };
        if action.state.is_terminal() || action.state == ActionState::Failed {
            return Ok(Handled::Settle(JobResult::Done));
        }
        let Some(case) = cases::get(&mut conn, guild, action.case).await? else {
            return Ok(dead("case_missing"));
        };
        let rule_name = rule_name(&mut conn, &case).await?;
        (action, case, rule_name)
    };
    // Facts that need Discord are gathered before any transaction.
    let facts = match gather(engine, guild, &case).await {
        Ok(facts) => facts,
        Err(code) => return Ok(Handled::Settle(retry(job, code))),
    };
    let number = match begin(engine, job, &action, &facts).await? {
        Begun::Settled(result) => return Ok(Handled::Settled(result)),
        Begun::Opened(number) => number,
    };
    let target = super::act::target(engine, &action, &case).await?;
    let first_write = no_earlier_write(engine, &action, number).await?;
    let result = act(engine, &action, &case, &target, &rule_name, first_write).await;
    settle_attempt(engine, job, id, number, result).await
}

/// Whether no earlier attempt of this action may have reached Discord
/// (§5.2): only then may a ban delete messages, so a retry after a lost
/// answer (or a crash after the call) never deletes them a second time,
/// even if someone lifted the ban in between.
async fn no_earlier_write(engine: &Engine, action: &Action, open: u32) -> Result<bool, DbError> {
    let mut conn = engine.db().conn().await?;
    let earlier: Vec<_> = actions::attempts(&mut conn, action.id)
        .await?
        .into_iter()
        .filter(|a| a.number != open)
        .map(|a| a.outcome)
        .collect();
    Ok(!quack_core::action::may_have_applied(&earlier))
}

async fn rule_name(conn: &mut quack_db::PgConnection, case: &Case) -> Result<String, DbError> {
    let Some((rule, version)) = case.rule else {
        return Ok(String::new());
    };
    Ok(quack_db::rules::version(conn, case.guild, rule, version)
        .await?
        .map_or_default(|v| v.definition.name))
}

enum Begun {
    Settled(JobResult),
    Opened(u32),
}

/// The start transaction (§5.4 step 1): member lock, case, action; settle
/// an interrupted attempt; then the start rows of §5.3.
async fn begin(
    engine: &Engine,
    job: &ClaimedJob,
    action: &Action,
    facts: &ActFacts,
) -> Result<Begun, DbError> {
    let guild = action.guild;
    let mut tx = engine.db().begin().await?;
    actions::lock_member(&mut tx, guild, action.member).await?;
    let case = cases::lock(&mut tx, guild, action.case)
        .await?
        .ok_or(DbError::NotFound)?;
    let mut current = actions::lock(&mut tx, guild, action.id)
        .await?
        .ok_or(DbError::NotFound)?;
    let mut settled = Vec::new();
    loop {
        let decision = start(&start_facts(&mut tx, &case, &current, facts).await?);
        let result = match decision {
            StartDecision::SettleInterrupted => {
                settled.push(interrupted(&mut tx, &case, &current).await?);
                current = actions::lock(&mut tx, guild, action.id)
                    .await?
                    .ok_or(DbError::NotFound)?;
                continue;
            }
            StartDecision::Open => {
                let number = actions::start(&mut tx, guild, action.id)
                    .await?
                    .ok_or(DbError::Stale)?;
                quack_db::jobs::fence(&mut tx, job).await?;
                tx.commit().await?;
                record(&settled);
                return Ok(Begun::Opened(number));
            }
            StartDecision::Finish => JobResult::Done,
            StartDecision::RetryAt(at) | StartDecision::DeferUntil(at) => {
                JobResult::Defer { at: RunAt::At(at) }
            }
            StartDecision::Cancel => {
                settled.extend(cancel(&mut tx, &case, &current).await?);
                JobResult::Done
            }
            StartDecision::Skip(still) => {
                settled.extend(skip(&mut tx, &case, &current, still).await?);
                JobResult::Done
            }
            StartDecision::Fail(failure) => {
                if actions::mark_failed(&mut tx, guild, current.id, Expected::Pending, &failure)
                    .await?
                {
                    log_failed(&mut tx, &case, &current, &failure, current.attempts).await?;
                    messages::bump(&mut tx, guild, Owner::Case(case.id)).await?;
                    settled.push(apply_settled(current.kind, ActionState::Failed, None, None));
                }
                JobResult::Done
            }
        };
        settle_job(&mut tx, job, &result).await?;
        tx.commit().await?;
        record(&settled);
        return Ok(Begun::Settled(result));
    }
}

fn record(settled: &[Recorded]) {
    for s in settled {
        s.record();
    }
}

async fn start_facts(
    tx: &mut Tx,
    case: &Case,
    action: &Action,
    facts: &ActFacts,
) -> Result<StartFacts, DbError> {
    let guild = case.guild;
    let member_busy = actions::member_running(tx, guild, action.member, action.id).await?;
    let dm_waiting = if matches!(action.kind, ActionKind::Kick | ActionKind::Ban) {
        dms::for_case(tx, guild, case.id)
            .await?
            .iter()
            .any(|dm| dm.kind == DmKind::Case && !dm.state.is_terminal())
    } else {
        false
    };
    let still_applies = match action.kind.undoes() {
        Some(original) => actions::guard(tx, guild, action.member, original, case.id)
            .await?
            .map(|(case_id, case_number)| StillApplies {
                case_id,
                case_number,
            }),
        None => None,
    };
    Ok(StartFacts {
        kind: action.kind,
        state: action.state,
        next_attempt_at: action.next_attempt_at,
        member_busy,
        case_valid: case.is_valid(),
        case_created_at: case.created_at,
        dm_waiting,
        still_applies,
        precheck: action_precheck(&facts.apply, action.kind),
        timeout_ends_at: action.timeout_ends_at,
        now: OffsetDateTime::now_utc(),
    })
}

async fn cancel(tx: &mut Tx, case: &Case, action: &Action) -> Result<Option<Recorded>, DbError> {
    if !actions::mark_cancelled(tx, case.guild, action.id, Expected::Pending).await? {
        return Ok(None);
    }
    messages::bump(tx, case.guild, Owner::Case(case.id)).await?;
    Ok(Some(apply_settled(
        action.kind,
        ActionState::Cancelled,
        None,
        None,
    )))
}

async fn skip(
    tx: &mut Tx,
    case: &Case,
    action: &Action,
    still: StillApplies,
) -> Result<Option<Recorded>, DbError> {
    let reason = quack_core::action::ActionSkipReason::OtherCaseApplies;
    let skipped = actions::mark_skipped(
        tx,
        case.guild,
        action.id,
        Expected::Pending,
        reason,
        Some(still.case_id),
    )
    .await?;
    if !skipped {
        return Ok(None);
    }
    log_skipped(tx, case, action, reason, Some(still.case_number)).await?;
    messages::bump(tx, case.guild, Owner::Case(case.id)).await?;
    Ok(Some(apply_settled(
        action.kind,
        ActionState::Skipped,
        None,
        None,
    )))
}

/// Settles a `run_action` job whose budget is spent, without calling
/// Discord (§16.5): an open attempt becomes `interrupted` and the action
/// `failed` (`unconfirmed` or `retries_exhausted`).
pub async fn finalize(engine: &Engine, job: &ClaimedJob) -> Result<Handled, DbError> {
    let (Some(guild), Ok(payload)) = (job.guild, job.payload::<Payload>()) else {
        return Ok(dead("bad_payload"));
    };
    obs::use_case("finalize_action", Some(guild), async {
        let mut tx = engine.db().begin().await?;
        let Some(action) = actions::get(&mut tx, guild, payload.action_id).await? else {
            return Ok(dead("action_missing"));
        };
        let case = cases::lock(&mut tx, guild, action.case)
            .await?
            .ok_or(DbError::NotFound)?;
        let action = actions::lock(&mut tx, guild, action.id)
            .await?
            .ok_or(DbError::NotFound)?;
        let recorded = write_finalized(&mut tx, &case, &action).await?;
        settle_job(&mut tx, job, &JobResult::Done).await?;
        tx.commit().await?;
        if let Some(r) = recorded {
            r.record();
        }
        Ok(Handled::Settled(JobResult::Done))
    })
    .await
}
