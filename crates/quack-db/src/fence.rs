//! The singleton fence (§17.5): one session-level advisory lock that only
//! one running Quack process can hold, so two instances never act at once.

use std::time::Duration;

use sqlx::Connection as _;
use sqlx::postgres::{PgConnectOptions, PgConnection};

use crate::DbError;
use crate::obs::{self, Select};

/// The advisory lock key (`0x517561636B`, "Quack" in ASCII).
pub const FENCE_KEY: i64 = 0x0051_7561_636B;

/// How often [`Fence::lost`] checks the connection.
pub const FENCE_CHECK_INTERVAL: Duration = Duration::from_secs(5);

/// How long one check may take before the fence counts as lost.
pub const FENCE_CHECK_TIMEOUT: Duration = Duration::from_secs(3);

/// A held singleton lock on its own connection, outside the pool. The lock
/// lasts as long as this connection: dropping the fence (or losing the
/// connection) releases it.
#[derive(Debug)]
pub struct Fence {
    conn: PgConnection,
}

impl Fence {
    /// Opens a dedicated connection and tries the lock once. `Ok(None)`
    /// means another process holds it; callers retry every few seconds.
    pub async fn try_acquire(options: &PgConnectOptions) -> Result<Option<Self>, DbError> {
        let mut conn =
            PgConnection::connect_with(&options.clone().application_name("quack-fence")).await?;
        let acquired = obs::run(
            Select,
            "pg_advisory_lock",
            sqlx::query_scalar!(
                "SELECT pg_try_advisory_lock($1) AS \"acquired!\"",
                FENCE_KEY
            )
            .fetch_one(&mut conn),
        )
        .await?;
        if acquired {
            Ok(Some(Self { conn }))
        } else {
            let _ = conn.close().await;
            Ok(None)
        }
    }

    /// Whether this connection still answers, within
    /// [`FENCE_CHECK_TIMEOUT`], and still holds the lock. A `false` means
    /// the lock may already be someone else's: stop starting Discord calls
    /// and exit. A half-open connection counts as lost, since PostgreSQL
    /// may already have ended the session and granted the lock elsewhere.
    pub async fn check(&mut self) -> bool {
        let held = tokio::time::timeout(
            FENCE_CHECK_TIMEOUT,
            obs::run(
                Select,
                "pg_locks",
                sqlx::query_scalar!(
                    r#"SELECT EXISTS (
                         SELECT 1 FROM pg_locks
                         WHERE locktype = 'advisory' AND pid = pg_backend_pid() AND granted
                           AND objsubid = 1
                           AND ((classid::bigint << 32) | objid::bigint) = $1
                       ) AS "held!""#,
                    FENCE_KEY
                )
                .fetch_one(&mut self.conn),
            ),
        )
        .await;
        matches!(held, Ok(Ok(true)))
    }

    /// Resolves when a [`Fence::check`] fails, checking every
    /// [`FENCE_CHECK_INTERVAL`]. Loss is noticed up to one interval plus
    /// one timeout late, and Discord calls already in flight can outlive
    /// it; the caller then stops admitting new ones and drops the fence,
    /// which closes its connection.
    pub async fn lost(&mut self) {
        let mut interval = tokio::time::interval(FENCE_CHECK_INTERVAL);
        loop {
            interval.tick().await;
            if !self.check().await {
                return;
            }
        }
    }

    /// Releases the lock and closes the connection (at shutdown).
    pub async fn release(self) -> Result<(), DbError> {
        Ok(self.conn.close().await?)
    }
}
