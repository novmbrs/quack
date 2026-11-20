//! Ops in Discord (§21): durable ops posts, the one `ops_state` row, the
//! alert reads, dead-job controls, and per-server counts for `/ops server`.

use std::str::FromStr;

use quack_core::UnknownValue;
use quack_core::action::{ActionFailure, ActionKind};
use quack_core::ids::{ChannelId, GuildId, JobId, MessageId};
use sqlx::PgConnection;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::jobs::{JobKind, NewJob};
use crate::obs::{self, Delete, Insert, Select, Update};
use crate::pg::{Pg, count, id, opt_id};
use crate::{DbError, Tx};

/// The ops channel a durable post goes to (§21.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Feed {
    /// `#ops-deploys`.
    Deploys,
    /// `#ops-installs`.
    Installs,
    /// `#ops-alerts`.
    Alerts,
}

impl Feed {
    /// The stored name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Deploys => "deploys",
            Self::Installs => "installs",
            Self::Alerts => "alerts",
        }
    }
}

impl FromStr for Feed {
    type Err = UnknownValue;

    fn from_str(s: &str) -> Result<Self, UnknownValue> {
        Ok(match s {
            "deploys" => Self::Deploys,
            "installs" => Self::Installs,
            "alerts" => Self::Alerts,
            other => {
                return Err(UnknownValue {
                    type_name: "Feed",
                    value: other.to_owned(),
                });
            }
        })
    }
}

/// A durable ops post waiting for its job.
#[derive(Clone, Debug, PartialEq)]
pub struct OpsPost {
    /// The post (also its nonce).
    pub id: Uuid,
    /// Where it goes.
    pub feed: Feed,
    /// What it says, as the ops crate's typed body.
    pub body: serde_json::Value,
}

/// Saves a post and its `post_ops` job in `tx`; the job runs in the ops
/// server `guild`.
pub async fn insert_post(
    tx: &mut Tx,
    guild: GuildId,
    feed: Feed,
    body: &serde_json::Value,
) -> Result<Uuid, DbError> {
    let post = obs::run(
        Insert,
        "ops_posts",
        sqlx::query_scalar!(
            "INSERT INTO ops_posts (feed, body) VALUES ($1, $2) RETURNING id",
            feed.as_str(),
            body,
        )
        .fetch_one(&mut **tx),
    )
    .await?;
    crate::jobs::enqueue(tx, &NewJob::post_ops(guild, post)).await?;
    Ok(post)
}

/// One waiting post.
pub async fn post(conn: &mut PgConnection, post: Uuid) -> Result<Option<OpsPost>, DbError> {
    let row = obs::run(
        Select,
        "ops_posts",
        sqlx::query!("SELECT id, feed, body FROM ops_posts WHERE id = $1", post)
            .fetch_optional(conn),
    )
    .await?;
    row.map(|r| {
        Ok(OpsPost {
            id: r.id,
            feed: r
                .feed
                .parse()
                .map_err(|e: UnknownValue| DbError::Decode(e.to_string()))?,
            body: r.body,
        })
    })
    .transpose()
}

/// Deletes a post once it is posted or dropped.
pub async fn delete_post(conn: &mut PgConnection, post: Uuid) -> Result<(), DbError> {
    obs::run(
        Delete,
        "ops_posts",
        sqlx::query!("DELETE FROM ops_posts WHERE id = $1", post).execute(conn),
    )
    .await?;
    Ok(())
}

/// How a run stopped cleanly (§21.3). A run that lost the fence records
/// nothing: another instance may already own `ops_state`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// SIGTERM or SIGINT.
    Signal,
    /// The gateway closed for good.
    GatewayClosed,
}

impl StopReason {
    /// The stored name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Signal => "signal",
            Self::GatewayClosed => "gateway_closed",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "signal" => Some(Self::Signal),
            "gateway_closed" => Some(Self::GatewayClosed),
            _ => None,
        }
    }
}

/// What `ops_state` said about the run before this one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreviousRun {
    /// Its version.
    pub version: Option<String>,
    /// When it started (`None`: there was none).
    pub started_at: Option<OffsetDateTime>,
    /// Its last heartbeat.
    pub seen_at: Option<OffsetDateTime>,
    /// When it stopped cleanly, and why.
    pub stopped: Option<(OffsetDateTime, StopReason)>,
}

