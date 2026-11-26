//! `/ops metrics` (§21.8): fleet-wide counts read on demand. Only
//! metadata and counts cross servers here (§21.1).

use quack_db::DbError;
use quack_db::fleet;
use quack_telemetry::labels::Attribute;
use quack_text::Msg;
use quack_text::ops::{self as text, SERVERS_PER_PAGE, ServerSort};
use time::OffsetDateTime;

use crate::Ops;
use crate::body::job_kind_name;

/// Days the overview's case sparkline covers.
const SPARK_DAYS: u16 = 14;

fn window(w: fleet::Windows) -> text::Window {
    text::Window {
        day: w.day,
        week: w.week,
        month: w.month,
        all: w.all,
    }
}

const fn db_sort(sort: ServerSort) -> fleet::ServerSort {
    match sort {
        ServerSort::Members => fleet::ServerSort::Members,
        ServerSort::Cases => fleet::ServerSort::Cases,
        ServerSort::Newest => fleet::ServerSort::Newest,
    }
}

impl Ops {
    /// `/ops metrics overview`: servers, members, activity, and modules.
    pub async fn metrics_overview(&self) -> Result<crate::Rendered, DbError> {
        let mut conn = self.engine().db().conn().await?;
        let f = fleet::fleet(&mut conn).await?;
        let a = fleet::activity(&mut conn).await?;
        let daily_cases = fleet::daily_cases(&mut conn, SPARK_DAYS).await?;
        drop(conn);
        let overview = text::MetricsOverview {
            fleet: text::Fleet {
                servers: f.servers,
                counted: f.counted,
                members: f.members,
                median_members: f.median_members,
                sizes: f.sizes,
                tickets_on: f.tickets_on,
                activity_log_on: f.activity_log_on,
                honeypot_on: f.honeypot_on,
            },
            cases: window(a.cases),
            voided: window(a.voided),
            appeals: window(a.appeals),
            tickets: window(a.tickets),
            actions_done: window(a.actions_done),
            actions_failed: window(a.actions_failed),
            joins: window(a.joins),
            leaves: window(a.leaves),
            daily_cases,
            updated_at: OffsetDateTime::now_utc(),
        };
        Ok(crate::card::attach(
            text::metrics_overview(&overview),
            text::metrics_overview_card(&overview),
            text::OVERVIEW_IMAGE,
        )
        .await)
    }

    /// `/ops metrics servers` and its buttons: one page (1-based) of the
    /// installed servers in `sort` order.
    pub async fn metrics_servers(&self, sort: ServerSort, page: u32) -> Result<Msg, DbError> {
        let page = page.max(1);
        let mut conn = self.engine().db().conn().await?;
        let total = quack_db::ops::installed_count(&mut conn).await?;
        let rows = fleet::ranked_servers(
            &mut conn,
            db_sort(sort),
            (page - 1).saturating_mul(SERVERS_PER_PAGE),
            SERVERS_PER_PAGE,
        )
        .await?;
        drop(conn);
        let servers: Vec<text::RankedServer> = rows
            .into_iter()
            .map(|r| text::RankedServer {
                guild: r.guild,
                name: r.name,
                members: r.members,
                cases_month: r.cases_month,
                cases: r.cases,
                first_joined_at: r.first_joined_at,
            })
            .collect();
        Ok(text::metrics_servers(&servers, sort, page, total))
    }

    /// `/ops metrics jobs`: this process's job runs, the queue now, and
    /// failed actions by reason.
    pub async fn metrics_jobs(&self) -> Result<Msg, DbError> {
        let mut conn = self.engine().db().conn().await?;
        let queue = fleet::queue(&mut conn).await?;
        let failed = fleet::failed_reasons(&mut conn).await?;
        drop(conn);
        let runs = quack_telemetry::metrics().jobs.tally().read();
        Ok(text::metrics_jobs(&text::MetricsJobs {
            runs: runs
                .kinds
                .iter()
                .map(|k| text::JobRuns {
                    kind: job_kind_name(k.kind.as_str()),
                    runs: k.runs,
                    retries: k.retries,
                    dead: k.dead,
                    mean: k.mean(),
                })
                .collect(),
            runs_cover: runs.covers,
            queue: queue
                .iter()
                .map(|q| text::KindQueue {
                    kind: q.kind.as_str(),
                    ready: q.ready,
                    scheduled: q.scheduled,
                    running: q.running,
                    dead: q.dead,
                    oldest_ready: q.oldest_ready,
                })
                .collect(),
            failed: failed
                .iter()
                .map(|f| text::FailedReason {
                    kind: f.kind,
                    reason: f.reason,
                    count: f.count,
                    servers: f.servers,
                })
                .collect(),
            updated_at: OffsetDateTime::now_utc(),
        }))
    }
}
