//! Fleet-wide counts for the ops server (§21.6, §21.8): servers and
//! members, activity over the last day, week, and month, the busiest
//! servers, and the queue by kind. Only metadata and counts (§21.1).

use std::str::FromStr;
use std::time::Duration;

use quack_core::UnknownValue;
use quack_core::action::{ActionFailure, ActionKind};
use quack_core::ids::GuildId;
use sqlx::PgConnection;
use time::OffsetDateTime;

use crate::DbError;
use crate::jobs::JobKind;
use crate::obs::{self, Select};
use crate::pg::{count, id};

/// One count over the last day, week, and month, and all time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Windows {
    /// The last 24 hours.
    pub day: u64,
    /// The last 7 days.
    pub week: u64,
    /// The last 30 days.
    pub month: u64,
    /// All time.
    pub all: u64,
}

impl Windows {
    fn of(day: i64, week: i64, month: i64, all: i64) -> Result<Self, DbError> {
        Ok(Self {
            day: count(day)?,
            week: count(week)?,
            month: count(month)?,
            all: count(all)?,
        })
    }
}

/// The installed servers, their members, and module use.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Fleet {
    /// Servers Quack is in.
    pub servers: u64,
    /// Of those, servers whose member count is known.
    pub counted: u64,
    /// Members across the counted servers (a member of two servers counts
    /// twice).
    pub members: u64,
    /// The median counted server's members.
    pub median_members: Option<u64>,
    /// Counted servers by size: under 100, under 1,000, under 10,000,
    /// under 100,000, and larger.
    pub sizes: [u64; 5],
    /// Servers with tickets on.
    pub tickets_on: u64,
    /// Servers with the server activity log on.
    pub activity_log_on: u64,
    /// Servers with the honeypot on.
    pub honeypot_on: u64,
}

/// Servers, members, sizes, and modules across the installed servers.
pub async fn fleet(conn: &mut PgConnection) -> Result<Fleet, DbError> {
    let r = obs::run(
        Select,
        "guilds",
        sqlx::query!(
            r#"SELECT count(*) AS "servers!",
                      count(g.member_count) AS "counted!",
                      coalesce(sum(g.member_count), 0)::bigint AS "members!",
                      percentile_disc(0.5) WITHIN GROUP (ORDER BY g.member_count) AS "median",
                      count(*) FILTER (WHERE g.member_count < 100) AS "s0!",
                      count(*) FILTER (WHERE g.member_count >= 100 AND g.member_count < 1000) AS "s1!",
                      count(*) FILTER (WHERE g.member_count >= 1000 AND g.member_count < 10000) AS "s2!",
                      count(*) FILTER (WHERE g.member_count >= 10000 AND g.member_count < 100000) AS "s3!",
                      count(*) FILTER (WHERE g.member_count >= 100000) AS "s4!",
                      count(*) FILTER (WHERE t.enabled) AS "tickets!",
                      count(*) FILTER (WHERE a.enabled) AS "activity!",
                      count(*) FILTER (WHERE h.enabled) AS "honeypot!"
               FROM guilds g
               LEFT JOIN ticket_settings t USING (guild_id)
               LEFT JOIN activity_log_settings a USING (guild_id)
               LEFT JOIN honeypot_settings h USING (guild_id)
               WHERE g.installed"#
        )
        .fetch_one(conn),
    )
    .await?;
    Ok(Fleet {
        servers: count(r.servers)?,
        counted: count(r.counted)?,
        members: count(r.members)?,
        median_members: r.median.map(count).transpose()?,
        sizes: [
            count(r.s0)?,
            count(r.s1)?,
            count(r.s2)?,
            count(r.s3)?,
            count(r.s4)?,
        ],
        tickets_on: count(r.tickets)?,
        activity_log_on: count(r.activity)?,
        honeypot_on: count(r.honeypot)?,
    })
}

