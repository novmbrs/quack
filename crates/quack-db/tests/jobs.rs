//! The job queue's SQL under concurrency: coalescing, claims, leases,
//! fences, and settling.

mod common;

use std::collections::HashSet;
use std::time::Duration;

use anyhow::{Context as _, Result};
use common::{HEAVY, db, db_with, guild};
use quack_core::ids::{DmId, GuildId, MessageRowId};
use quack_db::jobs::{self, ClaimedJob, JobKind, JobResult, NewJob, RunAt};
use quack_db::{Db, DbError, notify};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

async fn enqueue(db: &Db, job: &NewJob) -> Result<()> {
    let mut tx = db.begin().await?;
    jobs::enqueue(&mut tx, job).await?;
    tx.commit().await?;
    Ok(())
}

async fn claim_one(db: &Db, kind: JobKind) -> Result<Option<ClaimedJob>> {
    let mut conn = db.conn().await?;
    Ok(jobs::claim(&mut conn, kind, 1).await?.into_iter().next())
}

async fn settle(db: &Db, job: &ClaimedJob, result: JobResult) -> Result<(), DbError> {
    let mut tx = db.begin().await?;
    jobs::settle(&mut tx, job, &result).await?;
    tx.commit().await
}

/// `(run_at - now, rerun_at - now, attempts, dead)` of the job with `key`.
async fn row(db: &Db, key: &str) -> Result<Option<(f64, Option<f64>, i32, bool)>> {
    let mut conn = db.conn().await?;
    Ok(sqlx::query_as(
        "SELECT EXTRACT(EPOCH FROM run_at - now())::float8, EXTRACT(EPOCH FROM rerun_at - now())::float8,
                attempts, dead_at IS NOT NULL
         FROM jobs WHERE key = $1",
    )
    .bind(key)
    .fetch_optional(&mut *conn)
    .await?)
}

