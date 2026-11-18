//! The `send_dm` handler (§6.4) and its finalize (§16.5).

use std::time::{Duration, Instant};

use quack_core::dm::{
    DmEvent, DmFailure, DmKind, DmState, UNKNOWN_REPEAT_DELAY, failure_for, in_window, wait_fits,
};
use quack_core::ids::{ChannelId, GuildId};
use quack_db::dms::{self, Dm};
use quack_db::jobs::{ClaimedJob, Handled, JobResult, RunAt, fence};
use quack_db::{DbError, cases};
use time::OffsetDateTime;

use super::render::render;
use super::{Change, Payload, settle_dm};
use crate::jobs::{dead, retry};
use crate::port::{Nonce, Outcome};
use crate::{Engine, obs};

/// Sends one DM (§6.4). Safe to repeat: the nonce is the DM's id, a
/// create is repeated only inside the window, and a DM whose result was
/// never learned is settled `unknown`, never sent again.
pub async fn send(engine: &Engine, job: &ClaimedJob) -> Result<Handled, DbError> {
    let (Some(guild), Ok(payload)) = (job.guild, job.payload::<Payload>()) else {
        return Ok(dead("bad_payload"));
    };
    obs::use_case("send_dm", Some(guild), async {
        let dm = {
            let mut conn = engine.db().conn().await?;
            dms::get(&mut conn, guild, payload.dm_id).await?
        };
        let Some(dm) = dm else {
            return Ok(dead("dm_missing"));
        };
        if dm.state.is_terminal() {
            return Ok(Handled::Settle(JobResult::Done));
        }
        let started = Instant::now();
        let (channel, first) = match (dm.state, dm.channel, dm.first_attempt_at) {
            (DmState::Sending, Some(channel), Some(first)) => (channel, first),
            _ => match start(engine, job, &dm, started).await? {
                Started::Sending(channel, first) => (channel, first),
                Started::Settled(handled) => return Ok(handled),
            },
        };
        let dm = Dm {
            state: DmState::Sending,
            channel: Some(channel),
            first_attempt_at: Some(first),
            ..dm
        };
        deliver(engine, job, &dm, guild, channel, first, started).await
    })
    .await
}

enum Started {
    Sending(ChannelId, OffsetDateTime),
    Settled(Handled),
}

/// Step 2: open the DM channel, then the start transaction (case DMs check
/// the case is still valid under its lock) records the window's start.
async fn start(
    engine: &Engine,
    job: &ClaimedJob,
    dm: &Dm,
    started: Instant,
) -> Result<Started, DbError> {
    let channel = match engine.discord().open_dm(dm.member.account()).await {
        Outcome::Done(channel) => channel,
        Outcome::Refused(r) => {
            let failure =
                failure_for(DmEvent::OpenRefused, r.code).unwrap_or(DmFailure::DiscordRefused);
            let change = Change::Failed(failure, r.code);
            let handled =
                settle_dm(engine, job, dm, change, JobResult::Done, Some(started)).await?;
            return Ok(Started::Settled(handled));
        }
        Outcome::RetryAfter(d) => {
            return Ok(Started::Settled(Handled::Settle(retry_after(job, d))));
        }
        other => return Ok(Started::Settled(Handled::Settle(retry(job, other.code())))),
    };
    let mut tx = engine.db().begin().await?;
    let voided = match dm.case {
        Some(case) if dm.kind == DmKind::Case => !cases::lock(&mut tx, dm.guild, case)
            .await?
            .is_some_and(|c| c.is_valid()),
        Some(case) => {
            cases::lock(&mut tx, dm.guild, case).await?;
            false
        }
        None => false,
    };
    if voided {
        tx.rollback().await?;
        let handled = settle_dm(engine, job, dm, Change::NotNeeded, JobResult::Done, None).await?;
        return Ok(Started::Settled(handled));
    }
    let first = dms::start_sending(&mut tx, dm.guild, dm.id, channel)
        .await?
        .ok_or(DbError::Stale)?;
    fence(&mut tx, job).await?;
    tx.commit().await?;
    Ok(Started::Sending(channel, first))
}

