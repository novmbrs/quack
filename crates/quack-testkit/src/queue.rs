//! Running the job queue deterministically: one job at a time, in a fixed
//! kind order, with the same claim, finalize, and settle rules as the
//! real runner. Durable times come from PostgreSQL, so "time passing" is
//! simulated by moving stored times back (see [`Harness::fast_forward`]).

use anyhow::{Result, bail};
use quack_db::DbError;
use quack_db::jobs::{self, ClaimedJob, Handled, JobKind, JobResult, Phase, RunAt};
use quack_engine::Engine;
use tokio::task::JoinHandle;

use crate::Harness;
use crate::fake::Gate;

/// The engine's kinds, in the order a pass runs them.
pub const ENGINE_KINDS: &[JobKind] = &[
    JobKind::RunAction,
    JobKind::SendDm,
    JobKind::CaptureEvidence,
    JobKind::SyncMessage,
    JobKind::PostModLog,
    JobKind::Housekeeping,
];

/// How one job run ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunEnd {
    /// The handler (or finalize) settled with this result.
    Settled(JobResult),
    /// The run lost its lease; nothing was settled.
    LostLease,
}

/// What a claimed run did.
#[derive(Clone, Debug, PartialEq)]
pub struct Ran {
    /// The job.
    pub job: ClaimedJob,
    /// How it ended.
    pub end: RunEnd,
}

/// Runs one claimed job like the runner does: finalize when the budget is
/// spent or the last run asks for a retry; settle results the handler
/// didn't settle itself.
pub async fn run_claimed(engine: &Engine, job: ClaimedJob) -> Ran {
    let mut phase = if job.budget_spent() {
        Phase::Finalize
    } else {
        Phase::Run
    };
    loop {
        let result = match quack_engine::jobs::handle(engine, &job, phase).await {
            Ok(Handled::Settled(result)) => {
                return Ran {
                    job,
                    end: RunEnd::Settled(result),
                };
            }
            Err(DbError::Stale) => {
                return Ran {
                    job,
                    end: RunEnd::LostLease,
                };
            }
            Ok(Handled::Settle(result)) => result,
            Err(error) => JobResult::Retry {
                at: RunAt::In(job.kind.policy().backoff(job.attempt)),
                error: format!("db_{error}"),
            },
        };
        if phase == Phase::Run && matches!(result, JobResult::Retry { .. }) && job.is_last_attempt()
        {
            phase = Phase::Finalize;
            continue;
        }
        let settled = async {
            let mut tx = engine.db().begin().await?;
            jobs::settle(&mut tx, &job, &result).await?;
            tx.commit().await
        }
        .await;
        let end = match settled {
            Ok(()) => RunEnd::Settled(result),
            Err(_) => RunEnd::LostLease,
        };
        return Ran { job, end };
    }
}

impl Harness {
    /// Claims the next due job of `kind`, if any.
    pub async fn claim(&self, kind: JobKind) -> Result<Option<ClaimedJob>> {
        let mut conn = self.db.db.conn().await?;
        Ok(jobs::claim(&mut conn, kind, 1).await?.into_iter().next())
    }

    /// Runs the next due job of `kind` to its end.
    pub async fn run_one(&self, kind: JobKind) -> Result<Option<Ran>> {
        let Some(job) = self.claim(kind).await? else {
            return Ok(None);
        };
        Ok(Some(run_claimed(&self.engine, job).await))
    }

    /// Runs due jobs, one at a time, until none is due. Returns what ran.
    pub async fn run_jobs(&self) -> Result<Vec<Ran>> {
        let mut ran = Vec::new();
        for _ in 0..10_000 {
            let mut any = false;
            for kind in ENGINE_KINDS {
                if let Some(r) = self.run_one(*kind).await? {
                    ran.push(r);
                    any = true;
                }
            }
            if !any {
                return Ok(ran);
            }
        }
        bail!("the job queue did not settle")
    }

    /// Runs the queue to quiescence: runs due jobs, then moves every
    /// waiting job and pending action retry to now, until no job is left
    /// (or only dead ones). Returns what ran.
    pub async fn settle(&self) -> Result<Vec<Ran>> {
        let mut ran = Vec::new();
        for _ in 0..200 {
            ran.extend(self.run_jobs().await?);
            if self.waiting_jobs().await? == 0 {
                return Ok(ran);
            }
            self.fast_forward().await?;
        }
        bail!("the job queue never became quiet")
    }

    /// Jobs that are neither dead nor claimed.
    pub async fn waiting_jobs(&self) -> Result<i64> {
        self.db
            .count("SELECT count(*) FROM jobs WHERE dead_at IS NULL AND locked_until IS NULL AND kind <> 'housekeeping'")
            .await
    }

    /// "Time passes": every waiting job becomes due now, and every
    /// pending action's next attempt is due now.
    pub async fn fast_forward(&self) -> Result<()> {
        self.db
            .execute("UPDATE jobs SET run_at = now() WHERE run_at > now() AND locked_until IS NULL AND dead_at IS NULL")
            .await?;
        self.db
            .execute("UPDATE actions SET next_attempt_at = now() - interval '1 millisecond' WHERE state = 'pending' AND next_attempt_at > now()")
            .await?;
        Ok(())
    }

    /// A crash: every claimed job's lease ends now, so the next claim of
    /// its kind sweeps it and runs it again.
    pub async fn expire_leases(&self) -> Result<()> {
        self.db
            .execute("UPDATE jobs SET locked_until = now() - interval '1 second' WHERE locked_until IS NOT NULL")
            .await?;
        let mut conn = self.db.db.conn().await?;
        for kind in JobKind::ALL {
            jobs::sweep_leases(&mut conn, *kind).await?;
        }
        Ok(())
    }

    /// Starts the next due job of `kind` in a task (to race it against
    /// other work, or to crash it at a gate).
    pub async fn spawn_one(&self, kind: JobKind) -> Result<JoinHandle<Ran>> {
        let Some(job) = self.claim(kind).await? else {
            bail!("no {kind} job is due");
        };
        let engine = self.engine.clone();
        Ok(tokio::spawn(async move { run_claimed(&engine, job).await }))
    }

    /// Runs the next due job of `kind` until its call reaches `gate`, then
    /// kills it there (the process died mid-run) and expires its lease.
    pub async fn crash_at(&self, kind: JobKind, gate: &Gate) -> Result<()> {
        let task = self.spawn_one(kind).await?;
        gate.reached().await;
        task.abort();
        let _ = task.await;
        gate.release();
        self.expire_leases().await
    }
}
