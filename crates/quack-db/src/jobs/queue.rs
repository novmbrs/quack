//! The queue's SQL: enqueue, claim, fence, settle, lease sweep, gauges.

use std::time::Duration;

use quack_core::ids::JobId;
use sqlx::PgConnection;

use super::{ClaimedJob, JobKind, JobResult, NewJob, RunAt, short_error};
use crate::obs::{self, AfterCommit, Delete, Insert, Select, Update};
use crate::pg::{Pg, count, opt_id};
use crate::{DbError, Tx};

/// Enqueues `job` in the caller's transaction and notifies its kind (§16.3).
///
/// A key that already exists is coalesced: a waiting job moves earlier, a
/// claimed one records `rerun_at` so it runs once more after it finishes,
/// and a dead one is revived with a fresh budget. The stored trace context
/// stays the first enqueuer's. `quack.jobs.enqueued` counts on commit.
pub async fn enqueue(tx: &mut Tx, job: &NewJob) -> Result<JobId, DbError> {
    let (delay, at) = job.run_at.sql();
    let guild = job.guild.map(i64::from);
    let trace = quack_telemetry::current_trace_context().filter(|t| t.len() <= 256);
    let id = obs::run(
        Insert,
        "jobs",
        sqlx::query_scalar!(
            r#"INSERT INTO jobs (id, kind, key, guild_id, payload, run_at, max_attempts, trace_context)
               VALUES ($1, $2, $3, $4, $5, COALESCE($6, now() + make_interval(secs => $7)), $8, $9)
               ON CONFLICT (key) DO UPDATE SET
                 run_at     = CASE WHEN jobs.locked_until IS NULL
                                   THEN LEAST(jobs.run_at, EXCLUDED.run_at) ELSE jobs.run_at END,
                 rerun_at   = CASE WHEN jobs.locked_until IS NOT NULL
                                   THEN LEAST(COALESCE(jobs.rerun_at, EXCLUDED.run_at), EXCLUDED.run_at)
                                   ELSE jobs.rerun_at END,
                 attempts   = CASE WHEN jobs.dead_at IS NOT NULL THEN 0 ELSE jobs.attempts END,
                 dead_at    = NULL,
                 updated_at = now()
               RETURNING id"#,
            uuid::Uuid::now_v7(),
            Pg(job.kind),
            job.key.as_str(),
            guild,
            job.payload,
            at,
            delay,
            i32::try_from(job.kind.policy().max_attempts).unwrap_or(i32::MAX),
            trace,
        )
        .fetch_one(&mut **tx),
    )
    .await?;
    notify_kind(tx, job.kind).await?;
    tx.after_commit(AfterCommit::JobEnqueued(job.kind));
    Ok(JobId::from_uuid(id))
}

async fn notify_kind(conn: &mut PgConnection, kind: JobKind) -> Result<(), DbError> {
    obs::run(
        obs::Notify,
        "quack_jobs",
        sqlx::query!("SELECT pg_notify('quack_jobs', $1)", kind.as_str()).execute(conn),
    )
    .await?;
    Ok(())
}

/// Ensures the periodic `housekeeping` job exists (at startup).
pub async fn ensure_housekeeping(conn: &mut PgConnection) -> Result<(), DbError> {
    let job = NewJob::housekeeping();
    obs::run(
        Insert,
        "jobs",
        sqlx::query!(
            "INSERT INTO jobs (id, kind, key, payload, max_attempts) VALUES ($1, 'housekeeping', $2, '{}', 1)
             ON CONFLICT (key) DO NOTHING",
            uuid::Uuid::now_v7(),
            job.key.as_str(),
        )
        .execute(conn),
    )
    .await?;
    Ok(())
}

/// Claims up to `limit` due jobs of `kind` (§16.2): a fresh claim token, a
/// lease of the kind's length, and `attempts + 1`, all from PostgreSQL.
pub async fn claim(
    conn: &mut PgConnection,
    kind: JobKind,
    limit: usize,
) -> Result<Vec<ClaimedJob>, DbError> {
    let lease = kind.policy().lease.as_secs_f64();
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let rows = obs::run(
        Update,
        "jobs",
        sqlx::query!(
            r#"WITH picked AS (
                 SELECT id FROM jobs
                 WHERE kind = $1 AND dead_at IS NULL AND locked_until IS NULL AND run_at <= now()
                 ORDER BY run_at, id
                 LIMIT $2
                 FOR UPDATE SKIP LOCKED
               )
               UPDATE jobs j
               SET claim_token  = gen_random_uuid(),
                   locked_until = now() + make_interval(secs => $3),
                   attempts     = j.attempts + 1,
                   updated_at   = now()
               FROM picked
               WHERE j.id = picked.id
               RETURNING j.id, j.key, j.guild_id, j.payload, j.attempts, j.max_attempts,
                         j.claim_token AS "claim_token!", j.locked_until AS "locked_until!",
                         j.trace_context, j.run_at, j.created_at"#,
            Pg(kind),
            limit,
            lease,
        )
        .fetch_all(conn),
    )
    .await?;
    rows.into_iter()
        .map(|r| {
            Ok(ClaimedJob {
                id: JobId::from_uuid(r.id),
                kind,
                key: r.key,
                guild: opt_id(r.guild_id)?,
                payload: r.payload,
                attempt: count(r.attempts)?,
                max_attempts: count(r.max_attempts)?,
                claim_token: r.claim_token,
                locked_until: r.locked_until,
                trace_context: r.trace_context,
                run_at: r.run_at,
                created_at: r.created_at,
            })
        })
        .collect()
}

