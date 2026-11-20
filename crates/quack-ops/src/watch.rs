//! The periodic work (§21.2–21.7): a one-minute heartbeat of its own;
//! channel setup until it succeeds, then a one-minute tick (alerts, error
//! feed) and, every tenth tick, the status board and the gateway summary.

use std::time::Duration;

use quack_db::DbError;
use quack_db::ops::{self as db, Feed};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::body::{Body, DeadJob, FailedGroup};
use quack_text::ops::FAILED_GROUPS_SHOWN;

use crate::{GatewaySnapshot, Ops, channels, errors, post, status};

/// How often channel setup is tried again.
const SETUP_RETRY: Duration = Duration::from_mins(5);
/// The tick.
const TICK: Duration = Duration::from_mins(1);
/// Ticks between status board updates and gateway summaries (10 minutes).
const BOARD_EVERY: u32 = 10;
/// Dead jobs posted one by one per tick; the rest are counted.
const DEAD_JOBS_POSTED: u32 = 5;

/// Starts the heartbeat (on its own, so it beats while channel setup
/// keeps failing) and the task that does the rest.
pub(crate) fn spawn(ops: &Ops, stop: &CancellationToken) -> Vec<JoinHandle<()>> {
    let (beat_ops, beat_stop) = (ops.clone(), stop.clone());
    let (ops, stop) = (ops.clone(), stop.clone());
    vec![
        tokio::spawn(async move {
            tokio::select! {
                () = beat_stop.cancelled() => {}
                () = heartbeat(beat_ops) => {}
            }
        }),
        tokio::spawn(async move {
            tokio::select! {
                () = stop.cancelled() => {}
                () = run(ops) => {}
            }
        }),
    ]
}

/// Records every minute that this run is alive (§21.3).
async fn heartbeat(ops: Ops) {
    let mut interval = tokio::time::interval(TICK);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    interval.tick().await;
    loop {
        interval.tick().await;
        let beat = async {
            let mut conn = ops.engine().db().conn().await?;
            db::heartbeat(&mut conn).await
        };
        if let Err(error) = beat.await {
            tracing::warn!(%error, "couldn't record the ops heartbeat");
        }
    }
}

async fn run(ops: Ops) {
    setup(&ops).await;
    if let Err(error) = status::update(&ops).await {
        tracing::warn!(%error, "couldn't update the ops status board");
    }
    let mut gateway = ops.gateway().unwrap_or_default();
    let mut ticks: u32 = 0;
    let mut interval = tokio::time::interval(TICK);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    interval.tick().await;
    loop {
        interval.tick().await;
        ticks = ticks.wrapping_add(1);
        tick(&ops).await;
        if ticks.is_multiple_of(BOARD_EVERY) {
            if let Err(error) = status::update(&ops).await {
                tracing::warn!(%error, "couldn't update the ops status board");
            }
            gateway = summarize(&ops, gateway).await;
        }
    }
}

/// Finds or creates the channels, trying again every 5 minutes.
async fn setup(ops: &Ops) {
    let mut warned = false;
    loop {
        match channels::ensure(ops).await {
            Ok(found) => {
                tracing::info!("the ops channels are ready");
                ops.set_channels(found);
                return;
            }
            Err(why) if !warned => {
                warned = true;
                tracing::warn!(%why, "the ops channels aren't ready; trying again every 5 minutes");
            }
            Err(why) => tracing::debug!(%why, "the ops channels still aren't ready"),
        }
        tokio::time::sleep(SETUP_RETRY).await;
    }
}

/// One minute: alerts and the error feed.
async fn tick(ops: &Ops) {
    if let Err(error) = alerts(ops).await {
        tracing::warn!(%error, "couldn't read the ops alerts");
    }
    errors::flush(ops).await;
}

/// Saves a post for each new dead job and one for new failed actions, and
/// marks them reported, in one transaction (§21.5).
pub(crate) async fn alerts(ops: &Ops) -> Result<(), DbError> {
    let mut tx = ops.engine().db().begin().await?;
    let (dead, more) = db::take_new_dead_jobs(&mut tx, DEAD_JOBS_POSTED).await?;
    for job in &dead {
        post::insert(
            &mut tx,
            ops,
            Feed::Alerts,
            &Body::DeadJob(DeadJob::from(job)),
        )
        .await?;
    }
    if more > 0 {
        post::insert(&mut tx, ops, Feed::Alerts, &Body::DeadJobsMore { more }).await?;
    }
    let failed = db::take_new_failed_actions(&mut tx).await?;
    if !failed.is_empty() {
        // Every failure is now marked reported, so the post keeps the
        // whole count even though it names only the largest groups.
        let total = failed.iter().map(|g| g.count).sum();
        let hidden_groups =
            u64::try_from(failed.len().saturating_sub(FAILED_GROUPS_SHOWN)).unwrap_or(0);
        let groups = failed
            .iter()
            .take(FAILED_GROUPS_SHOWN)
            .map(|g| FailedGroup {
                guild_id: g.guild,
                action: g.kind,
                reason: g.reason,
                count: g.count,
            })
            .collect();
        let body = Body::FailedActions {
            groups,
            total,
            hidden_groups,
        };
        post::insert(&mut tx, ops, Feed::Alerts, &body).await?;
    }
    tx.commit().await
}

/// Posts what the gateway did since `before`, if anything; returns now.
async fn summarize(ops: &Ops, before: GatewaySnapshot) -> GatewaySnapshot {
    let Some(now) = ops.gateway() else {
        return before;
    };
    let diff = |a: u64, b: u64| a.saturating_sub(b);
    let body = Body::Gateway {
        drops: diff(now.drops, before.drops),
        connect_failures: diff(now.connect_failures, before.connect_failures),
        identifies: diff(now.identifies, before.identifies),
        resumes: diff(now.resumes, before.resumes),
        invalid_sessions: diff(now.invalid_sessions, before.invalid_sessions),
        connected_now: now.connected,
    };
    let Body::Gateway {
        drops,
        connect_failures,
        identifies,
        resumes,
        invalid_sessions,
        ..
    } = body
    else {
        return now;
    };
    if drops + connect_failures + identifies + resumes + invalid_sessions > 0
        && let Err(error) = post::enqueue(ops, Feed::Deploys, &body).await
    {
        tracing::warn!(%error, "couldn't save the gateway summary");
    }
    now
}
