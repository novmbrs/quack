//! The generic job runner: one dispatcher per kind, per-kind and global
//! concurrency limits, a hard deadline per run, and a graceful stop.
//!
//! The engine supplies one [`Handler`] callback for every kind. The runner
//! never interprets payloads; it claims, times, and settles.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::Instrument as _;

use super::{ClaimedJob, JobKind, JobResult, RunAt, claim, next_due_in, settle, sweep_leases};
use crate::notify::Notification;
use crate::obs::{self, RunOutcome};
use crate::{Db, DbError};

/// Which entry point of the kind the runner is calling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// A normal run, which may call Discord.
    Run,
    /// The budget is spent (or the last run asked for a retry): settle the
    /// domain record terminally without calling Discord (§16.1).
    Finalize,
}

/// What a handler did with its run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Handled {
    /// The handler already applied this result with [`settle`] in its own
    /// transaction, together with its domain writes.
    Settled(JobResult),
    /// The runner should apply this result (fenced) in a transaction of
    /// its own; for runs that wrote nothing else.
    Settle(JobResult),
}

/// The boxed future a [`Handler`] returns.
pub type HandlerFuture = Pin<Box<dyn Future<Output = Result<Handled, DbError>> + Send>>;

/// The engine's job entry point, called for every claimed job of every
/// kind. [`DbError::Stale`] means the lease was lost (nothing is settled);
/// any other error is retried with the kind's backoff.
pub type Handler = Arc<dyn Fn(ClaimedJob, Phase) -> HandlerFuture + Send + Sync>;

/// Runner settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunnerConfig {
    /// Jobs running at once across all kinds (`QUACK_WORKER_CONCURRENCY`).
    pub concurrency: usize,
    /// The kinds to dispatch.
    pub kinds: Vec<JobKind>,
    /// The polling fallback for lost notifications.
    pub poll: Duration,
    /// The most jobs one claim query takes.
    pub claim_batch: usize,
}

impl RunnerConfig {
    /// Every kind, `concurrency` jobs at once, a 2-second poll.
    pub fn new(concurrency: usize) -> Self {
        Self {
            concurrency: concurrency.max(1),
            kinds: JobKind::ALL.to_vec(),
            poll: Duration::from_secs(2),
            claim_batch: 8,
        }
    }
}

struct KindSlot {
    wake: Notify,
    permits: Arc<Semaphore>,
    heartbeat: Mutex<Option<Instant>>,
}

struct Shared {
    db: Db,
    handler: Handler,
    config: RunnerConfig,
    global: Arc<Semaphore>,
    slots: HashMap<JobKind, KindSlot>,
    /// Kinds that found every global permit taken, woken in order as
    /// permits free up, so one busy kind can't starve the others.
    waiting: Mutex<VecDeque<JobKind>>,
    stop_claiming: CancellationToken,
    hard_stop: CancellationToken,
    tracker: TaskTracker,
}

/// Runs claimed jobs through the [`Handler`]. Cheap to clone.
#[derive(Clone)]
pub struct Runner {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for Runner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runner")
            .field("config", &self.shared.config)
            .finish_non_exhaustive()
    }
}

impl Runner {
    /// Builds a runner; nothing runs until [`Runner::start`].
    pub fn new(db: Db, config: RunnerConfig, handler: Handler) -> Self {
        let slots = config
            .kinds
            .iter()
            .map(|kind| {
                let slot = KindSlot {
                    wake: Notify::new(),
                    permits: Arc::new(Semaphore::new(kind.policy().concurrency)),
                    heartbeat: Mutex::new(None),
                };
                (*kind, slot)
            })
            .collect();
        Self {
            shared: Arc::new(Shared {
                db,
                handler,
                global: Arc::new(Semaphore::new(config.concurrency)),
                config,
                slots,
                waiting: Mutex::new(VecDeque::new()),
                stop_claiming: CancellationToken::new(),
                hard_stop: CancellationToken::new(),
                tracker: TaskTracker::new(),
            }),
        }
    }