/// Records a new run (version, started now) and returns the one before.
pub async fn start_run(conn: &mut PgConnection, version: &str) -> Result<PreviousRun, DbError> {
    let row = obs::run(
        Update,
        "ops_state",
        sqlx::query!(
            r#"UPDATE ops_state SET version = $1, started_at = now(), seen_at = now(),
                 stopped_at = NULL, stop_reason = NULL
               RETURNING old.version AS "version?", old.started_at AS "started_at?",
                         old.seen_at AS "seen_at?", old.stopped_at AS "stopped_at?",
                         old.stop_reason AS "stop_reason?""#,
            version,
        )
        .fetch_one(conn),
    )
    .await?;
    Ok(PreviousRun {
        version: row.version,
        started_at: row.started_at,
        seen_at: row.seen_at,
        stopped: row
            .stopped_at
            .zip(row.stop_reason.as_deref().and_then(StopReason::parse)),
    })
}

/// The run's heartbeat: it is still alive now.
pub async fn heartbeat(conn: &mut PgConnection) -> Result<(), DbError> {
    obs::run(
        Update,
        "ops_state",
        sqlx::query!("UPDATE ops_state SET seen_at = now()").execute(conn),
    )
    .await?;
    Ok(())
}

/// Records a clean stop.
pub async fn record_stop(conn: &mut PgConnection, reason: StopReason) -> Result<(), DbError> {
    obs::run(
        Update,
        "ops_state",
        sqlx::query!(
            "UPDATE ops_state SET seen_at = now(), stopped_at = now(), stop_reason = $1",
            reason.as_str()
        )
        .execute(conn),
    )
    .await?;
    Ok(())
}

/// The status board's message, when one was posted.
pub async fn status_message(
    conn: &mut PgConnection,
) -> Result<Option<(ChannelId, MessageId)>, DbError> {
    let row = obs::run(
        Select,
        "ops_state",
        sqlx::query!("SELECT status_channel_id, status_message_id FROM ops_state").fetch_one(conn),
    )
    .await?;
    Ok(match (row.status_channel_id, row.status_message_id) {
        (Some(c), Some(m)) => Some((id(c)?, id(m)?)),
        _ => None,
    })
}

/// Remembers the status board's message.
pub async fn set_status_message(
    conn: &mut PgConnection,
    channel: ChannelId,
    message: MessageId,
) -> Result<(), DbError> {
    obs::run(
        Update,
        "ops_state",
        sqlx::query!(
            "UPDATE ops_state SET status_channel_id = $1, status_message_id = $2",
            i64::from(channel),
            i64::from(message)
        )
        .execute(conn),
    )
    .await?;
    Ok(())
}

/// A dead job (§16.1), as ops shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeadJob {
    /// The job.
    pub id: JobId,
    /// Its kind.
    pub kind: JobKind,
    /// Its key.
    pub key: String,
    /// Its server (`None` for housekeeping).
    pub guild: Option<GuildId>,
    /// Runs it used.
    pub attempts: u32,
    /// Its last error code.
    pub last_error: Option<String>,
    /// When it died.
    pub dead_at: OffsetDateTime,
}

struct DeadRow {
    id: Uuid,
    kind: Pg<JobKind>,
    key: String,
    guild_id: Option<i64>,
    attempts: i32,
    last_error: Option<String>,
    dead_at: Option<OffsetDateTime>,
}

impl TryFrom<DeadRow> for DeadJob {
    type Error = DbError;

    fn try_from(r: DeadRow) -> Result<Self, DbError> {
        Ok(Self {
            id: JobId::from_uuid(r.id),
            kind: r.kind.0,
            key: r.key,
            guild: opt_id(r.guild_id)?,
            attempts: u32::try_from(r.attempts).unwrap_or(0),
            last_error: r.last_error,
            dead_at: r
                .dead_at
                .ok_or_else(|| DbError::Decode("a dead job without dead_at".to_owned()))?,
        })
    }
}