/// Steps 3–6: create the message with the DM's nonce while the window
/// lasts, and settle.
async fn deliver(
    engine: &Engine,
    job: &ClaimedJob,
    dm: &Dm,
    guild: GuildId,
    channel: ChannelId,
    first: OffsetDateTime,
    started: Instant,
) -> Result<Handled, DbError> {
    let done = |change| settle_dm(engine, job, dm, change, JobResult::Done, Some(started));
    if !in_window(first, OffsetDateTime::now_utc()) {
        return done(Change::Unknown).await;
    }
    let Some(rendered) = render(engine, dm).await? else {
        tracing::warn!(guild_id = %guild, dm_id = %dm.id.get(), "a DM's records are missing");
        return Ok(dead("dm_records_missing"));
    };
    let nonce = Nonce::new(dm.id.to_u22());
    let mut any_unknown = false;
    loop {
        let outcome = engine
            .discord()
            .send(channel, &rendered.msg, &rendered.files, &nonce)
            .await;
        let wait = match outcome {
            Outcome::Done(sent) => return done(Change::Sent(sent.message_id)).await,
            Outcome::Refused(_) if any_unknown => return done(Change::Unknown).await,
            Outcome::Refused(r) => {
                let failure = failure_for(
                    DmEvent::CreateRefused {
                        after_unknown: false,
                    },
                    r.code,
                )
                .unwrap_or(DmFailure::DiscordRefused);
                return done(Change::Failed(failure, r.code)).await;
            }
            Outcome::Unknown(_) => {
                any_unknown = true;
                to_std(UNKNOWN_REPEAT_DELAY)
            }
            Outcome::RetryAfter(d) => d,
            Outcome::NotSent(_) => to_std(UNKNOWN_REPEAT_DELAY),
        };
        let fits = time::Duration::try_from(wait)
            .is_ok_and(|w| wait_fits(first, OffsetDateTime::now_utc(), w));
        if fits {
            tokio::time::sleep(wait).await;
            continue;
        }
        if any_unknown {
            return done(Change::Unknown).await;
        }
        // Nothing was sent: back to pending, and the job tries again later.
        let result = retry_after(job, wait);
        return settle_dm(engine, job, dm, Change::BackToPending, result, None).await;
    }
}

fn to_std(d: time::Duration) -> Duration {
    Duration::try_from(d).unwrap_or_default()
}

/// Retry after `d`, or the kind's backoff when that is longer.
fn retry_after(job: &ClaimedJob, d: Duration) -> JobResult {
    JobResult::Retry {
        at: RunAt::In(d.max(job.kind.policy().backoff(job.attempt))),
        error: "rate_limited".to_owned(),
    }
}

/// Settles a `send_dm` job whose budget is spent, without calling Discord:
/// `pending` → `failed` (`discord_unavailable`), `sending` → `unknown`;
/// a waiting kick or ban is woken.
pub async fn finalize(engine: &Engine, job: &ClaimedJob) -> Result<Handled, DbError> {
    let (Some(guild), Ok(payload)) = (job.guild, job.payload::<Payload>()) else {
        return Ok(dead("bad_payload"));
    };
    obs::use_case("finalize_dm", Some(guild), async {
        let dm = {
            let mut conn = engine.db().conn().await?;
            dms::get(&mut conn, guild, payload.dm_id).await?
        };
        let Some(dm) = dm else {
            return Ok(dead("dm_missing"));
        };
        let change = match dm.state {
            DmState::Pending => {
                let failure = failure_for(DmEvent::BudgetSpent, None)
                    .unwrap_or(DmFailure::DiscordUnavailable);
                Change::Failed(failure, None)
            }
            DmState::Sending => Change::Unknown,
            _ => return Ok(Handled::Settle(JobResult::Done)),
        };
        settle_dm(engine, job, &dm, change, JobResult::Done, None).await
    })
    .await
}