    /// Starts one dispatcher per kind.
    pub fn start(&self) {
        for kind in &self.shared.config.kinds {
            let shared = Arc::clone(&self.shared);
            let kind = *kind;
            self.shared
                .tracker
                .spawn(async move { dispatch(shared, kind).await });
        }
    }

    /// Wakes the dispatcher of `kind` (a `quack_jobs` notification).
    pub fn wake(&self, kind: JobKind) {
        if let Some(slot) = self.shared.slots.get(&kind) {
            slot.wake.notify_one();
        }
    }

    /// Wakes every dispatcher (after the listener reconnects).
    pub fn wake_all(&self) {
        for slot in self.shared.slots.values() {
            slot.wake.notify_one();
        }
    }

    /// Routes a listener notification: job kinds wake their dispatcher, a
    /// reconnect wakes all; cache notifications are ignored.
    pub fn on_notification(&self, notification: Notification) {
        match notification {
            Notification::Job(kind) => self.wake(kind),
            Notification::Reconnected => self.wake_all(),
            Notification::Rules(_) | Notification::Settings(_) => {}
        }
    }

    /// Whether every dispatcher finished a pass within `max_age` (readiness,
    /// §14.9). A dispatcher that never finished a pass is not fresh.
    pub fn heartbeats_fresh(&self, max_age: Duration) -> bool {
        self.shared.slots.values().all(|slot| {
            slot.heartbeat
                .lock()
                .ok()
                .and_then(|beat| *beat)
                .is_some_and(|beat| beat.elapsed() <= max_age)
        })
    }

    /// Stops claiming at once, lets running handlers finish for up to
    /// `drain`, then drops the rest without settling (their leases expire
    /// and the next process resolves them, §16.7).
    pub async fn shutdown(&self, drain: Duration) {
        self.shared.stop_claiming.cancel();
        self.shared.tracker.close();
        if tokio::time::timeout(drain, self.shared.tracker.wait())
            .await
            .is_err()
        {
            self.shared.hard_stop.cancel();
            self.shared.tracker.wait().await;
        }
    }
}

/// One kind's loop: pass, heartbeat, then wait for a wake-up, the poll, or
/// the next due job, whichever comes first.
async fn dispatch(shared: Arc<Shared>, kind: JobKind) {
    let Some(slot) = shared.slots.get(&kind) else {
        return;
    };
    while !shared.stop_claiming.is_cancelled() {
        let wait = match pass(&shared, kind).await {
            Ok(next) => {
                if let Ok(mut beat) = slot.heartbeat.lock() {
                    *beat = Some(Instant::now());
                }
                next.map_or(shared.config.poll, |due| due.min(shared.config.poll))
            }
            Err(error) => {
                tracing::warn!(job_kind = %kind, error = %error, "job dispatch pass failed");
                shared.config.poll
            }
        };
        tokio::select! {
            () = slot.wake.notified() => {}
            () = tokio::time::sleep(wait) => {}
            () = shared.stop_claiming.cancelled() => break,
        }
    }
}

/// Sweeps expired leases, then claims and starts as many jobs as permits
/// allow. Returns when the next job is due, if known.
async fn pass(shared: &Arc<Shared>, kind: JobKind) -> Result<Option<Duration>, DbError> {
    let Some(slot) = shared.slots.get(&kind) else {
        return Ok(None);
    };
    let mut conn = shared.db.conn().await?;
    let expired = sweep_leases(&mut conn, kind).await?;
    if expired > 0 {
        obs::lease_expired(kind, expired);
    }
    loop {
        if shared.stop_claiming.is_cancelled() {
            return Ok(None);
        }
        let mut permits = Vec::new();
        while permits.len() < shared.config.claim_batch {
            let Ok(own) = Arc::clone(&slot.permits).try_acquire_owned() else {
                break;
            };
            let Ok(global) = Arc::clone(&shared.global).try_acquire_owned() else {
                if let Ok(mut waiting) = shared.waiting.lock()
                    && !waiting.contains(&kind)
                {
                    waiting.push_back(kind);
                }
                drop(own);
                break;
            };
            permits.push(Permit {
                kind,
                shared: Arc::clone(shared),
                held: Some((own, global)),
            });
        }
        if permits.is_empty() {
            // Busy: a finishing job wakes this kind.
            return Ok(None);
        }
        let asked = permits.len();
        let jobs = claim(&mut conn, kind, asked).await?;
        let claimed = jobs.len();
        for (job, permit) in jobs.into_iter().zip(permits) {
            let shared = Arc::clone(shared);
            shared.tracker.clone().spawn(run_job(shared, job, permit));
        }
        if claimed < asked {
            break;
        }
    }
    next_due_in(&mut conn, kind).await
}

