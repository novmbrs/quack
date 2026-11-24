//! Durable ops posts (§21.3): an `ops_posts` row and its `post_ops` job,
//! inserted together; the job sends the post and deletes the row.

use std::sync::Arc;
use std::time::Duration;

use quack_core::ids::GuildId;
use quack_db::jobs::{ClaimedJob, Handled, HandlerFuture, JobKind, JobResult, Phase, RunAt};
use quack_db::ops::{self as db, Feed};
use quack_db::{DbError, Tx};
use quack_engine::jobs::ModuleJobs;
use quack_engine::jobs::module::{begin, dead, finish, retry};
use quack_engine::port::{Nonce, Outcome};
use uuid::Uuid;

use crate::Ops;
use crate::body::{Body, Change, Previous};

/// Saves `body` for `feed` and its job in `tx`.
pub(crate) async fn insert(tx: &mut Tx, ops: &Ops, feed: Feed, body: &Body) -> Result<(), DbError> {
    let json = serde_json::to_value(body)
        .map_err(|error| DbError::Decode(format!("ops post body: {error}")))?;
    db::insert_post(tx, ops.guild(), feed, &json).await?;
    Ok(())
}

/// Saves one post in its own transaction.
pub(crate) async fn enqueue(ops: &Ops, feed: Feed, body: &Body) -> Result<(), DbError> {
    let mut tx = ops.engine().db().begin().await?;
    insert(&mut tx, ops, feed, body).await?;
    tx.commit().await
}

/// Records the run and posts `ops.started`.
pub(crate) async fn started(
    ops: &Ops,
    migrations_applied: usize,
    ready_in: Duration,
) -> Result<(), DbError> {
    let (previous, servers) = {
        let mut conn = ops.engine().db().conn().await?;
        let previous = db::start_run(&mut conn, &ops.config().version).await?;
        (previous, db::installed_count(&mut conn).await?)
    };
    let previous = match (previous.started_at, previous.stopped, previous.seen_at) {
        (None, _, _) => Previous::None,
        (Some(_), Some((at, reason)), _) => Previous::Stopped {
            at,
            gateway_closed: reason == db::StopReason::GatewayClosed,
        },
        (Some(started), None, seen) => Previous::Ended {
            seen_at: seen.unwrap_or(started),
        },
    };
    let body = Body::Started {
        version: ops.config().version.clone(),
        commit: ops.config().commit.clone(),
        started_at: ops.inner.started_at,
        ready_in_seconds: ready_in.as_secs(),
        migrations_applied,
        servers,
        previous,
    };
    enqueue(ops, Feed::Deploys, &body).await
}

/// Posts a join (`Some(rejoined)`) or a leave (`None`) of `guild`: its
/// members, and on a leave how long Quack stayed and what it leaves
/// behind (§21.4).
pub(crate) async fn install(ops: &Ops, guild: GuildId, rejoined: Option<bool>) {
    let result = async {
        let mut conn = ops.engine().db().conn().await?;
        let Some(stored) = quack_db::guilds::get(&mut conn, guild).await? else {
            return Ok(());
        };
        let servers = db::installed_count(&mut conn).await?;
        let day = quack_db::fleet::day(&mut conn).await?;
        let left = rejoined.is_none();
        let (since, counts) = if left {
            let since = quack_db::fleet::last_joined_at(&mut conn, guild)
                .await?
                .unwrap_or(stored.first_joined_at);
            (
                Some(since),
                Some(db::server_counts(&mut conn, guild).await?),
            )
        } else {
            (None, None)
        };
        drop(conn);
        let body = Body::Install {
            change: match rejoined {
                Some(false) => Change::Joined,
                Some(true) => Change::Rejoined,
                None => Change::Left,
            },
            name: stored.name,
            guild_id: guild,
            owner_id: stored.owner_id,
            servers,
            members: stored.member_count,
            total_members: Some(day.members),
            since,
            left_at: if left { stored.left_at } else { None },
            cases: counts.map(|c| c.cases),
            rules: counts.map(|c| c.active_rules),
        };
        enqueue(ops, Feed::Installs, &body).await
    }
    .await;
    if let Err(error) = result {
        tracing::warn!(guild_id = guild.get(), %error, "couldn't save an ops install post");
    }
}

/// The `post_ops` payload.
#[derive(serde::Deserialize)]
struct Payload {
    post_id: Uuid,
}

/// `post_ops` for the runner.
pub(crate) fn jobs(ops: Ops) -> ModuleJobs {
    ModuleJobs {
        kinds: &[JobKind::PostOps],
        handler: Arc::new(move |job: ClaimedJob, phase: Phase| -> HandlerFuture {
            let ops = ops.clone();
            Box::pin(async move {
                match (job.kind, phase) {
                    (JobKind::PostOps, Phase::Run) => run(&ops, &job).await,
                    (JobKind::PostOps, Phase::Finalize) => {
                        drop_post(&ops, &job, "retries_exhausted").await
                    }
                    _ => Ok(dead("no_handler")),
                }
            })
        }),
    }
}

/// How long a rate-limited post waits at least.
const RATE_LIMIT_FLOOR: Duration = Duration::from_secs(1);

/// Sends one post. Its nonce is the post id, so a repeat inside Discord's
/// window returns the first message (§10.4).
async fn run(ops: &Ops, job: &ClaimedJob) -> Result<Handled, DbError> {
    let Ok(Payload { post_id }) = job.payload::<Payload>() else {
        return Ok(dead("bad_payload"));
    };
    let post = {
        let mut conn = ops.engine().db().conn().await?;
        db::post(&mut conn, post_id).await?
    };
    let Some(post) = post else {
        return Ok(Handled::Settle(JobResult::Done));
    };
    let Some(channels) = ops.channels() else {
        return Ok(retry(job, "ops_channels_not_ready"));
    };
    let Some(channel) = channels.feed(post.feed) else {
        return drop_post(ops, job, "ops_channel_not_private").await;
    };
    let Ok(body) = serde_json::from_value::<Body>(post.body) else {
        return drop_post(ops, job, "bad_body").await;
    };
    let msg = body.render();
    let nonce = Nonce::new(quack_core::ids::JobId::from_uuid(post_id).to_u22());
    match ops
        .engine()
        .discord()
        .send(channel, &msg, &[], &nonce)
        .await
    {
        Outcome::Done(_) => {
            let mut tx = begin(ops.engine(), job).await?;
            db::delete_post(&mut tx, post_id).await?;
            finish(tx, job, JobResult::Done).await
        }
        Outcome::RetryAfter(wait) => Ok(Handled::Settle(JobResult::Defer {
            at: RunAt::In(wait.max(RATE_LIMIT_FLOOR)),
        })),
        refused @ Outcome::Refused(_) => drop_post(ops, job, &refused.code()).await,
        other => Ok(retry(job, other.code())),
    }
}

/// Gives a post up: deletes its row and settles the job done.
async fn drop_post(ops: &Ops, job: &ClaimedJob, why: &str) -> Result<Handled, DbError> {
    let post = job.payload::<Payload>().ok().map(|p| p.post_id);
    tracing::warn!(job_id = %job.id.get(), why, "dropped an ops post");
    let mut tx = begin(ops.engine(), job).await?;
    if let Some(post) = post {
        db::delete_post(&mut tx, post).await?;
    }
    finish(tx, job, JobResult::Done).await
}