/// Jobs that died and haven't been alerted for that death yet (except
/// `post_ops`), oldest first, at most `limit`, and how many more there
/// were. Each is marked alerted (`ops_alerted_at = dead_at`) in `tx`, so it
/// is reported once per death. A death still uncommitted is simply not
/// seen yet: the next poll gets it (§21.5).
pub async fn take_new_dead_jobs(tx: &mut Tx, limit: u32) -> Result<(Vec<DeadJob>, u64), DbError> {
    let mut rows = obs::run(
        Update,
        "jobs",
        sqlx::query_as!(
            DeadRow,
            r#"UPDATE jobs SET ops_alerted_at = dead_at
               WHERE dead_at IS NOT NULL AND kind <> 'post_ops'
                 AND (ops_alerted_at IS NULL OR ops_alerted_at < dead_at)
               RETURNING id, kind, key, guild_id, attempts, last_error, dead_at"#
        )
        .fetch_all(&mut **tx),
    )
    .await?;
    rows.sort_by_key(|r| (r.dead_at, r.id));
    let total = rows.len();
    let jobs = rows
        .into_iter()
        .take(usize::try_from(limit).unwrap_or(usize::MAX))
        .map(DeadJob::try_from)
        .collect::<Result<Vec<_>, _>>()?;
    let more = u64::try_from(total - jobs.len()).unwrap_or(u64::MAX);
    Ok((jobs, more))
}

/// Failed actions, grouped: how many of one kind and reason in one server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FailedGroup {
    /// The server.
    pub guild: GuildId,
    /// The action kind.
    pub kind: ActionKind,
    /// Why they failed.
    pub reason: ActionFailure,
    /// How many.
    pub count: u64,
}

/// Undismissed failed actions not yet alerted for their current failure,
/// grouped by server, kind, and reason (largest first). Each is marked
/// alerted (`ops_alerted_at = updated_at`) in `tx`; a later retry that
/// fails again moves `updated_at` and alerts again.
pub async fn take_new_failed_actions(tx: &mut Tx) -> Result<Vec<FailedGroup>, DbError> {
    let rows = obs::run(
        Update,
        "actions",
        sqlx::query!(
            r#"WITH alerted AS (
                 UPDATE actions SET ops_alerted_at = updated_at
                 WHERE state = 'failed' AND dismissed_at IS NULL
                   AND (ops_alerted_at IS NULL OR ops_alerted_at < updated_at)
                 RETURNING guild_id, kind, failure_reason)
               SELECT guild_id AS "guild_id!", kind AS "kind!", failure_reason AS "reason!", count(*) AS "n!"
               FROM alerted
               GROUP BY guild_id, kind, failure_reason
               ORDER BY count(*) DESC, guild_id"#
        )
        .fetch_all(&mut **tx),
    )
    .await?;
    rows.into_iter()
        .map(|r| {
            Ok(FailedGroup {
                guild: id(r.guild_id)?,
                kind: r.kind.0,
                reason: r.reason.0,
                count: count(r.n)?,
            })
        })
        .collect()
}

/// One dead job by id (`None` when it is gone or no longer dead).
pub async fn dead_job(conn: &mut PgConnection, job: JobId) -> Result<Option<DeadJob>, DbError> {
    let row = obs::run(
        Select,
        "jobs",
        sqlx::query_as!(
            DeadRow,
            r#"SELECT id, kind, key, guild_id, attempts, last_error, dead_at
               FROM jobs WHERE id = $1 AND dead_at IS NOT NULL"#,
            job.get()
        )
        .fetch_optional(conn),
    )
    .await?;
    row.map(DeadJob::try_from).transpose()
}