/// Ends the lease of every claimed job, as if its worker had crashed.
async fn expire_leases(db: &Db) -> Result<()> {
    let mut conn = db.conn().await?;
    sqlx::query(
        "UPDATE jobs SET locked_until = now() - interval '1 second' WHERE locked_until IS NOT NULL",
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

fn row_job(g: GuildId) -> NewJob {
    NewJob::sync_message(g, MessageRowId::from_uuid(Uuid::now_v7()))
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn a_key_exists_once_and_coalesces(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = db(options).await?;
    let g = guild(&db, 1).await?;
    let job = row_job(g);
    let key = job.key().as_str().to_owned();

    // Waiting: a second enqueue moves it earlier, never later.
    enqueue(&db, &job.clone().at(RunAt::In(Duration::from_mins(10)))).await?;
    enqueue(&db, &job.clone().at(RunAt::In(Duration::from_mins(20)))).await?;
    let (run_in, ..) = row(&db, &key).await?.context("row")?;
    assert!((590.0..=600.0).contains(&run_in), "{run_in}");
    enqueue(&db, &job).await?;
    let (run_in, rerun, ..) = row(&db, &key).await?.context("row")?;
    assert!(run_in <= 0.0 && rerun.is_none());

    // Claimed: enqueueing records a rerun instead of touching the claim.
    let claimed = claim_one(&db, JobKind::SyncMessage)
        .await?
        .context("claim")?;
    assert_eq!(claimed.attempt, 1);
    enqueue(&db, &job.clone().at(RunAt::In(Duration::from_secs(30)))).await?;
    enqueue(&db, &job.clone().at(RunAt::In(Duration::from_secs(60)))).await?;
    let (_, rerun, ..) = row(&db, &key).await?.context("row")?;
    assert!(
        rerun.is_some_and(|r| (20.0..=30.0).contains(&r)),
        "{rerun:?}"
    );
    assert!(
        claim_one(&db, JobKind::SyncMessage).await?.is_none(),
        "claimed jobs are not claimed again"
    );

    // Done with a rerun pending keeps the job, unclaimed, with a fresh budget.
    settle(&db, &claimed, JobResult::Done).await?;
    let (run_in, rerun, attempts, _) = row(&db, &key).await?.context("still there")?;
    assert!((20.0..=30.0).contains(&run_in) && rerun.is_none() && attempts == 0);

    // Done without a rerun deletes it.
    let mut conn = db.conn().await?;
    sqlx::query("UPDATE jobs SET run_at = now() WHERE key = $1")
        .bind(&key)
        .execute(&mut *conn)
        .await?;
    let claimed = claim_one(&db, JobKind::SyncMessage)
        .await?
        .context("claim")?;
    settle(&db, &claimed, JobResult::Done).await?;
    assert!(row(&db, &key).await?.is_none());
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn a_dead_job_is_revived_with_a_fresh_budget(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = db(options).await?;
    let g = guild(&db, 1).await?;
    let job = NewJob::send_dm(g, DmId::from_uuid(Uuid::now_v7()));
    let key = job.key().as_str().to_owned();
    enqueue(&db, &job).await?;
    let claimed = claim_one(&db, JobKind::SendDm).await?.context("claim")?;
    settle(
        &db,
        &claimed,
        JobResult::Dead {
            error: "dm_missing".to_owned(),
        },
    )
    .await?;
    let (.., dead) = row(&db, &key).await?.context("row")?;
    assert!(dead);
    assert!(
        claim_one(&db, JobKind::SendDm).await?.is_none(),
        "dead jobs never run"
    );
    let mut conn = db.conn().await?;
    let backlog = jobs::backlog(&mut conn).await?;
    let dm = backlog
        .iter()
        .find(|b| b.kind == JobKind::SendDm)
        .context("kind")?;
    assert_eq!((dm.ready, dm.dead), (0, 1));

    enqueue(&db, &job).await?;
    let (_, _, attempts, dead) = row(&db, &key).await?.context("row")?;
    assert_eq!((attempts, dead), (0, false));
    assert_eq!(
        claim_one(&db, JobKind::SendDm).await?.map(|j| j.attempt),
        Some(1)
    );
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn concurrent_workers_never_claim_a_job_twice(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let _heavy = HEAVY.lock().await;
    let db = db_with(options, 16).await?;
    let g = guild(&db, 1).await?;
    let mut tx = db.begin().await?;
    for _ in 0..300 {
        jobs::enqueue(&mut tx, &row_job(g)).await?;
    }
    tx.commit().await?;

    let mut workers = Vec::new();
    for _ in 0..12 {
        let db = db.clone();
        workers.push(tokio::spawn(async move {
            let mut seen = Vec::new();
            loop {
                let mut conn = db.conn().await?;
                let batch = jobs::claim(&mut conn, JobKind::SyncMessage, 4).await?;
                if batch.is_empty() {
                    return Ok::<_, anyhow::Error>(seen);
                }
                seen.extend(batch.into_iter().map(|j| j.id));
            }
        }));
    }
    let mut all = Vec::new();
    for worker in workers {
        all.extend(worker.await??);
    }
    let unique: HashSet<_> = all.iter().collect();
    assert_eq!(all.len(), 300);
    assert_eq!(unique.len(), 300);
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn expired_leases_are_reclaimed_but_never_beyond_the_budget(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = db(options).await?;
    let g = guild(&db, 1).await?;
    enqueue(&db, &NewJob::send_dm(g, DmId::from_uuid(Uuid::now_v7()))).await?;
    let max = JobKind::SendDm.policy().max_attempts;
    let mut handler_runs = 0;
    let mut previous: Option<ClaimedJob> = None;
    for _ in 0..10 {
        let mut conn = db.conn().await?;
        jobs::sweep_leases(&mut conn, JobKind::SendDm).await?;
        let Some(job) = jobs::claim(&mut conn, JobKind::SendDm, 1)
            .await?
            .into_iter()
            .next()
        else {
            break;
        };
        // The crashed run's lease is gone: its fence and settle fail.
        if let Some(old) = previous.take() {
            let mut tx = db.begin().await?;
            assert!(matches!(
                jobs::fence(&mut tx, &old).await,
                Err(DbError::Stale)
            ));
            drop(tx);
            assert!(matches!(
                settle(&db, &old, JobResult::Done).await,
                Err(DbError::Stale)
            ));
        }
        if job.budget_spent() {
            // The runner finalizes (database only) and finishes the job.
            settle(&db, &job, JobResult::Done).await?;
            break;
        }
        handler_runs += 1;
        // The worker "crashes": nothing is settled and the lease runs out.
        previous = Some(job);
        expire_leases(&db).await?;
    }
    assert_eq!(handler_runs, max);
    assert!(claim_one(&db, JobKind::SendDm).await?.is_none());
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn settle_applies_each_result(_: PgPoolOptions, options: PgConnectOptions) -> Result<()> {
    let db = db(options).await?;
    let g = guild(&db, 1).await?;
    let job = row_job(g);
    let key = job.key().as_str().to_owned();
    enqueue(&db, &job).await?;

    let claimed = claim_one(&db, JobKind::SyncMessage)
        .await?
        .context("claim")?;
    settle(
        &db,
        &claimed,
        JobResult::Retry {
            at: RunAt::Now,
            error: "http_500".to_owned(),
        },
    )
    .await?;
    assert_eq!(
        row(&db, &key).await?.map(|r| r.2),
        Some(1),
        "a retry counts"
    );

    let claimed = claim_one(&db, JobKind::SyncMessage)
        .await?
        .context("claim")?;
    assert_eq!(claimed.attempt, 2);
    settle(&db, &claimed, JobResult::Defer { at: RunAt::Now }).await?;
    assert_eq!(
        row(&db, &key).await?.map(|r| r.2),
        Some(1),
        "a deferral gives the run back"
    );

    let claimed = claim_one(&db, JobKind::SyncMessage)
        .await?
        .context("claim")?;
    settle(&db, &claimed, JobResult::Continue).await?;
    let claimed = claim_one(&db, JobKind::SyncMessage)
        .await?
        .context("claim")?;
    assert_eq!(claimed.attempt, 2);
    settle(
        &db,
        &claimed,
        JobResult::Reschedule {
            at: RunAt::In(Duration::from_secs(600)),
        },
    )
    .await?;
    let (run_in, _, attempts, _) = row(&db, &key).await?.context("row")?;
    assert!(run_in > 500.0 && attempts == 0);

    // `wake` brings a waiting job forward.
    let mut conn = db.conn().await?;
    assert!(jobs::wake(&mut conn, &job).await?);
    assert!(!jobs::wake(&mut conn, &job).await?, "already due");
    assert_eq!(
        jobs::next_due_in(&mut conn, JobKind::SyncMessage).await?,
        Some(Duration::ZERO)
    );
    let mut tx = db.begin().await?;
    let claimed = jobs::claim(&mut tx, JobKind::SyncMessage, 1).await?;
    let claimed = claimed.first().context("claim")?;
    // A fence inside the handler's transaction passes while the lease holds.
    jobs::fence(&mut tx, claimed).await?;
    jobs::settle(&mut tx, claimed, &JobResult::Done).await?;
    tx.commit().await?;
    assert!(row(&db, &key).await?.is_none());
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn the_listener_hears_commits_only(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = db(options.clone()).await?;
    let g = guild(&db, 1).await?;
    let mut listener = notify::Listener::connect(&options).await?;

    let mut rolled_back = db.begin().await?;
    jobs::enqueue(&mut rolled_back, &row_job(g)).await?;
    rolled_back.rollback().await?;
    let mut tx = db.begin().await?;
    jobs::enqueue(
        &mut tx,
        &NewJob::send_dm(g, DmId::from_uuid(Uuid::now_v7())),
    )
    .await?;
    notify::rules_changed(&mut tx, g).await?;
    tx.commit().await?;

    let first = tokio::time::timeout(Duration::from_secs(5), listener.recv()).await??;
    let second = tokio::time::timeout(Duration::from_secs(5), listener.recv()).await??;
    assert_eq!(first, notify::Notification::Job(JobKind::SendDm));
    assert_eq!(second, notify::Notification::Rules(g));
    Ok(())
}
