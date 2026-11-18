//! When a tracked message or its channel is gone (§10.3), and cleaning up
//! the earlier locations of moved messages (§10.3 step 5).

use quack_core::messages::{
    MessageKind, MissingPlan, StaleCleanup, on_missing as plan_for, stale_cleanup,
};
use quack_db::jobs::{ClaimedJob, Handled, JobResult, fence};
use quack_db::messages::{self, Location, TrackedMessage};
use quack_db::{DbError, honeypot, settings, tickets};
use quack_telemetry::labels::Outcome as Label;
use quack_text::{ChannelRef, Msg};
use time::OffsetDateTime;

use super::render::render;
use super::{finish, kind, synced};
use crate::Engine;
use crate::jobs::retry;
use crate::port::{FileEdit, InteractionToken, Outcome};

/// Where a message of this kind belongs now: the appeal, ticket entry,
/// ticket queue, or honeypot channel setting.
async fn current_channel(
    engine: &Engine,
    row: &TrackedMessage,
) -> Result<Option<quack_core::ids::ChannelId>, DbError> {
    let mut conn = engine.db().conn().await?;
    let guild = row.guild;
    Ok(match kind(row) {
        MessageKind::CaseReceipt => None,
        MessageKind::AppealPost => settings::get(&mut conn, guild)
            .await?
            .and_then(|s| s.appeal_channel),
        MessageKind::TicketPanel => tickets::settings(&mut conn, guild)
            .await?
            .and_then(|s| s.entry_channel),
        MessageKind::TicketPost => tickets::settings(&mut conn, guild)
            .await?
            .and_then(|s| s.queue_channel),
        MessageKind::HoneypotWarning => honeypot::settings(&mut conn, guild)
            .await?
            .and_then(|s| s.channel),
    })
}

/// The message or its channel is gone (404 Unknown Message or Channel,
/// 403 Missing Access), or a public receipt's create was refused: post it
/// again where it belongs, wait for a channel, make the receipt private,
/// or mark it gone (§10.3 table).
pub(super) async fn on_missing(
    engine: &Engine,
    job: &ClaimedJob,
    row: &TrackedMessage,
    create_refused: bool,
) -> Result<Handled, DbError> {
    let now = OffsetDateTime::now_utc();
    let token_valid = row
        .interaction_token
        .as_ref()
        .is_some_and(|(_, at)| *at > now);
    let plan = plan_for(
        kind(row),
        current_channel(engine, row).await?,
        create_refused,
        token_valid,
    );
    let mut tx = engine.db().begin().await?;
    let Some(locked) = messages::lock(&mut tx, row.guild, row.id).await? else {
        return finish(tx, job, JobResult::Done).await;
    };
    if locked.generation != row.generation {
        // Moved meanwhile: the new generation's sync takes over.
        return finish(tx, job, JobResult::Done).await;
    }
    match plan {
        MissingPlan::Repost(channel) => {
            messages::relocate(&mut tx, row.guild, row.id, row.generation, Some(channel)).await?;
        }
        MissingPlan::WaitForChannel => {
            messages::relocate(&mut tx, row.guild, row.id, row.generation, None).await?;
        }
        MissingPlan::MarkGone => {
            messages::mark_gone(&mut tx, row.guild, row.id, row.generation).await?;
        }
        MissingPlan::BecomePrivate => {
            messages::make_private(&mut tx, row.guild, row.id, row.generation).await?;
            fence(&mut tx, job).await?;
            tx.commit().await?;
            return become_private(engine, job, row).await;
        }
    }
    let handled = finish(tx, job, JobResult::Done).await?;
    synced(row, Label::Skipped);
    Ok(handled)
}

/// The receipt Quack couldn't post becomes the moderator's private reply:
/// the card with `case.receipt.post_failed` (§10.3).
async fn become_private(
    engine: &Engine,
    job: &ClaimedJob,
    row: &TrackedMessage,
) -> Result<Handled, DbError> {
    let private = TrackedMessage {
        private: true,
        ..row.clone()
    };
    let (Some((token, _)), Some(rendered)) =
        (&row.interaction_token, render(engine, &private).await?)
    else {
        return Ok(Handled::Settle(JobResult::Done));
    };
    let number = match row.owner {
        quack_db::messages::Owner::Case(case) => {
            let mut conn = engine.db().conn().await?;
            quack_db::cases::get(&mut conn, row.guild, case)
                .await?
                .map_or(0, |c| c.number)
        }
        _ => 0,
    };
    let mut msg = rendered.msg;
    msg.blocks.insert(
        0,
        quack_text::Block::Text(quack_text::case::receipt_post_failed(number)),
    );
    let token = InteractionToken::new(token.clone());
    match engine
        .discord()
        .edit_interaction_original(&token, &msg)
        .await
    {
        Outcome::Done(()) => {}
        Outcome::Refused(_) => return Ok(Handled::Settle(JobResult::Done)),
        other => return Ok(Handled::Settle(retry(job, other.code()))),
    }
    let mut tx = engine.db().begin().await?;
    messages::record_posted(
        &mut tx,
        row.guild,
        row.id,
        row.generation,
        row.wanted_revision,
        None,
    )
    .await?;
    let handled = finish(tx, job, JobResult::Done).await?;
    synced(row, Label::Done);
    Ok(handled)
}

/// Step 5: each earlier location is edited to say where the message moved
/// (ticket panels, appeal posts) or deleted; a message that is already
/// gone counts as cleaned up. `Some` when the run must stop here.
pub(super) async fn clean_stale(
    engine: &Engine,
    job: &ClaimedJob,
    row: &TrackedMessage,
) -> Result<Option<Handled>, DbError> {
    for location in &row.stale {
        let outcome = cleanup(engine, row, *location).await;
        match outcome {
            Outcome::Done(()) | Outcome::Refused(_) => {}
            other => return Ok(Some(Handled::Settle(retry(job, other.code())))),
        }
        let mut tx = engine.db().begin().await?;
        messages::remove_stale(&mut tx, row.guild, row.id, *location).await?;
        fence(&mut tx, job).await?;
        tx.commit().await?;
    }
    Ok(None)
}

async fn cleanup(engine: &Engine, row: &TrackedMessage, at: Location) -> Outcome<()> {
    let discord = engine.discord();
    let moved_to = row.channel.map(|id| ChannelRef { id, name: None });
    let moved: Option<Msg> = match (stale_cleanup(kind(row)), moved_to) {
        (StaleCleanup::EditToMoved, Some(to)) if kind(row) == MessageKind::AppealPost => {
            Some(quack_text::appeal::post_moved(&to))
        }
        (StaleCleanup::EditToMoved, Some(to)) => Some(quack_text::ticket::panel_moved(&to)),
        _ => None,
    };
    match moved {
        Some(msg) => discord
            .edit(at.channel_id, at.message_id, &msg, &FileEdit::None)
            .await
            .map(|_| ()),
        None => discord.delete_message(at.channel_id, at.message_id).await,
    }
}
