//! The `quack.tickets.open` gauge, sampled from the database by a task the
//! binary runs, so a metric export never queries the database.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use quack_db::{Db, DbError, tickets};
use quack_telemetry::metrics;

/// The last count of open tickets and the callback that reports it.
#[derive(Clone, Debug, Default)]
pub struct OpenGauge {
    last: Arc<AtomicU64>,
}

impl OpenGauge {
    /// How often [`OpenGauge::run`] samples.
    pub const EVERY: Duration = Duration::from_secs(60);

    /// Installs the gauge's callback.
    pub fn install() -> Self {
        let gauge = Self::default();
        let last = Arc::clone(&gauge.last);
        metrics()
            .tickets
            .observe_open(move || last.load(Ordering::Relaxed));
        gauge
    }

    /// Counts the open tickets across servers now.
    pub async fn sample(&self, db: &Db) -> Result<u64, DbError> {
        let mut conn = db.conn().await?;
        let open = tickets::count_open(&mut conn, None).await?;
        self.last.store(open, Ordering::Relaxed);
        Ok(open)
    }

    /// Samples every [`OpenGauge::EVERY`] until the task is dropped.
    pub async fn run(self, db: Db) {
        loop {
            if let Err(error) = self.sample(&db).await {
                tracing::warn!(%error, "sampling open tickets failed");
            }
            tokio::time::sleep(Self::EVERY).await;
        }
    }
}