/// Dead jobs, newest first: `limit` from `offset`, and how many there are.
pub async fn dead_jobs(
    conn: &mut PgConnection,
    offset: u32,
    limit: u32,
) -> Result<(Vec<DeadJob>, u64), DbError> {
    let total = obs::run(
        Select,
        "jobs",
        sqlx::query_scalar!(r#"SELECT count(*) AS "n!" FROM jobs WHERE dead_at IS NOT NULL"#)
            .fetch_one(&mut *conn),
    )
    .await?;
    let rows = obs::run(
        Select,
        "jobs",
        sqlx::query_as!(
            DeadRow,
            r#"SELECT id, kind, key, guild_id, attempts, last_error, dead_at
               FROM jobs WHERE dead_at IS NOT NULL
               ORDER BY dead_at DESC, id DESC OFFSET $1 LIMIT $2"#,
            i64::from(offset),
            i64::from(limit)
        )
        .fetch_all(conn),
    )
    .await?;
    let jobs = rows
        .into_iter()
        .map(DeadJob::try_from)
        .collect::<Result<Vec<_>, _>>()?;
    Ok((jobs, count(total)?))
}

/// Requeues a dead job: §20.3's sanctioned edit, then wakes its kind.
/// Returns whether it was dead.
pub async fn requeue_dead(conn: &mut PgConnection, job: JobId) -> Result<bool, DbError> {
    let kind = obs::run(
        Update,
        "jobs",
        sqlx::query_scalar!(
            r#"UPDATE jobs SET dead_at = NULL, attempts = 0, run_at = now(), updated_at = now()
               WHERE id = $1 AND dead_at IS NOT NULL
               RETURNING kind"#,
            job.get()
        )
        .fetch_optional(&mut *conn),
    )
    .await?;
    match kind {
        Some(kind) => {
            crate::jobs::notify(conn, kind.0).await?;
            Ok(true)
        }
        None => Ok(false),
    }
}

/// Deletes a dead job now, as housekeeping would after 7 days. Returns
/// whether it was dead.
pub async fn dismiss_dead(conn: &mut PgConnection, job: JobId) -> Result<bool, DbError> {
    let done = obs::run(
        Delete,
        "jobs",
        sqlx::query!(
            "DELETE FROM jobs WHERE id = $1 AND dead_at IS NOT NULL",
            job.get()
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// What `/ops server` counts (§21.1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ServerCounts {
    /// Cases, voided included.
    pub cases: u64,
    /// Voided cases.
    pub voided_cases: u64,
    /// Active rules.
    pub active_rules: u64,
    /// Appeals waiting for staff.
    pub open_appeals: u64,
    /// Failed actions not dismissed.
    pub failed_actions: u64,
    /// Open tickets.
    pub open_tickets: u64,
    /// Tickets is on.
    pub tickets_on: bool,
    /// The server activity log is on.
    pub activity_log_on: bool,
    /// The honeypot is on.
    pub honeypot_on: bool,
}

/// One server's counts and module switches.
pub async fn server_counts(
    conn: &mut PgConnection,
    guild: GuildId,
) -> Result<ServerCounts, DbError> {
    let r = obs::run(
        Select,
        "guilds",
        sqlx::query!(
            r#"SELECT
                 (SELECT count(*) FROM cases WHERE guild_id = $1) AS "cases!",
                 (SELECT count(*) FROM cases WHERE guild_id = $1 AND voided_at IS NOT NULL) AS "voided!",
                 (SELECT count(*) FROM rules WHERE guild_id = $1 AND archived_at IS NULL) AS "rules!",
                 (SELECT count(*) FROM appeals WHERE guild_id = $1
                    AND state IN ('pending', 'needs_information')) AS "appeals!",
                 (SELECT count(*) FROM actions WHERE guild_id = $1 AND state = 'failed'
                    AND dismissed_at IS NULL) AS "failed!",
                 (SELECT count(*) FROM tickets WHERE guild_id = $1 AND state = 'open') AS "tickets!",
                 coalesce((SELECT enabled FROM ticket_settings WHERE guild_id = $1), false) AS "tickets_on!",
                 coalesce((SELECT enabled FROM activity_log_settings WHERE guild_id = $1), false) AS "activity_on!",
                 coalesce((SELECT enabled FROM honeypot_settings WHERE guild_id = $1), false) AS "honeypot_on!""#,
            i64::from(guild)
        )
        .fetch_one(conn),
    )
    .await?;
    Ok(ServerCounts {
        cases: count(r.cases)?,
        voided_cases: count(r.voided)?,
        active_rules: count(r.rules)?,
        open_appeals: count(r.appeals)?,
        failed_actions: count(r.failed)?,
        open_tickets: count(r.tickets)?,
        tickets_on: r.tickets_on,
        activity_log_on: r.activity_on,
        honeypot_on: r.honeypot_on,
    })
}

/// How many servers Quack is in.
pub async fn installed_count(conn: &mut PgConnection) -> Result<u64, DbError> {
    let n = obs::run(
        Select,
        "guilds",
        sqlx::query_scalar!(r#"SELECT count(*) AS "n!" FROM guilds WHERE installed"#)
            .fetch_one(conn),
    )
    .await?;
    count(n)
}
