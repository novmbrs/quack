//! The generic runner: polling without notifications, retries into
//! finalize, and graceful stop.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use anyhow::Result;
use common::{db, guild};
use quack_core::ids::{DmId, GuildId};
use quack_db::Db;
use quack_db::jobs::{
    self, Handled, Handler, JobKind, JobResult, NewJob, Phase, RunAt, Runner, RunnerConfig,
};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

fn config() -> RunnerConfig {
    RunnerConfig {
        kinds: vec![JobKind::SendDm],
        poll: Duration::from_millis(200),
        ..RunnerConfig::new(4)
    }
}

/// Inserts a due job directly, without the `NOTIFY` that `enqueue` sends,
/// as if the notification were lost.
async fn insert_silently(db: &Db, g: GuildId) -> Result<()> {
    let job = NewJob::send_dm(g, DmId::from_uuid(Uuid::now_v7()));
    let mut conn = db.conn().await?;
    sqlx::query(
        "INSERT INTO jobs (kind, key, guild_id, max_attempts) VALUES ('send_dm', $1, $2, 3)",
    )
    .bind(job.key().as_str())
    .bind(i64::from(g))
    .execute(&mut *conn)
    .await?;
    Ok(())
}

async fn remaining(db: &Db) -> Result<i64> {
    let mut conn = db.conn().await?;
    Ok(sqlx::query_scalar("SELECT count(*) FROM jobs")
        .fetch_one(&mut *conn)
        .await?)
}

async fn eventually(mut check: impl AsyncFnMut() -> Result<bool>) -> Result<bool> {
    for _ in 0..50 {
        if check().await? {
            return Ok(true);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(false)
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn polling_picks_up_a_job_whose_notification_was_lost(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = db(options).await?;
    let g = guild(&db, 1).await?;
    let runs = Arc::new(AtomicU32::new(0));
    let counter = Arc::clone(&runs);
    let handler: Handler = Arc::new(move |_job, _phase| {
        counter.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(Handled::Settle(JobResult::Done)) })
    });
    let runner = Runner::new(db.clone(), config(), handler);
    runner.start();
    tokio::time::sleep(Duration::from_millis(300)).await;
    insert_silently(&db, g).await?;
    assert!(eventually(async || Ok(remaining(&db).await? == 0)).await?);
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    assert!(runner.heartbeats_fresh(Duration::from_secs(5)));
    runner.shutdown(Duration::from_secs(5)).await;
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn a_retry_on_the_last_attempt_finalizes(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = db(options).await?;
    let g = guild(&db, 1).await?;
    let runs = Arc::new(AtomicU32::new(0));
    let finals = Arc::new(AtomicU32::new(0));
    let (r, f) = (Arc::clone(&runs), Arc::clone(&finals));
    let handler: Handler = Arc::new(move |_job, phase| {
        let result = match phase {
            Phase::Run => {
                r.fetch_add(1, Ordering::SeqCst);
                JobResult::Retry {
                    at: RunAt::Now,
                    error: "discord_unavailable".to_owned(),
                }
            }
            Phase::Finalize => {
                f.fetch_add(1, Ordering::SeqCst);
                JobResult::Done
            }
        };
        Box::pin(async move { Ok(Handled::Settle(result)) })
    });
    let runner = Runner::new(db.clone(), config(), handler);
    runner.start();
    let mut tx = db.begin().await?;
    jobs::enqueue(
        &mut tx,
        &NewJob::send_dm(g, DmId::from_uuid(Uuid::now_v7())),
    )
    .await?;
    tx.commit().await?;
    runner.wake(JobKind::SendDm);
    assert!(eventually(async || Ok(remaining(&db).await? == 0)).await?);
    runner.shutdown(Duration::from_secs(5)).await;
    assert_eq!(
        runs.load(Ordering::SeqCst),
        JobKind::SendDm.policy().max_attempts
    );
    assert_eq!(finals.load(Ordering::SeqCst), 1);
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn shutdown_stops_claiming_and_drains_running_handlers(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = db(options).await?;
    let g = guild(&db, 1).await?;
    let started = Arc::new(tokio::sync::Notify::new());
    let signal = Arc::clone(&started);
    let handler: Handler = Arc::new(move |_job, _phase| {
        let signal = Arc::clone(&signal);
        Box::pin(async move {
            signal.notify_one();
            tokio::time::sleep(Duration::from_millis(500)).await;
            Ok(Handled::Settle(JobResult::Done))
        })
    });
    let runner = Runner::new(db.clone(), config(), handler);
    runner.start();
    insert_silently(&db, g).await?;
    tokio::time::timeout(Duration::from_secs(5), started.notified()).await?;
    runner.shutdown(Duration::from_secs(5)).await;
    assert_eq!(
        remaining(&db).await?,
        0,
        "the running handler finished and settled"
    );

    // After shutdown nothing is claimed any more.
    insert_silently(&db, g).await?;
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(remaining(&db).await?, 1);
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn a_drain_deadline_drops_handlers_without_settling(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = db(options).await?;
    let g = guild(&db, 1).await?;
    let started = Arc::new(tokio::sync::Notify::new());
    let signal = Arc::clone(&started);
    let handler: Handler = Arc::new(move |_job, _phase| {
        let signal = Arc::clone(&signal);
        Box::pin(async move {
            signal.notify_one();
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok(Handled::Settle(JobResult::Done))
        })
    });
    let runner = Runner::new(db.clone(), config(), handler);
    runner.start();
    insert_silently(&db, g).await?;
    tokio::time::timeout(Duration::from_secs(5), started.notified()).await?;
    let stopping = std::time::Instant::now();
    runner.shutdown(Duration::from_millis(300)).await;
    assert!(stopping.elapsed() < Duration::from_secs(5));
    let mut conn = db.conn().await?;
    let claimed: bool = sqlx::query_scalar("SELECT locked_until IS NOT NULL FROM jobs")
        .fetch_one(&mut *conn)
        .await?;
    assert!(claimed, "the job stays claimed until its lease expires");
    Ok(())
}