/// Unclaims every job of `kind` whose lease ended (§16.1) and returns how
/// many. The next run finds the domain record mid-effect and resolves it.
pub async fn sweep_leases(conn: &mut PgConnection, kind: JobKind) -> Result<u64, DbError> {
    let done = obs::run(
        Update,
        "jobs",
        sqlx::query!(
            "UPDATE jobs SET claim_token = NULL, locked_until = NULL, updated_at = now()
             WHERE locked_until < now() AND dead_at IS NULL AND kind = $1",
            Pg(kind),
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected())
}

/// The fence: proves this claim still holds. Run it as the last statement
/// of every handler transaction; [`DbError::Stale`] means the lease was
/// lost and the transaction must roll back.
pub async fn fence(conn: &mut PgConnection, job: &ClaimedJob) -> Result<(), DbError> {
    let done = obs::run(
        Update,
        "jobs",
        sqlx::query!(
            "UPDATE jobs SET updated_at = now() WHERE id = $1 AND claim_token = $2 AND locked_until > now()",
            job.id.get(),
            job.claim_token,
        )
        .execute(conn),
    )
    .await?;
    if done.rows_affected() == 1 {
        Ok(())
    } else {
        Err(DbError::Stale)
    }
}

/// Applies a run's result under the fence (§16.1). [`DbError::Stale`]
/// means the claim no longer holds and nothing was changed.
pub async fn settle(
    conn: &mut PgConnection,
    job: &ClaimedJob,
    result: &JobResult,
) -> Result<(), DbError> {
    let id = job.id.get();
    let token = job.claim_token;
    let applied = match result {
        JobResult::Done => {
            let deleted = obs::run(
                Delete,
                "jobs",
                sqlx::query!(
                    "DELETE FROM jobs WHERE id = $1 AND claim_token = $2 AND locked_until > now() AND rerun_at IS NULL",
                    id,
                    token
                )
                .execute(&mut *conn),
            )
            .await?;
            deleted.rows_affected() == 1
                || obs::run(
                    Update,
                    "jobs",
                    sqlx::query!(
                        "UPDATE jobs SET claim_token = NULL, locked_until = NULL, attempts = 0, run_at = rerun_at,
                                         rerun_at = NULL, last_error = NULL, updated_at = now()
                         WHERE id = $1 AND claim_token = $2 AND locked_until > now() AND rerun_at IS NOT NULL",
                        id,
                        token
                    )
                    .execute(&mut *conn),
                )
                .await?
                .rows_affected()
                    == 1
        }
        JobResult::Retry { at, error } => {
            unclaim(conn, job, *at, false, Some(&short_error(error))).await?
        }
        JobResult::Defer { at } => unclaim(conn, job, *at, true, None).await?,
        JobResult::Continue => unclaim(conn, job, RunAt::Now, true, None).await?,
        JobResult::Dead { error } => {
            let error = short_error(error);
            obs::run(
                Update,
                "jobs",
                sqlx::query!(
                    "UPDATE jobs SET dead_at = now(), claim_token = NULL, locked_until = NULL, rerun_at = NULL,
                                     last_error = $3, updated_at = now()
                     WHERE id = $1 AND claim_token = $2 AND locked_until > now()",
                    id,
                    token,
                    error
                )
                .execute(&mut *conn),
            )
            .await?
            .rows_affected()
                == 1
        }
        JobResult::Reschedule { at } => {
            let (delay, at) = at.sql();
            obs::run(
                Update,
                "jobs",
                sqlx::query!(
                    "UPDATE jobs SET claim_token = NULL, locked_until = NULL, attempts = 0, last_error = NULL,
                                     run_at = LEAST(COALESCE($3, now() + make_interval(secs => $4)),
                                                    COALESCE(rerun_at, 'infinity')),
                                     rerun_at = NULL, updated_at = now()
                     WHERE id = $1 AND claim_token = $2 AND locked_until > now()",
                    id,
                    token,
                    at,
                    delay
                )
                .execute(&mut *conn),
            )
            .await?
            .rows_affected()
                == 1
        }
    };
    if applied { Ok(()) } else { Err(DbError::Stale) }
}

/// Retry, Defer, and Continue: unclaim and run again at `at` (or earlier,
/// when the job was enqueued again meanwhile). `uncount` gives the run back.
async fn unclaim(
    conn: &mut PgConnection,
    job: &ClaimedJob,
    at: RunAt,
    uncount: bool,
    error: Option<&str>,
) -> Result<bool, DbError> {
    let (delay, at) = at.sql();
    let done = obs::run(
        Update,
        "jobs",
        sqlx::query!(
            "UPDATE jobs SET claim_token = NULL, locked_until = NULL,
                             run_at = LEAST(COALESCE($3, now() + make_interval(secs => $4)),
                                            COALESCE(rerun_at, 'infinity')),
                             rerun_at = NULL,
                             attempts = CASE WHEN $5 THEN GREATEST(attempts - 1, 0) ELSE attempts END,
                             last_error = COALESCE($6, last_error),
                             updated_at = now()
             WHERE id = $1 AND claim_token = $2 AND locked_until > now()",
            job.id.get(),
            job.claim_token,
            at,
            delay,
            uncount,
            error,
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Moves a waiting job earlier to now (e.g. a kick waiting for its DM,
/// §6.2) and notifies its kind. Returns whether a waiting job was moved.
pub async fn wake(conn: &mut PgConnection, job: &NewJob) -> Result<bool, DbError> {
    let done = obs::run(
        Update,
        "jobs",
        sqlx::query!(
            "UPDATE jobs SET run_at = now(), updated_at = now()
             WHERE key = $1 AND locked_until IS NULL AND dead_at IS NULL AND run_at > now()",
            job.key.as_str(),
        )
        .execute(&mut *conn),
    )
    .await?;
    if done.rows_affected() == 0 {
        return Ok(false);
    }
    notify_kind(conn, job.kind).await?;
    Ok(true)
}

/// How long until the earliest unclaimed job of `kind` is due (zero when
/// one is due now), or `None` when there is none.
pub async fn next_due_in(
    conn: &mut PgConnection,
    kind: JobKind,
) -> Result<Option<Duration>, DbError> {
    let secs = obs::run(
        Select,
        "jobs",
        sqlx::query_scalar!(
            r#"SELECT EXTRACT(EPOCH FROM min(run_at) - now())::float8 AS "secs"
               FROM jobs WHERE kind = $1 AND dead_at IS NULL AND locked_until IS NULL"#,
            Pg(kind),
        )
        .fetch_one(conn),
    )
    .await?;
    Ok(secs.map(|s| Duration::try_from_secs_f64(s.max(0.0)).unwrap_or(Duration::ZERO)))
}

/// The ready backlog and dead jobs of one kind, for the gauges (§16.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JobBacklog {
    /// The kind.
    pub kind: JobKind,
    /// Unclaimed, due, not dead.
    pub ready: u64,
    /// How long the oldest ready job has been due.
    pub oldest_ready_age: Duration,
    /// Dead jobs kept for inspection.
    pub dead: u64,
}

impl JobBacklog {
    /// This backlog as the `quack.jobs.ready` / `oldest_ready.age` / `dead`
    /// gauge sample (the 60-second gauge task installs these).
    pub fn sample(&self) -> quack_telemetry::JobQueueSample {
        crate::obs::backlog_sample(self)
    }
}

/// [`JobBacklog`] for every kind; kinds without rows report zero.
pub async fn backlog(conn: &mut PgConnection) -> Result<Vec<JobBacklog>, DbError> {
    let rows = obs::run(
        Select,
        "jobs",
        sqlx::query!(
            r#"SELECT kind,
                      count(*) FILTER (WHERE dead_at IS NULL AND locked_until IS NULL AND run_at <= now()) AS "ready!",
                      EXTRACT(EPOCH FROM now() - min(run_at)
                        FILTER (WHERE dead_at IS NULL AND locked_until IS NULL AND run_at <= now()))::float8 AS "age",
                      count(*) FILTER (WHERE dead_at IS NOT NULL) AS "dead!"
               FROM jobs GROUP BY kind"#
        )
        .fetch_all(conn),
    )
    .await?;
    Ok(JobKind::ALL
        .iter()
        .map(|kind| {
            let row = rows.iter().find(|r| r.kind.0 == *kind);
            JobBacklog {
                kind: *kind,
                ready: row.map_or(0, |r| u64::try_from(r.ready).unwrap_or(0)),
                oldest_ready_age: row
                    .and_then(|r| r.age)
                    .and_then(|s| Duration::try_from_secs_f64(s.max(0.0)).ok())
                    .unwrap_or(Duration::ZERO),
                dead: row.map_or(0, |r| u64::try_from(r.dead).unwrap_or(0)),
            }
        })
        .collect())
}