/// What happened across every server, by window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Activity {
    /// Cases created.
    pub cases: Windows,
    /// Cases voided.
    pub voided: Windows,
    /// Appeals sent.
    pub appeals: Windows,
    /// Tickets opened.
    pub tickets: Windows,
    /// Servers that added Quack (first joins and returns).
    pub joins: Windows,
    /// Servers that removed Quack.
    pub leaves: Windows,
    /// Actions done.
    pub actions_done: Windows,
    /// Actions that failed and still are (`all`: every one not dismissed).
    pub actions_failed: Windows,
}

/// [`Activity`] over the last day, week, month, and all time.
pub async fn activity(conn: &mut PgConnection) -> Result<Activity, DbError> {
    let r = obs::run(
        Select,
        "cases",
        sqlx::query!(
            r#"WITH
               c AS (SELECT count(*) FILTER (WHERE created_at > now() - interval '1 day') AS c1,
                            count(*) FILTER (WHERE created_at > now() - interval '7 days') AS c7,
                            count(*) FILTER (WHERE created_at > now() - interval '30 days') AS c30,
                            count(*) AS call,
                            count(*) FILTER (WHERE voided_at > now() - interval '1 day') AS v1,
                            count(*) FILTER (WHERE voided_at > now() - interval '7 days') AS v7,
                            count(*) FILTER (WHERE voided_at > now() - interval '30 days') AS v30,
                            count(voided_at) AS vall
                     FROM cases),
               ap AS (SELECT count(*) FILTER (WHERE created_at > now() - interval '1 day') AS a1,
                             count(*) FILTER (WHERE created_at > now() - interval '7 days') AS a7,
                             count(*) FILTER (WHERE created_at > now() - interval '30 days') AS a30,
                             count(*) AS aall
                      FROM appeals),
               t AS (SELECT count(*) FILTER (WHERE created_at > now() - interval '1 day') AS t1,
                            count(*) FILTER (WHERE created_at > now() - interval '7 days') AS t7,
                            count(*) FILTER (WHERE created_at > now() - interval '30 days') AS t30,
                            count(*) AS tall
                     FROM tickets),
               i AS (SELECT count(*) FILTER (WHERE kind = 'quack_joined' AND created_at > now() - interval '1 day') AS j1,
                            count(*) FILTER (WHERE kind = 'quack_joined' AND created_at > now() - interval '7 days') AS j7,
                            count(*) FILTER (WHERE kind = 'quack_joined' AND created_at > now() - interval '30 days') AS j30,
                            count(*) FILTER (WHERE kind = 'quack_joined') AS jall,
                            count(*) FILTER (WHERE kind = 'quack_left' AND created_at > now() - interval '1 day') AS l1,
                            count(*) FILTER (WHERE kind = 'quack_left' AND created_at > now() - interval '7 days') AS l7,
                            count(*) FILTER (WHERE kind = 'quack_left' AND created_at > now() - interval '30 days') AS l30,
                            count(*) FILTER (WHERE kind = 'quack_left') AS lall
                     FROM mod_log WHERE kind IN ('quack_joined', 'quack_left')),
               x AS (SELECT count(*) FILTER (WHERE state = 'done' AND done_at > now() - interval '1 day') AS d1,
                            count(*) FILTER (WHERE state = 'done' AND done_at > now() - interval '7 days') AS d7,
                            count(*) FILTER (WHERE state = 'done' AND done_at > now() - interval '30 days') AS d30,
                            count(*) FILTER (WHERE state = 'done') AS dall,
                            count(*) FILTER (WHERE state = 'failed' AND dismissed_at IS NULL AND updated_at > now() - interval '1 day') AS f1,
                            count(*) FILTER (WHERE state = 'failed' AND dismissed_at IS NULL AND updated_at > now() - interval '7 days') AS f7,
                            count(*) FILTER (WHERE state = 'failed' AND dismissed_at IS NULL AND updated_at > now() - interval '30 days') AS f30,
                            count(*) FILTER (WHERE state = 'failed' AND dismissed_at IS NULL) AS fall
                     FROM actions)
               SELECT c1 AS "c1!", c7 AS "c7!", c30 AS "c30!", call AS "call!",
                      v1 AS "v1!", v7 AS "v7!", v30 AS "v30!", vall AS "vall!",
                      a1 AS "a1!", a7 AS "a7!", a30 AS "a30!", aall AS "aall!",
                      t1 AS "t1!", t7 AS "t7!", t30 AS "t30!", tall AS "tall!",
                      j1 AS "j1!", j7 AS "j7!", j30 AS "j30!", jall AS "jall!",
                      l1 AS "l1!", l7 AS "l7!", l30 AS "l30!", lall AS "lall!",
                      d1 AS "d1!", d7 AS "d7!", d30 AS "d30!", dall AS "dall!",
                      f1 AS "f1!", f7 AS "f7!", f30 AS "f30!", fall AS "fall!"
               FROM c, ap, t, i, x"#
        )
        .fetch_one(conn),
    )
    .await?;
    Ok(Activity {
        cases: Windows::of(r.c1, r.c7, r.c30, r.call)?,
        voided: Windows::of(r.v1, r.v7, r.v30, r.vall)?,
        appeals: Windows::of(r.a1, r.a7, r.a30, r.aall)?,
        tickets: Windows::of(r.t1, r.t7, r.t30, r.tall)?,
        joins: Windows::of(r.j1, r.j7, r.j30, r.jall)?,
        leaves: Windows::of(r.l1, r.l7, r.l30, r.lall)?,
        actions_done: Windows::of(r.d1, r.d7, r.d30, r.dall)?,
        actions_failed: Windows::of(r.f1, r.f7, r.f30, r.fall)?,
    })
}

