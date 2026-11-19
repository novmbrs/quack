//! The database-backed gauges (§16.6, §18.2): failed actions waiting for
//! staff, open appeals, active sessions, and the job backlog, sampled every
//! 60 seconds by a task the binary runs. The metric callbacks read the last
//! sample, so an export never queries the database.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use quack_db::{Db, DbError, actions, appeals, jobs, sessions};
use quack_telemetry::{JobQueueSample, metrics};

use crate::obs;

/// How often the task samples.
pub const EVERY: Duration = Duration::from_secs(60);

#[derive(Debug, Default)]
struct Sample {
    failed: Vec<(quack_telemetry::labels::ActionKind, u64)>,
    open_appeals: u64,
    sessions: u64,
    backlog: Vec<JobQueueSample>,
}

/// The last sample and the callbacks that report it.
#[derive(Clone, Debug, Default)]
pub struct Gauges {
    last: Arc<Mutex<Sample>>,
}

impl Gauges {
    /// Installs the callbacks of `quack.actions.failed`,
    /// `quack.appeals.open`, `quack.sessions.active`, and the job backlog
    /// gauges.
    pub fn install() -> Self {
        let gauges = Self::default();
        let last = Arc::clone(&gauges.last);
        metrics()
            .actions
            .observe_failed(move || last.lock().map_or_default(|s| s.failed.clone()));
        let last = Arc::clone(&gauges.last);
        metrics()
            .appeals
            .observe_open(move || last.lock().map_or_default(|s| s.open_appeals));
        let last = Arc::clone(&gauges.last);
        metrics()
            .http
            .observe_sessions(move || last.lock().map_or_default(|s| s.sessions));
        let last = Arc::clone(&gauges.last);
        metrics()
            .jobs
            .observe_backlog(move || last.lock().map_or_default(|s| s.backlog.clone()));
        gauges
    }

    /// Takes one sample (kinds without rows report zero).
    pub async fn sample(&self, db: &Db) -> Result<(), DbError> {
        let mut conn = db.conn().await?;
        let failed = actions::failed_by_kind(&mut conn)
            .await?
            .into_iter()
            .map(|(kind, n)| (obs::action_kind(kind), n))
            .collect();
        let open_appeals = appeals::count_open(&mut conn).await?;
        let sessions = sessions::count_active(&mut conn).await?;
        let backlog = jobs::backlog(&mut conn)
            .await?
            .iter()
            .map(jobs::JobBacklog::sample)
            .collect();
        if let Ok(mut last) = self.last.lock() {
            *last = Sample {
                failed,
                open_appeals,
                sessions,
                backlog,
            };
        }
        Ok(())
    }

    /// Samples every [`EVERY`] until the task is dropped.
    pub async fn run(self, db: Db) {
        loop {
            if let Err(error) = self.sample(&db).await {
                tracing::warn!(error = %error, "sampling the gauges failed");
            }
            tokio::time::sleep(EVERY).await;
        }
    }
}
