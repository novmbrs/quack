//! The singleton fence (§17.5): one session-level advisory lock that only
//! one running Quack process can hold, so two instances never act at once.

use std::time::Duration;

use sqlx::postgres::{PgConnectOptions, PgConnection};
use sqlx::{Connection as _, Executor as _};

use crate::DbError;
use crate::obs::{self, Select};

/// The advisory lock key (`0x517561636B`, "Quack" in ASCII).
pub const FENCE_KEY: i64 = 0x0051_7561_636B;

/// How often [`Fence::lost`] checks the connection.
pub const FENCE_CHECK_INTERVAL: Duration = Duration::from_secs(5);

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

    /// Whether the fence connection still answers. A `false` means the lock
    /// may already be someone else's: stop all work and exit.
    pub async fn check(&mut self) -> bool {
        obs::run(Select, "pg_advisory_lock", self.conn.execute("SELECT 1"))
            .await
            .is_ok()
    }

    /// Resolves when the fence connection stops answering, checking every
    /// [`FENCE_CHECK_INTERVAL`].
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