/// The last 24 hours, for the status board: cheap enough to read every
/// 10 minutes (each count walks a time index).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Day {
    /// Members across the installed servers whose count is known.
    pub members: u64,
    /// Cases created.
    pub cases: u64,
    /// Appeals sent.
    pub appeals: u64,
    /// Tickets opened.
    pub tickets: u64,
    /// Servers that added Quack.
    pub joins: u64,
    /// Servers that removed Quack.
    pub leaves: u64,
}

/// [`Day`].
pub async fn day(conn: &mut PgConnection) -> Result<Day, DbError> {
    let r = obs::run(
        Select,
        "guilds",
        sqlx::query!(
            r#"SELECT
                 (SELECT coalesce(sum(member_count), 0)::bigint FROM guilds WHERE installed) AS "members!",
                 (SELECT count(*) FROM cases WHERE created_at > now() - interval '1 day') AS "cases!",
                 (SELECT count(*) FROM appeals WHERE created_at > now() - interval '1 day') AS "appeals!",
                 (SELECT count(*) FROM tickets WHERE created_at > now() - interval '1 day') AS "tickets!",
                 (SELECT count(*) FROM mod_log WHERE kind = 'quack_joined'
                    AND created_at > now() - interval '1 day') AS "joins!",
                 (SELECT count(*) FROM mod_log WHERE kind = 'quack_left'
                    AND created_at > now() - interval '1 day') AS "leaves!""#
        )
        .fetch_one(conn),
    )
    .await?;
    Ok(Day {
        members: count(r.members)?,
        cases: count(r.cases)?,
        appeals: count(r.appeals)?,
        tickets: count(r.tickets)?,
        joins: count(r.joins)?,
        leaves: count(r.leaves)?,
    })
}

/// Members across the installed servers whose count is known.
pub async fn total_members(conn: &mut PgConnection) -> Result<u64, DbError> {
    let n = obs::run(
        Select,
        "guilds",
        sqlx::query_scalar!(
            r#"SELECT coalesce(sum(member_count), 0)::bigint AS "n!" FROM guilds WHERE installed"#
        )
        .fetch_one(conn),
    )
    .await?;
    count(n)
}

/// Cases created on each of the last `days` UTC days, oldest first (today
/// last, so far).
pub async fn daily_cases(conn: &mut PgConnection, days: u16) -> Result<Vec<u64>, DbError> {
    let span = i32::from(days.max(1)) - 1;
    let rows = obs::run(
        Select,
        "cases",
        sqlx::query_scalar!(
            r#"SELECT count(c.id) AS "n!"
               FROM generate_series(0, $1::int) AS d(back)
               LEFT JOIN cases c
                 ON c.created_at >= (current_date - d.back)::timestamp AT TIME ZONE 'UTC'
                AND c.created_at < (current_date - d.back + 1)::timestamp AT TIME ZONE 'UTC'
               GROUP BY d.back
               ORDER BY d.back DESC"#,
            span
        )
        .fetch_all(conn),
    )
    .await?;
    rows.into_iter().map(count).collect()
}

