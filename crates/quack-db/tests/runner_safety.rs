//! Runner safety regressions (from the quack-db review): an idle runner
//! sleeps, expired claims never run, shutdown starts nothing new, finalize
//! failures end, and a waiting kind keeps its place for the next permit.

#![allow(clippy::unwrap_used, clippy::panic)]

use anyhow::Result;
use quack_db::{
    Db, DbError,
    jobs::{self, Handled, Handler, JobKind, JobResult, Runner, RunnerConfig},
};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::{
    sync::Mutex,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};

const WAIT: Duration = Duration::from_secs(5);

async fn insert(db: &Db) -> Result<()> {
    let mut conn = db.conn().await?;
    sqlx::query(
        "INSERT INTO jobs (kind,key,max_attempts) VALUES ('housekeeping','housekeeping',1)",
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn fence_passes_after_real_expiry(_: PgPoolOptions, options: PgConnectOptions) -> Result<()> {
    let db = Db::connect_with(options, 4).await?;
    insert(&db).await?;
    let mut conn = db.conn().await?;
    let job = jobs::claim(&mut conn, JobKind::Housekeeping, 1)
        .await?
        .remove(0);
    sqlx::query("UPDATE jobs SET locked_until=clock_timestamp()+interval '500 ms'")
        .execute(&mut *conn)
        .await?;
    let mut tx = db.begin().await?;
    loop {
        let expired: bool = sqlx::query_scalar("SELECT clock_timestamp() > locked_until FROM jobs")
            .fetch_one(&mut *tx)
            .await?;
        if expired {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let got = jobs::fence(&mut tx, &job).await;
    tx.rollback().await?;
    assert!(
        matches!(got, Err(DbError::Stale)),
        "expired fence returned {got:?}"
    );
    Ok(())
}

fn config() -> RunnerConfig {
    RunnerConfig {
        kinds: vec![JobKind::Housekeeping],
        poll: Duration::from_secs(30),
        ..RunnerConfig::new(1)
    }
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn idle_dispatcher_sleeps(_: PgPoolOptions, options: PgConnectOptions) -> Result<()> {
    let db = Db::connect_with(options, 4).await?;
    let handler: Handler = Arc::new(|_, _| panic!("no jobs"));
    let mut conn = db.conn().await?;
    let snapshot = "SELECT xact_commit FROM pg_stat_database WHERE datname=current_database()";
    let before: i64 = sqlx::query_scalar(snapshot).fetch_one(&mut *conn).await?;
    let runner = Runner::new(db.clone(), config(), handler);
    runner.start();
    tokio::time::sleep(Duration::from_millis(1800)).await;
    runner.shutdown(WAIT).await;
    let after: i64 = sqlx::query_scalar(snapshot).fetch_one(&mut *conn).await?;
    assert!(
        after - before < 30,
        "idle database committed {} transactions in 1.8 seconds",
        after - before
    );
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn shutdown_does_not_launch_pending_claim(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = Db::connect_with(options, 4).await?;
    insert(&db).await?;
    let mut gate = db.conn().await?;
    sqlx::raw_sql("CREATE FUNCTION gate_claim() RETURNS trigger LANGUAGE plpgsql AS $$BEGIN IF OLD.claim_token IS NULL AND NEW.claim_token IS NOT NULL THEN PERFORM pg_advisory_xact_lock(91342); END IF; RETURN NEW; END$$;
        CREATE TRIGGER gate_claim BEFORE UPDATE ON jobs FOR EACH ROW EXECUTE FUNCTION gate_claim(); SELECT pg_advisory_lock(91342);")
        .execute(&mut *gate).await?;
    let runs = Arc::new(AtomicU32::new(0));
    let observed = Arc::clone(&runs);
    let handler: Handler = Arc::new(move |_, _| {
        observed.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(Handled::Settle(JobResult::Done)) })
    });
    let runner = Runner::new(db, config(), handler);
    runner.start();
    tokio::time::timeout(WAIT, async {
        while !sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM pg_locks WHERE locktype='advisory' AND objid=91342 AND NOT granted)")
            .fetch_one(&mut *gate).await? {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok::<_, sqlx::Error>(())
    }).await??;
    let shutdown = runner.shutdown(WAIT);
    tokio::pin!(shutdown);
    tokio::select! {
        () = &mut shutdown => panic!("shutdown returned while the claim was held"),
        () = tokio::time::sleep(Duration::from_millis(50)) => {}
    };
    sqlx::query("SELECT pg_advisory_unlock(91342)")
        .execute(&mut *gate)
        .await?;
    shutdown.await;
    assert_eq!(
        runs.load(Ordering::SeqCst),
        0,
        "handler launched after shutdown"
    );
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn broken_finalize_has_a_terminal_bound(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = Db::connect_with(options, 4).await?;
    insert(&db).await?;
    let mut conn = db.conn().await?;
    sqlx::query("UPDATE jobs SET attempts = max_attempts")
        .execute(&mut *conn)
        .await?;
    let calls = Arc::new(AtomicU32::new(0));
    let observed = Arc::clone(&calls);
    let handler: Handler = Arc::new(move |_, phase| {
        assert_eq!(phase, jobs::Phase::Finalize);
        observed.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            Err(DbError::Decode(
                "permanently invalid domain record".to_owned(),
            ))
        })
    });
    let runner = Runner::new(db.clone(), config(), handler);
    runner.start();
    for target in 1..=4 {
        let still_retrying =
            tokio::time::timeout(WAIT, async {
                loop {
                    let ready: Option<bool> = sqlx::query_scalar(
                    "SELECT dead_at IS NULL FROM jobs WHERE attempts>$1 AND claim_token IS NULL")
                    .bind(target)
                    .fetch_optional(&mut *conn).await?;
                    if let Some(still_retrying) = ready {
                        return Ok::<_, sqlx::Error>(still_retrying);
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await??;
        if !still_retrying {
            break;
        }
        sqlx::query("UPDATE jobs SET run_at=now()")
            .execute(&mut *conn)
            .await?;
        runner.wake(JobKind::Housekeeping);
    }
    runner.shutdown(WAIT).await;
    assert!(
        calls.load(Ordering::SeqCst) < 4,
        "permanent finalize errors retried continuously: {}",
        calls.load(Ordering::SeqCst)
    );
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn an_expired_claim_does_not_start_a_handler(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = Db::connect_with(options, 4).await?;
    insert(&db).await?;
    let mut conn = db.conn().await?;
    sqlx::raw_sql(
        "CREATE FUNCTION age_claim() RETURNS trigger LANGUAGE plpgsql AS $$BEGIN
        IF OLD.claim_token IS NULL AND NEW.claim_token IS NOT NULL THEN
            NEW.locked_until := clock_timestamp() - interval '1 second';
        END IF; RETURN NEW; END$$;
        CREATE TRIGGER age_claim BEFORE UPDATE ON jobs FOR EACH ROW EXECUTE FUNCTION age_claim();",
    )
    .execute(&mut *conn)
    .await?;
    let expired_starts = Arc::new(AtomicU32::new(0));
    let observed = Arc::clone(&expired_starts);
    let handler: Handler = Arc::new(move |_, _| {
        observed.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Err(DbError::Stale) })
    });
    let runner = Runner::new(db, config(), handler);
    runner.start();
    tokio::time::timeout(WAIT, async {
        while !runner.heartbeats_fresh(WAIT) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    runner.shutdown(WAIT).await;
    assert_eq!(
        expired_starts.load(Ordering::SeqCst),
        0,
        "an already-expired claim called its handler"
    );
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn waiting_kind_keeps_its_place(_: PgPoolOptions, options: PgConnectOptions) -> Result<()> {
    let db = Db::connect_with(options, 5).await?;
    insert(&db).await?;
    let mut observer = db.conn().await?;
    sqlx::raw_sql(
        "INSERT INTO guilds (guild_id,name,owner_id) VALUES (1,'Test',1);
        INSERT INTO jobs (kind,key,guild_id,max_attempts) VALUES ('send_dm','send_dm:due',1,3)",
    )
    .execute(&mut *observer)
    .await?;
    let (sender, mut starts) = tokio::sync::mpsc::unbounded_channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let release_first = Arc::clone(&release);
    let handler: Handler = Arc::new(move |job, _| {
        sender.send(job.kind).unwrap();
        let release_first = Arc::clone(&release_first);
        Box::pin(async move {
            if job.kind == JobKind::Housekeeping {
                release_first.notified().await;
            }
            Ok(Handled::Settle(JobResult::Done))
        })
    });
    // Run both kinds, with Housekeeping established as the permit holder by
    // gating SendDm's first sweep. The lock is released before SendDm joins
    // the waiting list.
    let mut gate = db.begin().await?;
    sqlx::query(
        "INSERT INTO jobs (kind,key,guild_id,max_attempts,claim_token,locked_until)
        VALUES ('send_dm','send_dm:expired',1,3,gen_random_uuid(),now()-interval '1 second')",
    )
    .execute(&mut *observer)
    .await?;
    let lock_expired = "SELECT id FROM jobs WHERE key='send_dm:expired' FOR UPDATE";
    sqlx::query(lock_expired).execute(&mut *gate).await?;
    let both = Runner::new(
        db.clone(),
        RunnerConfig {
            kinds: vec![JobKind::Housekeeping, JobKind::SendDm],
            ..config()
        },
        handler,
    );
    both.start();
    assert_eq!(
        tokio::time::timeout(WAIT, starts.recv()).await?,
        Some(JobKind::Housekeeping)
    );
    gate.rollback().await?;
    tokio::time::timeout(WAIT, async {
        while !both.heartbeats_fresh(WAIT) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;
    // Hold SendDm immediately before its next permit attempt. Keeping the
    // waiter's place should now prevent the completing kind from jumping it.
    sqlx::query(
        "UPDATE jobs SET claim_token=gen_random_uuid(),locked_until=now()-interval '1 second'
        WHERE key='send_dm:expired'",
    )
    .execute(&mut *observer)
    .await?;
    let mut gate = db.begin().await?;
    let locker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *gate)
        .await?;
    sqlx::query(lock_expired).execute(&mut *gate).await?;
    both.wake(JobKind::SendDm);
    tokio::time::timeout(WAIT, async {
        loop {
            if sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity
                WHERE datname=current_database() AND $1 = ANY(pg_blocking_pids(pid)))",
            )
            .bind(locker)
            .fetch_one(&mut *observer)
            .await?
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        Ok::<_, sqlx::Error>(())
    })
    .await??;
    release.notify_one();
    // Wait for the first Housekeeping row to settle, then enqueue a
    // second one and wake its dispatcher while SendDm is still waiting.
    tokio::time::timeout(WAIT, async {
        loop {
            if !sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM jobs WHERE kind='housekeeping')",
            )
            .fetch_one(&mut *observer)
            .await?
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        Ok::<_, sqlx::Error>(())
    })
    .await??;
    insert(&db).await?;
    both.wake(JobKind::Housekeeping);
    let overtook = matches!(
        tokio::time::timeout(Duration::from_millis(250), starts.recv()).await,
        Ok(Some(JobKind::Housekeeping))
    );
    gate.rollback().await?;
    release.notify_one();
    both.shutdown(WAIT).await;
    assert!(
        !overtook,
        "same kind reclaimed the freed permit while another kind was next in line"
    );
    Ok(())
}

async fn commits(conn: &mut sqlx::PgConnection) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT xact_commit FROM pg_stat_database WHERE datname = current_database()",
    )
    .fetch_one(conn)
    .await?)
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn an_idle_runner_issues_a_handful_of_queries_per_second(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = Db::connect_with(options, 8).await?;
    let handler: Handler = Arc::new(|_, _| panic!("no jobs"));
    let runner = Runner::new(db.clone(), RunnerConfig::new(16), handler);
    runner.start();
    // Past the first passes, which also prepare each statement on each new
    // pool connection.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let mut conn = db.conn().await?;
    let before = commits(&mut conn).await?;
    tokio::time::sleep(Duration::from_secs(4)).await;
    let after = commits(&mut conn).await?;
    runner.shutdown(WAIT).await;
    // Ten kinds, one query per pass, one pass per 2-second poll: about
    // five a second (the statistics view lags a little either way). The
    // spinning runner this guards against made thousands.
    let per_second = (after - before) / 4;
    assert!(
        per_second <= 10,
        "an idle runner committed {} transactions in 4 seconds",
        after - before
    );
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn a_panicking_handler_is_retried_and_frees_its_permits(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = Db::connect_with(options, 4).await?;
    let mut conn = db.conn().await?;
    sqlx::query(
        "INSERT INTO jobs (kind, key, max_attempts) VALUES ('housekeeping', 'housekeeping', 5)",
    )
    .execute(&mut *conn)
    .await?;
    let calls = Arc::new(AtomicU32::new(0));
    let observed = Arc::clone(&calls);
    let handler: Handler = Arc::new(move |_, phase| {
        let call = observed.fetch_add(1, Ordering::SeqCst);
        assert!(call > 0, "the handler panics while building its future");
        Box::pin(async move {
            assert!(call != 1, "the handler's future panics");
            assert_eq!(phase, jobs::Phase::Run);
            Ok(Handled::Settle(JobResult::Done))
        })
    });
    let runner = Runner::new(db.clone(), config(), handler);
    runner.start();
    let mut seen = 0;
    let done = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let row: Option<(i32, Option<String>)> =
                sqlx::query_as("SELECT attempts, last_error FROM jobs")
                    .fetch_optional(&mut *conn)
                    .await?;
            match row {
                None => return Ok::<_, sqlx::Error>(()),
                Some((attempts, Some(error))) if attempts > seen => {
                    assert_eq!(error, "panic");
                    seen = attempts;
                    // Skip the backoff.
                    sqlx::query("UPDATE jobs SET run_at = now()")
                        .execute(&mut *conn)
                        .await?;
                    runner.wake(JobKind::Housekeeping);
                }
                Some(_) => {}
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    runner.shutdown(WAIT).await;
    done??;
    assert_eq!(calls.load(Ordering::SeqCst), 3, "two panics, then done");
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn a_panicking_finalize_ends_dead(_: PgPoolOptions, options: PgConnectOptions) -> Result<()> {
    let db = Db::connect_with(options, 4).await?;
    insert(&db).await?;
    let mut conn = db.conn().await?;
    sqlx::query("UPDATE jobs SET attempts = max_attempts")
        .execute(&mut *conn)
        .await?;
    let calls = Arc::new(AtomicU32::new(0));
    let observed = Arc::clone(&calls);
    let handler: Handler = Arc::new(move |_, phase| {
        assert_eq!(phase, jobs::Phase::Finalize);
        observed.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { panic!("finalize always panics") })
    });
    let runner = Runner::new(db.clone(), config(), handler);
    runner.start();
    let dead = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let row: (bool, Option<String>) =
                sqlx::query_as("SELECT dead_at IS NOT NULL, last_error FROM jobs")
                    .fetch_one(&mut *conn)
                    .await?;
            if row.0 {
                return Ok::<_, sqlx::Error>(row.1);
            }
            sqlx::query("UPDATE jobs SET run_at = now() WHERE claim_token IS NULL")
                .execute(&mut *conn)
                .await?;
            runner.wake(JobKind::Housekeeping);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    runner.shutdown(WAIT).await;
    assert_eq!(dead??.as_deref(), Some("finalize_panic"));
    assert_eq!(calls.load(Ordering::SeqCst), jobs::FINALIZE_TRIES);
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn a_self_settled_retry_on_the_last_attempt_finalizes_at_once(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = Db::connect_with(options, 4).await?;
    insert(&db).await?;
    let phases = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&phases);
    let pool = db.clone();
    let handler: Handler = Arc::new(move |job, phase| {
        observed.lock().unwrap().push(phase);
        let pool = pool.clone();
        Box::pin(async move {
            let result = match phase {
                jobs::Phase::Run => JobResult::Retry {
                    at: jobs::RunAt::In(Duration::from_secs(3600)),
                    error: "discord_unavailable".to_owned(),
                },
                jobs::Phase::Finalize => JobResult::Done,
            };
            let mut tx = pool.begin().await?;
            jobs::settle(&mut tx, &job, &result).await?;
            tx.commit().await?;
            Ok(Handled::Settled(result))
        })
    });
    let runner = Runner::new(db.clone(), config(), handler);
    runner.start();
    let mut conn = db.conn().await?;
    let gone = tokio::time::timeout(WAIT, async {
        while sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM jobs)")
            .fetch_one(&mut *conn)
            .await?
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok::<_, sqlx::Error>(())
    })
    .await;
    runner.shutdown(WAIT).await;
    gone??;
    assert_eq!(
        *phases.lock().unwrap(),
        vec![jobs::Phase::Run, jobs::Phase::Finalize],
        "finalize ran without waiting out the hour-long backoff"
    );
    Ok(())
}
