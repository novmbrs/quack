//! The generic job runner: one dispatcher per kind, per-kind and global
//! concurrency limits, a hard deadline per run, and a graceful stop.
//!
//! The engine supplies one [`Handler`] callback for every kind. The runner
//! never interprets payloads; it claims, times, and settles.
//!
//! An idle dispatcher costs one query per pass (sweep plus next due time)
//! and sleeps until a notification, a freed permit, the next due job, or
//! the poll. Nothing it does on its own wakes it again.

mod admit;
mod run;

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use tokio::sync::{Notify, Semaphore};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use super::{
    ClaimedJob, JobKind, JobResult, RunAt, claim, next_due_in, settle, sweep_and_next_due,
};
use crate::notify::Notification;
use crate::obs;
use crate::{Db, DbError};
use admit::{Arbiter, Permit};

pub use run::FINALIZE_TRIES;

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
/// any other error, or a panic, is retried with the kind's backoff. In
/// [`Phase::Finalize`] a failure is retried at most [`FINALIZE_TRIES`]
/// times in all (once for an unreadable or unstorable value) before the
/// job is kept as dead.
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

/// How long a dispatcher waits before claiming again when a due job was
/// skipped because another transaction holds its row lock.
const LOCKED_ROW_RETRY: Duration = Duration::from_millis(250);

struct KindSlot {
    permits: Arc<Semaphore>,
    heartbeat: Mutex<Option<Instant>>,
}

