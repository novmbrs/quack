//! The connection pool and transactions.

use std::future::Future;
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::str::FromStr as _;
use std::time::{Duration, Instant};

use quack_telemetry::labels::TransactionOutcome;
use sqlx::pool::PoolConnection;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{PgConnection, PgPool, Postgres};

use crate::DbError;
use crate::obs::{self, AfterCommit};

/// How long a caller may wait for a free pool connection.
pub const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(5);
/// The per-connection statement timeout (§17.5).
pub const STATEMENT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a connection may sit idle inside an open transaction before
/// PostgreSQL ends it, so a stuck task can never hold locks for long.
pub const IDLE_IN_TRANSACTION_TIMEOUT: Duration = Duration::from_secs(60);
/// How many times [`Db::transaction`] runs a transaction again after a
/// serialization failure or deadlock (§4.12).
pub const CONTENTION_RETRIES: u32 = 3;

/// The pool: every query and transaction goes through it.
///
/// Cheap to clone. The job listener and the singleton fence use their own
/// connections outside it.
#[derive(Clone, Debug)]
pub struct Db {
    pool: PgPool,
    options: PgConnectOptions,
}

impl Db {
    /// Connects a pool of at most `max_connections` to `url`, with the
    /// timeouts above, and registers its gauges.
    pub async fn connect(url: &str, max_connections: u32) -> Result<Self, DbError> {
        let options = PgConnectOptions::from_str(url)?;
        Self::connect_with(options, max_connections).await
    }

    /// [`Db::connect`] with parsed options.
    pub async fn connect_with(
        options: PgConnectOptions,
        max_connections: u32,
    ) -> Result<Self, DbError> {
        let options = options.application_name("quack");
        let session = options.clone().options([
            ("statement_timeout", millis(STATEMENT_TIMEOUT)),
            (
                "idle_in_transaction_session_timeout",
                millis(IDLE_IN_TRANSACTION_TIMEOUT),
            ),
            ("timezone", "UTC".to_owned()),
            // List queries take optional filters (`$n IS NULL OR …`); a
            // custom plan per execution folds them so the filter's index is
            // used. Every query here is small, so planning is cheap.
            ("plan_cache_mode", "force_custom_plan".to_owned()),
        ]);
        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .min_connections(1)
            .acquire_timeout(ACQUIRE_TIMEOUT)
            .idle_timeout(Duration::from_mins(10))
            .max_lifetime(Duration::from_mins(30))
            .test_before_acquire(false)
            .connect_with(session)
            .await?;
        obs::observe_pool(pool.clone());
        Ok(Self { pool, options })
    }

    /// The underlying pool, for sqlx APIs that need it.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// The connection options without the pool's session settings, for
    /// dedicated connections (listener, fence, migrations).
    pub fn connect_options(&self) -> &PgConnectOptions {
        &self.options
    }

    /// A pool connection for reads or single statements, recording the wait.
    pub async fn conn(&self) -> Result<PoolConnection<Postgres>, DbError> {
        let started = Instant::now();
        let result = self.pool.acquire().await;
        obs::connection_acquired(started.elapsed(), result.as_ref().err());
        Ok(result?)
    }

    /// Starts a transaction (READ COMMITTED). Its duration and outcome are
    /// recorded when it commits, rolls back, or is dropped.
    pub async fn begin(&self) -> Result<Tx, DbError> {
        let started = Instant::now();
        let result = self.pool.begin().await;
        obs::connection_acquired(started.elapsed(), result.as_ref().err());
        Ok(Tx {
            inner: result?,
            guard: TxGuard {
                started: Instant::now(),
                finished: false,
            },
            after_commit: Vec::new(),
        })
    }

