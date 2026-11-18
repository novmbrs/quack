//! The pool's transactions, their retries and timeouts, and the fence.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use anyhow::Result;
use common::{db, db_with, guild};
use quack_db::{DbError, Fence};
use quack_telemetry::labels::DbErrorClass;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn serialization_failures_are_retried(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = db_with(options, 6).await?;
    let g = guild(&db, 1).await?;
    let runs = Arc::new(AtomicU32::new(0));
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let mut tasks = Vec::new();
    for _ in 0..2 {
        let (db, runs, barrier) = (db.clone(), Arc::clone(&runs), Arc::clone(&barrier));
        tasks.push(tokio::spawn(async move {
            db.transaction(move |tx| {
                let runs = Arc::clone(&runs);
                let barrier = Arc::clone(&barrier);
                Box::pin(async move {
                    let attempt = runs.fetch_add(1, Ordering::SeqCst);
                    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
                        .execute(&mut **tx)
                        .await?;
                    let last: i64 = sqlx::query_scalar(
                        "SELECT last_number FROM case_counters WHERE guild_id = $1",
                    )
                    .bind(i64::from(g))
                    .fetch_one(&mut **tx)
                    .await?;
                    // Both first runs read before either writes.
                    if attempt < 2 {
                        barrier.wait().await;
                    }
                    sqlx::query("UPDATE case_counters SET last_number = $2 WHERE guild_id = $1")
                        .bind(i64::from(g))
                        .bind(last + 1)
                        .execute(&mut **tx)
                        .await?;
                    Ok::<_, DbError>(())
                })
            })
            .await
        }));
    }
    for task in tasks {
        task.await??;
    }
    assert!(
        runs.load(Ordering::SeqCst) >= 3,
        "the losing transaction ran again"
    );
    let mut conn = db.conn().await?;
    let last: i64 = sqlx::query_scalar("SELECT last_number FROM case_counters")
        .fetch_one(&mut *conn)
        .await?;
    assert_eq!(last, 2, "both increments landed");
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn lock_timeouts_are_unavailable_and_drops_roll_back(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = db(options).await?;
    let g = guild(&db, 1).await?;
    let mut holder = db.begin().await?;
    quack_db::guilds::next_case_number(&mut holder, g).await?;

    let mut waiter = db.begin().await?;
    waiter
        .set_timeouts(Duration::from_secs(2), Duration::from_millis(200))
        .await?;
    let blocked = quack_db::guilds::next_case_number(&mut waiter, g).await;
    assert!(matches!(
        blocked,
        Err(DbError::Unavailable(DbErrorClass::Timeout, _))
    ));
    drop(waiter);

    // Dropping the holder rolls back: the number is not consumed.
    drop(holder);
    let mut conn = db.conn().await?;
    assert_eq!(quack_db::guilds::next_case_number(&mut conn, g).await?, 1);
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn only_one_process_holds_the_fence(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let mut first = Fence::try_acquire(&options)
        .await?
        .ok_or_else(|| anyhow::anyhow!("first"))?;
    assert!(Fence::try_acquire(&options).await?.is_none());
    assert!(first.check().await);
    first.release().await?;
    let second = Fence::try_acquire(&options).await?;
    assert!(second.is_some());
    Ok(())
}
