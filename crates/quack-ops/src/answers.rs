//! `/ops` answers and the ops buttons (§21.8). The bot checks
//! [`Ops::is_operator`] before calling any of these.

use quack_core::discord::Outcome;
use quack_core::ids::{AccountId, GuildId, JobId};
use quack_db::DbError;
use quack_text::ops::{self as text, Handled, JOBS_PER_PAGE};
use quack_text::{Msg, Text};

use crate::Ops;
use crate::body::DeadJob;

/// An answer: the message, or a refusal to show instead.
pub type Answer = Result<Msg, Text>;

impl Ops {
    /// The status board, with **Refresh** (which re-renders wherever it is
    /// clicked: the board or a private copy).
    pub async fn status(&self) -> Result<Msg, DbError> {
        crate::status::message(self).await
    }

    /// `/ops server`: metadata and counts of any stored server (§21.1).
    pub async fn server(&self, raw_id: &str) -> Result<Answer, DbError> {
        let Some(guild) = raw_id.trim().parse::<u64>().ok().and_then(GuildId::new) else {
            return Ok(Err(text::server_unknown(raw_id)));
        };
        let (stored, counts) = {
            let mut conn = self.engine().db().conn().await?;
            let Some(stored) = quack_db::guilds::get(&mut conn, guild).await? else {
                return Ok(Err(text::server_unknown(raw_id)));
            };
            (
                stored,
                quack_db::ops::server_counts(&mut conn, guild).await?,
            )
        };
        let missing_permissions = if stored.installed {
            match self.engine().discord().guild_facts(guild).await {
                Outcome::Done(facts) => Some(facts.quack.map_or_else(
                    || vec!["Quack isn't in the server".to_owned()],
                    |q| {
                        q.permissions
                            .missing(quack_engine::settings::wanted())
                            .into_iter()
                            .map(|p| quack_text::access::permission(p).to_owned())
                            .collect()
                    },
                )),
                _ => None,
            }
        } else {
            Some(Vec::new())
        };
        let modules_on = [
            (counts.tickets_on, "tickets"),
            (counts.activity_log_on, "server activity log"),
            (counts.honeypot_on, "honeypot"),
        ]
        .into_iter()
        .filter_map(|(on, name)| on.then_some(name))
        .collect();
        Ok(Ok(text::server(&text::Server {
            guild,
            name: stored.name,
            owner: stored.owner_id,
            installed: stored.installed,
            first_joined_at: stored.first_joined_at,
            left_at: stored.left_at,
            cases: counts.cases,
            voided_cases: counts.voided_cases,
            active_rules: counts.active_rules,
            open_appeals: counts.open_appeals,
            failed_actions: counts.failed_actions,
            open_tickets: counts.open_tickets,
            modules_on,
            missing_permissions,
        })))
    }

    /// `/ops jobs`: one page (1-based) of dead jobs.
    pub async fn dead_jobs(&self, page: u32) -> Result<Msg, DbError> {
        let page = page.max(1);
        let mut conn = self.engine().db().conn().await?;
        let (jobs, total) = quack_db::ops::dead_jobs(
            &mut conn,
            (page - 1).saturating_mul(JOBS_PER_PAGE),
            JOBS_PER_PAGE,
        )
        .await?;
        let views: Vec<text::DeadJob> = jobs.iter().map(|j| DeadJob::from(j).view()).collect();
        Ok(text::jobs(&views, page, total))
    }

    /// **Requeue**: §20.3's sanctioned edit on a dead job.
    pub async fn requeue(&self, job: JobId, by: AccountId) -> Result<Answer, DbError> {
        self.handle(job, by, Handled::Requeued).await
    }

    /// **Dismiss**: deletes a dead job now.
    pub async fn dismiss(&self, job: JobId, by: AccountId) -> Result<Answer, DbError> {
        self.handle(job, by, Handled::Dismissed).await
    }

    async fn handle(&self, job: JobId, by: AccountId, how: Handled) -> Result<Answer, DbError> {
        let mut conn = self.engine().db().conn().await?;
        let Some(dead) = quack_db::ops::dead_job(&mut conn, job).await? else {
            return Ok(Err(text::job_gone()));
        };
        let done = match how {
            Handled::Requeued => quack_db::ops::requeue_dead(&mut conn, job).await?,
            Handled::Dismissed => quack_db::ops::dismiss_dead(&mut conn, job).await?,
        };
        if !done {
            return Ok(Err(text::job_gone()));
        }
        tracing::info!(job_id = %job.get(), operator = by.get(), ?how, "an operator handled a dead job");
        Ok(Ok(text::dead_job_handled(
            &DeadJob::from(&dead).view(),
            how,
            by,
        )))
    }
}
