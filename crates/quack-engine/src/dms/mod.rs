//! DMs (§6): the `send_dm` job handler and its finalize. A DM is sent at
//! most once: a create whose answer is lost is repeated only with the same
//! nonce inside the window (§10.4), and after that it is "couldn't
//! confirm" for good.

mod render;
mod send;

use std::time::Instant;

use quack_core::action::{ActionKind, ActionState};
use quack_core::dm::{DmFailure, DmKind, DmState};
use quack_core::ids::{DmId, MessageId};
use quack_core::modlog::{DmFailedDetail, DmFailedState, DmSentDetail, ModLogEntry, ModLogLinks};
use quack_db::dms::{self, Dm};
use quack_db::jobs::{self, ClaimedJob, Handled, JobResult, NewJob, settle as settle_job};
use quack_db::messages::{self, Owner};
use quack_db::{DbError, Tx, actions, appeals, cases};
use quack_telemetry::metrics;

use crate::modlog::{self, By};
use crate::{Engine, obs};

pub(crate) use render::context_lines;
pub use send::{finalize, send};

/// The `send_dm` payload.
#[derive(Clone, Copy, Debug, serde::Deserialize)]
struct Payload {
    dm_id: DmId,
}

/// A change to a DM's state (§6.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Change {
    /// `sending` → `sent`.
    Sent(MessageId),
    /// `pending` or `sending` → `failed`.
    Failed(DmFailure, Option<u32>),
    /// `sending` → `unknown`.
    Unknown,
    /// `pending` → `not_needed` (the case was voided before the start).
    NotNeeded,
    /// `sending` → `pending`: nothing was sent.
    BackToPending,
}

impl Change {
    const fn state(self) -> DmState {
        match self {
            Self::Sent(_) => DmState::Sent,
            Self::Failed(..) => DmState::Failed,
            Self::Unknown => DmState::Unknown,
            Self::NotNeeded => DmState::NotNeeded,
            Self::BackToPending => DmState::Pending,
        }
    }
}

/// Applies `change` to the DM with its log entry, receipt bump, and the
/// wake of a waiting kick or ban, in one transaction with the job's
/// settle (§6.2). Every write is a compare-and-set on the state read.
async fn settle_dm(
    engine: &Engine,
    job: &ClaimedJob,
    dm: &Dm,
    change: Change,
    result: JobResult,
    started: Option<Instant>,
) -> Result<Handled, DbError> {
    let guild = dm.guild;
    let mut tx = engine.db().begin().await?;
    if let Some(case) = dm.case {
        cases::lock(&mut tx, guild, case).await?;
    }
    let locked = dms::lock(&mut tx, guild, dm.id)
        .await?
        .ok_or(DbError::NotFound)?;
    let applied = match change {
        Change::Sent(message) => dms::mark_sent(&mut tx, guild, dm.id, message).await?,
        Change::Failed(failure, code) => {
            dms::mark_failed(&mut tx, guild, dm.id, locked.state, failure, code).await?
        }
        Change::Unknown => dms::mark_unknown(&mut tx, guild, dm.id).await?,
        Change::NotNeeded => dms::mark_not_needed(&mut tx, guild, dm.id).await?,
        Change::BackToPending => dms::back_to_pending(&mut tx, guild, dm.id).await?,
    };
    if !applied {
        tx.rollback().await?;
        return Err(DbError::Stale);
    }
    let state = change.state();
    if state.is_terminal() && state != DmState::NotNeeded {
        log_settled(&mut tx, dm, change).await?;
    }
    if state.is_terminal() && dm.kind == DmKind::Case {
        if let Some(case) = dm.case {
            messages::bump(&mut tx, guild, Owner::Case(case)).await?;
        }
        if quack_core::dm::releases_waiting_action(dm.kind, state) {
            wake_action(&mut tx, dm).await?;
        }
    }
    settle_job(&mut tx, job, &result).await?;
    tx.commit().await?;
    if state.is_terminal() {
        let failure = match change {
            Change::Failed(f, _) => Some(f),
            _ => None,
        };
        obs::dm_settled(dm.id, dm.kind, state, failure);
        if let Some(started) = started {
            metrics()
                .dms
                .sent_in(obs::dm_kind(dm.kind), started.elapsed());
        }
    }
    Ok(Handled::Settled(result))
}

/// `dm_sent` or `dm_failed` (§9.2).
async fn log_settled(tx: &mut Tx, dm: &Dm, change: Change) -> Result<(), DbError> {
    let appeal = match dm.appeal_event {
        Some(event) => appeals::event(tx, dm.guild, event).await?.map(|e| e.appeal),
        None => None,
    };
    let entry = match change {
        Change::Sent(_) => ModLogEntry::DmSent(DmSentDetail { dm_kind: dm.kind }),
        Change::Failed(failure, code) => ModLogEntry::DmFailed(DmFailedDetail {
            dm_kind: dm.kind,
            state: DmFailedState::Failed,
            failure: Some(failure),
            discord_code: code,
        }),
        Change::Unknown | Change::NotNeeded | Change::BackToPending => {
            ModLogEntry::DmFailed(DmFailedDetail {
                dm_kind: dm.kind,
                state: DmFailedState::Unknown,
                failure: None,
                discord_code: None,
            })
        }
    };
    modlog::write(
        tx,
        dm.guild,
        entry,
        By::QUACK,
        ModLogLinks {
            case_id: dm.case,
            member_id: Some(dm.member),
            appeal_id: appeal,
            ticket_id: dm.ticket,
            ..ModLogLinks::default()
        },
    )
    .await?;
    Ok(())
}

/// A kick or ban waiting for this case DM runs now (§6.3).
async fn wake_action(tx: &mut Tx, dm: &Dm) -> Result<(), DbError> {
    let Some(case) = dm.case else {
        return Ok(());
    };
    for action in actions::for_case(tx, dm.guild, case).await? {
        if matches!(action.kind, ActionKind::Kick | ActionKind::Ban)
            && action.state == ActionState::Pending
        {
            jobs::wake(tx, &NewJob::run_action(dm.guild, action.id)).await?;
        }
    }
    Ok(())
}
