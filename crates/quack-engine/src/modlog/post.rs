//! The `post_mod_log` handler (§9.4): one entry posted to the log channel
//! it was pinned to when it was written.

use std::time::Duration;

use quack_core::ids::{ChannelId, GuildId, ModLogId};
use quack_core::modlog::{
    ChannelSettingClearedDetail, LogChannelFailedDetail, ModLogEntry, ModLogLinks,
};
use quack_core::modules::{ChannelSetting, LogChannelFailure};
use quack_core::permissions::Permission;
use quack_db::jobs::{ClaimedJob, Handled, JobResult, settle as settle_job};
use quack_db::{DbError, Tx, modlog as db, notify, settings};
use quack_telemetry::labels::Outcome as Label;
use quack_telemetry::metrics;
use time::OffsetDateTime;

use super::{By, read::facts, write};
use crate::channels::{ChannelCheck, staff_channel};
use crate::jobs::{dead, retry};
use crate::port::{Nonce, Outcome, RefusalKind};
use crate::{Engine, obs};

/// How long one `log_channel_failed` entry stands for an outage.
///
/// The design (§9.4) bounds an outage by the server's last successful
/// post (`guild_settings.log_channel_posted_at`, migration 0002), which
/// `quack-db` does not have yet; until it does, an outage is taken to be
/// the post job's own retry horizon.
const OUTAGE: Duration = Duration::from_hours(1);

#[derive(Clone, Copy, Debug, serde::Deserialize)]
struct Payload {
    entry_id: ModLogId,
    channel_id: ChannelId,
}

/// Posts one moderation log entry (§9.4). Safe to repeat inside the nonce
/// window: the nonce is the entry's id. A job whose pinned channel is no
/// longer the log channel finishes without posting.
pub async fn post(engine: &Engine, job: &ClaimedJob) -> Result<Handled, DbError> {
    let (Some(guild), Ok(payload)) = (job.guild, job.payload::<Payload>()) else {
        return Ok(dead("bad_payload"));
    };
    obs::use_case("post_mod_log", Some(guild), async {
        let (current, entry) = {
            let mut conn = engine.db().conn().await?;
            let current = settings::get(&mut conn, guild)
                .await?
                .and_then(|s| s.log_channel);
            (current, db::get(&mut conn, guild, payload.entry_id).await?)
        };
        let Some(entry) = entry else {
            return Ok(dead("entry_missing"));
        };
        if current != Some(payload.channel_id) {
            metrics().modlog.posted(Label::Skipped);
            return Ok(Handled::Settle(JobResult::Done));
        }
        let needs = Permission::ViewChannel | Permission::SendMessages;
        match staff_channel(engine, guild, payload.channel_id, needs).await {
            ChannelCheck::Ok => {}
            ChannelCheck::Problems(_) => {
                return without_post(
                    engine,
                    job,
                    guild,
                    payload,
                    LogChannelFailure::NotStaffOnly,
                    None,
                )
                .await;
            }
            ChannelCheck::Missing => return channel_gone(engine, job, guild, payload).await,
            ChannelCheck::Unavailable => {
                return Ok(Handled::Settle(retry(job, "facts_unavailable")));
            }
        }
        let msg = {
            let mut conn = engine.db().conn().await?;
            let f = facts(engine, &mut conn, &entry, true).await?;
            quack_text::log::post(&entry.entry, &f)
        };
        let nonce = Nonce::new(entry.id.to_u22());
        match engine
            .discord()
            .send(payload.channel_id, &msg, &[], &nonce)
            .await
        {
            Outcome::Done(_) => {
                metrics().modlog.posted(Label::Done);
                Ok(Handled::Settle(JobResult::Done))
            }
            Outcome::Refused(r) if r.kind == RefusalKind::UnknownChannel => {
                channel_gone(engine, job, guild, payload).await
            }
            Outcome::Refused(r) => {
                without_post(
                    engine,
                    job,
                    guild,
                    payload,
                    LogChannelFailure::DiscordRefused,
                    r.code,
                )
                .await
            }
            other => Ok(Handled::Settle(retry(job, other.code()))),
        }
    })
    .await
}

/// 404 Unknown Channel: finish without posting, clear the log channel if
/// it still holds this channel (compare-and-clear), and say so.
async fn channel_gone(
    engine: &Engine,
    job: &ClaimedJob,
    guild: GuildId,
    payload: Payload,
) -> Result<Handled, DbError> {
    let mut tx = engine.db().begin().await?;
    if settings::clear_channel(
        &mut tx,
        guild,
        ChannelSetting::LogChannel,
        payload.channel_id,
    )
    .await?
    {
        write(
            &mut tx,
            guild,
            ModLogEntry::ChannelSettingCleared(ChannelSettingClearedDetail {
                setting: ChannelSetting::LogChannel,
            }),
            By::QUACK,
            ModLogLinks::default(),
        )
        .await?;
        notify::settings_changed(&mut tx, guild).await?;
    }
    failed_once(
        &mut tx,
        guild,
        payload.entry_id,
        LogChannelFailure::DiscordRefused,
        Some(10_003),
    )
    .await?;
    settle_job(&mut tx, job, &JobResult::Done).await?;
    tx.commit().await?;
    metrics().modlog.posted(Label::Skipped);
    Ok(Handled::Settled(JobResult::Done))
}

/// A finish without a post: the setting is kept, and `log_channel_failed`
/// is written once per outage.
async fn without_post(
    engine: &Engine,
    job: &ClaimedJob,
    guild: GuildId,
    payload: Payload,
    reason: LogChannelFailure,
    code: Option<u32>,
) -> Result<Handled, DbError> {
    let mut tx = engine.db().begin().await?;
    failed_once(&mut tx, guild, payload.entry_id, reason, code).await?;
    settle_job(&mut tx, job, &JobResult::Done).await?;
    tx.commit().await?;
    metrics().modlog.posted(Label::Failed);
    Ok(Handled::Settled(JobResult::Done))
}

async fn failed_once(
    tx: &mut Tx,
    guild: GuildId,
    entry: ModLogId,
    reason: LogChannelFailure,
    code: Option<u32>,
) -> Result<(), DbError> {
    let last = db::last_log_channel_failure(tx, guild).await?;
    let recent = last.is_some_and(|at| obs::since(at, OffsetDateTime::now_utc()) < OUTAGE);
    if recent {
        return Ok(());
    }
    write(
        tx,
        guild,
        ModLogEntry::LogChannelFailed(LogChannelFailedDetail {
            entry_id: entry,
            reason,
            discord_code: code,
        }),
        By::QUACK,
        ModLogLinks::default(),
    )
    .await?;
    Ok(())
}

/// Settles a `post_mod_log` job whose budget is spent: no post, and
/// `log_channel_failed` once per outage.
pub async fn finalize_post(engine: &Engine, job: &ClaimedJob) -> Result<Handled, DbError> {
    let (Some(guild), Ok(payload)) = (job.guild, job.payload::<Payload>()) else {
        return Ok(dead("bad_payload"));
    };
    without_post(
        engine,
        job,
        guild,
        payload,
        LogChannelFailure::RetriesExhausted,
        None,
    )
    .await
}