    /// Runs `work` in a transaction and commits it, running it again (up to
    /// [`CONTENTION_RETRIES`] times) after a serialization failure or
    /// deadlock. `work` must only touch the database, since it may run more
    /// than once. An `Err` from `work` rolls back and is returned.
    pub async fn transaction<T, F>(&self, mut work: F) -> Result<T, DbError>
    where
        F: for<'t> FnMut(
            &'t mut Tx,
        ) -> Pin<Box<dyn Future<Output = Result<T, DbError>> + Send + 't>>,
    {
        let mut retries = 0;
        loop {
            let mut tx = self.begin().await?;
            let result = match work(&mut tx).await {
                Ok(value) => tx.commit().await.map(|()| value),
                Err(error) => {
                    // A failed rollback leaves nothing to undo: the
                    // connection is discarded and the server aborts it.
                    let _ = tx.rollback().await;
                    Err(error)
                }
            };
            match result {
                Err(error) if retries < CONTENTION_RETRIES && error.is_contention() => {
                    obs::transaction_retry(error.class());
                    retries += 1;
                }
                other => return other,
            }
        }
    }

    /// Closes every pool connection, waiting for checked-out ones.
    pub async fn close(&self) {
        self.pool.close().await;
    }
}

fn millis(duration: Duration) -> String {
    format!("{}ms", duration.as_millis())
}

/// An open transaction. Derefs to the connection, so every query function
/// taking `&mut PgConnection` accepts `&mut tx`.
///
/// Dropping it without [`Tx::commit`] rolls back (and is recorded as a
/// rollback). Metrics that must only count committed work (jobs enqueued,
/// moderation log entries) are queued on it and recorded after commit.
pub struct Tx {
    inner: sqlx::Transaction<'static, Postgres>,
    guard: TxGuard,
    after_commit: Vec<AfterCommit>,
}

impl Tx {
    /// Commits, then records what was queued for after the commit.
    pub async fn commit(self) -> Result<(), DbError> {
        let Self {
            inner,
            mut guard,
            after_commit,
        } = self;
        let result = inner.commit().await.map_err(DbError::from);
        guard.finish(if result.is_ok() {
            TransactionOutcome::Commit
        } else {
            TransactionOutcome::Rollback
        });
        result?;
        for record in after_commit {
            record.record();
        }
        Ok(())
    }

    /// Rolls back explicitly.
    pub async fn rollback(self) -> Result<(), DbError> {
        let Self {
            inner, mut guard, ..
        } = self;
        let result = inner.rollback().await;
        guard.finish(TransactionOutcome::Rollback);
        Ok(result?)
    }

    /// Sets `statement_timeout` and `lock_timeout` for the rest of this
    /// transaction (`SET LOCAL`), e.g. 2 s each for case creation (§4.2).
    pub async fn set_timeouts(
        &mut self,
        statement: Duration,
        lock: Duration,
    ) -> Result<(), DbError> {
        let statement = i64::try_from(statement.as_millis()).unwrap_or(i64::MAX);
        let lock = i64::try_from(lock.as_millis()).unwrap_or(i64::MAX);
        obs::run(
            obs::Select,
            "set_config",
            sqlx::query!(
                r#"SELECT set_config('statement_timeout', $1::bigint::text || 'ms', true) AS "statement!",
                          set_config('lock_timeout', $2::bigint::text || 'ms', true) AS "lock!""#,
                statement,
                lock
            )
            .fetch_one(&mut *self.inner),
        )
        .await?;
        Ok(())
    }

    /// Queues a metric to record once this transaction commits.
    pub(crate) fn after_commit(&mut self, record: AfterCommit) {
        self.after_commit.push(record);
    }
}

impl Deref for Tx {
    type Target = PgConnection;

    fn deref(&self) -> &PgConnection {
        &self.inner
    }
}

impl DerefMut for Tx {
    fn deref_mut(&mut self) -> &mut PgConnection {
        &mut self.inner
    }
}

impl std::fmt::Debug for Tx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tx").finish_non_exhaustive()
    }
}

/// Records a transaction's duration exactly once, including when it is
/// dropped without being finished (an implicit rollback).
struct TxGuard {
    started: Instant,
    finished: bool,
}

impl TxGuard {
    fn finish(&mut self, outcome: TransactionOutcome) {
        if !self.finished {
            self.finished = true;
            obs::transaction(outcome, self.started.elapsed());
        }
    }
}

impl Drop for TxGuard {
    fn drop(&mut self) {
        self.finish(TransactionOutcome::Rollback);
    }
}