/// How `/ops metrics servers` orders the servers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ServerSort {
    /// Most members first.
    #[default]
    Members,
    /// Most cases in the last 30 days first.
    Cases,
    /// Newest first install first.
    Newest,
}

impl ServerSort {
    /// Every order, as the buttons show them.
    pub const ALL: [Self; 3] = [Self::Members, Self::Cases, Self::Newest];

    /// The stored and custom-id name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Members => "members",
            Self::Cases => "cases",
            Self::Newest => "newest",
        }
    }
}

impl FromStr for ServerSort {
    type Err = UnknownValue;

    fn from_str(s: &str) -> Result<Self, UnknownValue> {
        Self::ALL
            .into_iter()
            .find(|v| v.as_str() == s)
            .ok_or_else(|| UnknownValue {
                type_name: "ServerSort",
                value: s.to_owned(),
            })
    }
}

/// One installed server in the ranking.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RankedServer {
    /// The server.
    pub guild: GuildId,
    /// Its name.
    pub name: String,
    /// Its members, when known.
    pub members: Option<u64>,
    /// Cases in the last 30 days.
    pub cases_month: u64,
    /// Cases, all time.
    pub cases: u64,
    /// When Quack first joined.
    pub first_joined_at: OffsetDateTime,
}

/// One page of installed servers in `sort` order (ties by id).
pub async fn ranked_servers(
    conn: &mut PgConnection,
    sort: ServerSort,
    offset: u32,
    limit: u32,
) -> Result<Vec<RankedServer>, DbError> {
    let rows = obs::run(
        Select,
        "guilds",
        // Only the 30-day window is aggregated across servers (for the
        // Cases order); lifetime counts are read for the page alone.
        sqlx::query!(
            r#"WITH month AS (
                 SELECT guild_id, count(*) AS n FROM cases
                 WHERE $1 = 'cases' AND created_at > now() - interval '30 days'
                 GROUP BY guild_id),
               page AS (
                 SELECT g.guild_id, g.name, g.member_count, g.first_joined_at,
                        coalesce(m.n, 0) AS ranked_month
                 FROM guilds g LEFT JOIN month m USING (guild_id)
                 WHERE g.installed
                 ORDER BY
                   CASE WHEN $1 = 'members' THEN g.member_count END DESC NULLS LAST,
                   CASE WHEN $1 = 'cases' THEN coalesce(m.n, 0) END DESC NULLS LAST,
                   CASE WHEN $1 = 'newest' THEN g.first_joined_at END DESC NULLS LAST,
                   g.guild_id
                 OFFSET $2 LIMIT $3)
               SELECT p.guild_id, p.name, p.member_count, p.first_joined_at,
                      (SELECT count(*) FROM cases c WHERE c.guild_id = p.guild_id
                         AND c.created_at > now() - interval '30 days') AS "month!",
                      (SELECT count(*) FROM cases c WHERE c.guild_id = p.guild_id) AS "total!"
               FROM page p
               ORDER BY
                 CASE WHEN $1 = 'members' THEN p.member_count END DESC NULLS LAST,
                 CASE WHEN $1 = 'cases' THEN p.ranked_month END DESC NULLS LAST,
                 CASE WHEN $1 = 'newest' THEN p.first_joined_at END DESC NULLS LAST,
                 p.guild_id"#,
            sort.as_str(),
            i64::from(offset),
            i64::from(limit)
        )
        .fetch_all(conn),
    )
    .await?;
    rows.into_iter()
        .map(|r| {
            Ok(RankedServer {
                guild: id(r.guild_id)?,
                name: r.name,
                members: r.member_count.map(count).transpose()?,
                cases_month: count(r.month)?,
                cases: count(r.total)?,
                first_joined_at: r.first_joined_at,
            })
        })
        .collect()
}