struct Shared {
    db: Db,
    handler: Handler,
    config: RunnerConfig,
    arbiter: Arc<Arbiter>,
    slots: HashMap<JobKind, KindSlot>,
    wakes: Arc<HashMap<JobKind, Arc<Notify>>>,
    /// Whether claimed jobs may still start; cleared by shutdown under the
    /// same lock that guards every start.
    admitting: Mutex<bool>,
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
    /// Builds a runner; nothing runs until [`Runner::start`]. A zero
    /// `claim_batch` is treated as 1.
    pub fn new(db: Db, mut config: RunnerConfig, handler: Handler) -> Self {
        config.claim_batch = config.claim_batch.max(1);
        config.concurrency = config.concurrency.max(1);
        let slots = config
            .kinds
            .iter()
            .map(|kind| {
                let slot = KindSlot {
                    permits: Arc::new(Semaphore::new(kind.policy().concurrency)),
                    heartbeat: Mutex::new(None),
                };
                (*kind, slot)
            })
            .collect();
        let wakes = config
            .kinds
            .iter()
            .map(|kind| (*kind, Arc::new(Notify::new())))
            .collect();
        Self {
            shared: Arc::new(Shared {
                db,
                handler,
                arbiter: Arc::new(Arbiter::new(config.concurrency)),
                config,
                slots,
                wakes: Arc::new(wakes),
                admitting: Mutex::new(true),
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
        if let Some(wake) = self.shared.wakes.get(&kind) {
            wake.notify_one();
        }
    }

    /// Wakes every dispatcher (after the listener reconnects).
    pub fn wake_all(&self) {
        for wake in self.shared.wakes.values() {
            wake.notify_one();
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

    /// Stops claiming and starting jobs at once, lets running handlers (and
    /// a claim already sent, whose jobs are handed back unstarted) finish
    /// for up to `drain`, then drops the rest without settling (their
    /// leases expire and the next process resolves them, §16.7).
    pub async fn shutdown(&self, drain: Duration) {
        *self
            .shared
            .admitting
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = false;
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

/// What a dispatcher waits for after a pass.
enum Wait {
    /// A wake-up, or this long (capped by the poll).
    For(Duration),
    /// No permit is free: a finishing job (or a handed-over global permit)
    /// wakes the kind; the poll is the fallback.
    Permits,
}

/// One kind's loop: pass, heartbeat, then wait for a wake-up, the poll, or
/// the next due job, whichever comes first.
async fn dispatch(shared: Arc<Shared>, kind: JobKind) {
    let (Some(slot), Some(wake)) = (shared.slots.get(&kind), shared.wakes.get(&kind)) else {
        return;
    };
    let wake = Arc::clone(wake);
    while !shared.stop_claiming.is_cancelled() {
        let passed = tokio::select! {
            result = pass(&shared, kind) => result,
            () = shared.hard_stop.cancelled() => return,
        };
        let wait = match passed {
            Ok(wait) => {
                if let Ok(mut beat) = slot.heartbeat.lock() {
                    *beat = Some(Instant::now());
                }
                match wait {
                    Wait::For(due) => due.min(shared.config.poll),
                    Wait::Permits => shared.config.poll,
                }
            }
            Err(error) => {
                tracing::warn!(job_kind = %kind, error = %error, "job dispatch pass failed");
                shared.config.poll
            }
        };
        tokio::select! {
            () = wake.notified() => {}
            () = tokio::time::sleep(wait) => {}
            () = shared.stop_claiming.cancelled() => break,
        }
    }
}

/// Sweeps expired leases and reads when the next job is due; when one is
/// due now, claims and starts as many as permits allow.
async fn pass(shared: &Arc<Shared>, kind: JobKind) -> Result<Wait, DbError> {
    let Some(slot) = shared.slots.get(&kind) else {
        return Ok(Wait::For(shared.config.poll));
    };
    // Connecting and sweeping change nothing a later pass can't redo, so
    // shutdown may abandon them; a claim, once sent, is seen through.
    let ready = tokio::select! {
        ready = async {
            let mut conn = shared.db.conn().await?;
            let (expired, next) = sweep_and_next_due(&mut conn, kind).await?;
            Ok::<_, DbError>((conn, expired, next))
        } => ready?,
        () = shared.stop_claiming.cancelled() => return Ok(Wait::For(shared.config.poll)),
    };
    let (mut conn, expired, next) = ready;
    if expired > 0 {
        obs::lease_expired(kind, expired);
    }
    match next {
        None => {
            forfeit(shared, kind);
            return Ok(Wait::For(shared.config.poll));
        }
        Some(due) if !due.is_zero() => {
            forfeit(shared, kind);
            return Ok(Wait::For(due));
        }
        Some(_) => {}
    }
    loop {
        if shared.stop_claiming.is_cancelled() {
            return Ok(Wait::For(shared.config.poll));
        }
        let mut own = Vec::new();
        while own.len() < shared.config.claim_batch {
            let Ok(permit) = Arc::clone(&slot.permits).try_acquire_owned() else {
                break;
            };
            own.push(permit);
        }
        if own.is_empty() {
            forfeit(shared, kind);
            return Ok(Wait::Permits);
        }
        let global = shared.arbiter.take(kind, own.len());
        own.truncate(global);
        if global == 0 {
            return Ok(Wait::Permits);
        }
        let jobs = match claim(&mut conn, kind, global).await {
            Ok(jobs) => jobs,
            Err(error) => {
                give_back(shared, global);
                return Err(error);
            }
        };
        let claimed = jobs.len();
        let (refused, started) = start(shared, jobs, &mut own);
        give_back(shared, global - started);
        for job in refused {
            // Shutdown began while the claim ran: hand the job back with
            // its run uncounted. If that fails the lease expires instead.
            if let Err(error) = settle(&mut conn, &job, &JobResult::Defer { at: RunAt::Now }).await
            {
                tracing::warn!(job_kind = %kind, error = %error, "handing back an unstarted job failed");
            }
        }
        if claimed < global {
            break;
        }
    }
    Ok(match next_due_in(&mut conn, kind).await? {
        // A due job was skipped by `SKIP LOCKED`: its row is locked by
        // another transaction, so try again shortly rather than at once.
        Some(due) if due.is_zero() => Wait::For(LOCKED_ROW_RETRY),
        Some(due) => Wait::For(due),
        None => Wait::For(shared.config.poll),
    })
}

/// Starts each claimed job with one kind permit from `own`. Returns the
/// jobs to hand back because shutdown began, and how many started. A claim
/// whose deadline already passed is not started either: its lease expires
/// and the sweep recovers it.
fn start(
    shared: &Arc<Shared>,
    jobs: Vec<ClaimedJob>,
    own: &mut Vec<tokio::sync::OwnedSemaphorePermit>,
) -> (Vec<ClaimedJob>, usize) {
    let admitting = shared
        .admitting
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let mut refused = Vec::new();
    let mut started = 0;
    for job in jobs {
        if !*admitting {
            refused.push(job);
            continue;
        }
        if job.time_left().is_zero() {
            tracing::warn!(job_kind = %job.kind, job_id = %job.id, "a claim arrived with no time left; not starting it");
            obs::job_finished(
                &obs::job_span(&job),
                job.kind,
                obs::RunOutcome::TimedOut,
                Duration::ZERO,
            );
            continue;
        }
        let permit = Permit {
            kind: job.kind,
            own: own.pop(),
            arbiter: Arc::clone(&shared.arbiter),
            wakes: Arc::clone(&shared.wakes),
        };
        shared
            .tracker
            .spawn(run::run_job(Arc::clone(shared), job, permit));
        started += 1;
    }
    (refused, started)
}

/// Gives up `kind`'s place in line and any turn it was handed.
fn forfeit(shared: &Shared, kind: JobKind) {
    for next in shared.arbiter.forfeit(kind) {
        if let Some(wake) = shared.wakes.get(&next) {
            wake.notify_one();
        }
    }
}

/// Returns reserved global permits a pass didn't use, waking any kind they
/// were handed to (never the returning kind itself).
fn give_back(shared: &Shared, count: usize) {
    for kind in shared.arbiter.give_back(count) {
        if let Some(wake) = shared.wakes.get(&kind) {
            wake.notify_one();
        }
    }
}