/// A kind permit plus a global permit; releasing them wakes this kind and
/// the next kind waiting for a global permit.
struct Permit {
    kind: JobKind,
    shared: Arc<Shared>,
    held: Option<(OwnedSemaphorePermit, OwnedSemaphorePermit)>,
}

impl Drop for Permit {
    fn drop(&mut self) {
        drop(self.held.take());
        if let Some(slot) = self.shared.slots.get(&self.kind) {
            slot.wake.notify_one();
        }
        let next = self
            .shared
            .waiting
            .lock()
            .ok()
            .and_then(|mut w| w.pop_front());
        if let Some(next) = next.and_then(|kind| self.shared.slots.get(&kind)) {
            next.wake.notify_one();
        }
    }
}

/// Runs one claimed job under its deadline and records how it ended.
async fn run_job(shared: Arc<Shared>, job: ClaimedJob, permit: Permit) {
    let kind = job.kind;
    let span = obs::job_span(&job);
    let started = Instant::now();
    obs::job_started(kind, job.queue_wait());
    let deadline = kind.policy().deadline();
    let outcome = async {
        tokio::select! {
            result = tokio::time::timeout(deadline, run_phases(&shared, &job)) => {
                result.unwrap_or(RunOutcome::TimedOut)
            }
            () = shared.hard_stop.cancelled() => RunOutcome::TimedOut,
        }
    }
    .instrument(span.clone())
    .await;
    obs::job_finished(&span, kind, outcome, started.elapsed());
    drop(permit);
}

/// Calls the handler (or finalize), then settles when asked to.
async fn run_phases(shared: &Shared, job: &ClaimedJob) -> RunOutcome {
    {
        let mut phase = if job.budget_spent() {
            Phase::Finalize
        } else {
            Phase::Run
        };
        loop {
            let handled = (shared.handler)(job.clone(), phase).await;
            let result = match handled {
                Ok(Handled::Settled(result)) => return RunOutcome::of(&result, phase),
                Err(DbError::Stale) => return RunOutcome::LostLease,
                Ok(Handled::Settle(result)) => result,
                Err(error) => {
                    tracing::warn!(job_kind = %job.kind, error = %error, "job handler failed");
                    JobResult::Retry {
                        at: RunAt::In(job.kind.policy().backoff(job.attempt)),
                        error: format!("db_{}", obs::error_class(&error)),
                    }
                }
            };
            if phase == Phase::Run
                && matches!(result, JobResult::Retry { .. })
                && job.is_last_attempt()
            {
                phase = Phase::Finalize;
                continue;
            }
            return match settle_alone(&shared.db, job, &result).await {
                Ok(()) => RunOutcome::of(&result, phase),
                Err(DbError::Stale) => RunOutcome::LostLease,
                Err(error) => {
                    tracing::warn!(job_kind = %job.kind, error = %error, "settling a job failed");
                    RunOutcome::Failed
                }
            };
        }
    }
}

async fn settle_alone(db: &Db, job: &ClaimedJob, result: &JobResult) -> Result<(), DbError> {
    let mut tx = db.begin().await?;
    settle(&mut tx, job, result).await?;
    tx.commit().await
}