/// The queue of one job kind right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KindQueue {
    /// The kind.
    pub kind: JobKind,
    /// Due and waiting for a worker.
    pub ready: u64,
    /// Waiting for a later time (backoff or a schedule).
    pub scheduled: u64,
    /// Claimed by a worker now.
    pub running: u64,
    /// Not dead and already run at least once.
    pub retrying: u64,
    /// Dead, kept for inspection.
    pub dead: u64,
    /// How long the oldest ready job has waited.
    pub oldest_ready: Duration,
}

/// [`KindQueue`] for every kind with any rows, in [`JobKind::ALL`] order.
pub async fn queue(conn: &mut PgConnection) -> Result<Vec<KindQueue>, DbError> {
    let rows = obs::run(
        Select,
        "jobs",
        sqlx::query!(
            r#"SELECT kind,
                 count(*) FILTER (WHERE dead_at IS NULL AND locked_until IS NULL AND run_at <= now()) AS "ready!",
                 count(*) FILTER (WHERE dead_at IS NULL AND locked_until IS NULL AND run_at > now()) AS "scheduled!",
                 count(*) FILTER (WHERE locked_until IS NOT NULL) AS "running!",
                 count(*) FILTER (WHERE dead_at IS NULL AND attempts > 0) AS "retrying!",
                 count(*) FILTER (WHERE dead_at IS NOT NULL) AS "dead!",
                 EXTRACT(EPOCH FROM now() - min(run_at)
                   FILTER (WHERE dead_at IS NULL AND locked_until IS NULL AND run_at <= now()))::float8 AS "age"
               FROM jobs GROUP BY kind"#
        )
        .fetch_all(conn),
    )
    .await?;
    let mut out = Vec::new();
    for kind in JobKind::ALL {
        let Some(r) = rows.iter().find(|r| r.kind.0 == *kind) else {
            continue;
        };
        out.push(KindQueue {
            kind: *kind,
            ready: count(r.ready)?,
            scheduled: count(r.scheduled)?,
            running: count(r.running)?,
            retrying: count(r.retrying)?,
            dead: count(r.dead)?,
            oldest_ready: r
                .age
                .and_then(|s| Duration::try_from_secs_f64(s.max(0.0)).ok())
                .unwrap_or(Duration::ZERO),
        });
    }
    Ok(out)
}

/// Failed, undismissed actions of one kind and reason across servers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FailedReason {
    /// The action kind.
    pub kind: ActionKind,
    /// Why they failed.
    pub reason: ActionFailure,
    /// How many.
    pub count: u64,
    /// In how many servers.
    pub servers: u64,
}

/// [`FailedReason`]s, largest first.
pub async fn failed_reasons(conn: &mut PgConnection) -> Result<Vec<FailedReason>, DbError> {
    let rows = obs::run(
        Select,
        "actions",
        sqlx::query!(
            r#"SELECT kind, failure_reason AS "reason!", count(*) AS "n!",
                      count(DISTINCT guild_id) AS "servers!"
               FROM actions
               WHERE state = 'failed' AND dismissed_at IS NULL AND failure_reason IS NOT NULL
               GROUP BY kind, failure_reason
               ORDER BY count(*) DESC, kind, failure_reason"#
        )
        .fetch_all(conn),
    )
    .await?;
    rows.into_iter()
        .map(|r| {
            Ok(FailedReason {
                kind: r.kind.0,
                reason: r.reason.0,
                count: count(r.n)?,
                servers: count(r.servers)?,
            })
        })
        .collect()
}

/// When Quack last joined `guild` (its latest `quack_joined` entry), or
/// `None` for a server with none (an imported one).
pub async fn last_joined_at(
    conn: &mut PgConnection,
    guild: GuildId,
) -> Result<Option<OffsetDateTime>, DbError> {
    let at = obs::run(
        Select,
        "mod_log",
        sqlx::query_scalar!(
            r#"SELECT max(created_at) AS "at" FROM mod_log
               WHERE guild_id = $1 AND kind = 'quack_joined'"#,
            i64::from(guild)
        )
        .fetch_one(conn),
    )
    .await?;
    Ok(at)
}
