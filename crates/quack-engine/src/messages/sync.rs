//! The `sync_message` handler (§10.3): read, render, write, record, clean
//! up, and repeat while the message is behind (at most 3 loops per run).

use quack_core::dm::in_window;
use quack_core::messages::{
    MAX_SYNC_LOOPS, MessageKind, MessageState, SyncRoute, SyncRow, create_nonce, is_missing,
    needs_sync, sync_route,
};
use quack_core::permissions::{Permission, Permissions};
use quack_db::jobs::{ClaimedJob, Handled, JobResult, fence};
use quack_db::messages::{self, Location, TrackedMessage};
use quack_db::{DbError, guilds};
use quack_telemetry::labels::Outcome as Label;
use time::OffsetDateTime;

use super::render::{Rendered, render};
use super::stale::{clean_stale, on_missing};
use super::{Payload, finish, give_up, kind, synced};
use crate::channels::{ChannelCheck, staff_channel};
use crate::jobs::{dead, retry};
use crate::port::{FileEdit, InteractionToken, Nonce, Outcome, RefusalKind, SentMessage};
use crate::{Engine, obs};

/// Brings one tracked message up to date (§10.3). One writer per row: the
/// job is keyed by the row, and every write-back is compared on the
/// generation read, so a message moved meanwhile is cleaned up, not
/// overwritten.
pub async fn sync(engine: &Engine, job: &ClaimedJob) -> Result<Handled, DbError> {
    let (Some(guild), Ok(payload)) = (job.guild, job.payload::<Payload>()) else {
        return Ok(dead("bad_payload"));
    };
    obs::use_case("sync_message", Some(guild), async {
        for _ in 0..MAX_SYNC_LOOPS {
            match pass(engine, job, payload).await? {
                Pass::Finished(handled) => return Ok(handled),
                Pass::Again => {}
            }
        }
        Ok(Handled::Settle(JobResult::Continue))
    })
    .await
}

pub(super) enum Pass {
    Finished(Handled),
    Again,
}

/// What the read step settled on.
struct Read {
    row: TrackedMessage,
    route: SyncRoute,
    write: bool,
}

async fn pass(engine: &Engine, job: &ClaimedJob, payload: Payload) -> Result<Pass, DbError> {
    let Some(read) = read(engine, job, payload).await? else {
        return Ok(Pass::Finished(Handled::Settled(JobResult::Done)));
    };
    let row = &read.row;
    let mut created = None;
    if read.write {
        let Some(rendered) = render(engine, row).await? else {
            return give_up(engine, job, row, "owner_missing")
                .await
                .map(Pass::Finished);
        };
        match write(engine, job, row, read.route, &rendered).await? {
            Written::Posted(sent) => created = sent,
            Written::Finished(handled) => return Ok(Pass::Finished(handled)),
        }
    }
    let after = record(engine, job, row, read.write, created).await?;
    if let Some(handled) = clean_stale(engine, job, &after).await? {
        return Ok(Pass::Finished(handled));
    }
    if needs_sync(after.wanted_revision, after.posted_revision) {
        return Ok(Pass::Again);
    }
    if read.write {
        synced(row, Label::Done);
    }
    Ok(Pass::Finished(Handled::Settle(JobResult::Done)))
}

/// Step 1, one short transaction: lock the row, decide the route, and
/// start a create's window. `None` when the run finished here.
async fn read(
    engine: &Engine,
    job: &ClaimedJob,
    payload: Payload,
) -> Result<Option<Read>, DbError> {
    let mut tx = engine.db().begin().await?;
    let Some(guild) = job.guild else {
        return Err(DbError::NotFound);
    };
    let Some(row) = messages::lock(&mut tx, guild, payload.message_row_id).await? else {
        finish(
            tx,
            job,
            JobResult::Dead {
                error: "message_row_missing".to_owned(),
            },
        )
        .await?;
        return Ok(None);
    };
    let installed = guilds::get(&mut tx, guild)
        .await?
        .is_some_and(|g| g.installed);
    if row.state == MessageState::Gone || !installed {
        if row.state == MessageState::Active {
            messages::mark_gone(&mut tx, row.guild, row.id, row.generation).await?;
        }
        finish(tx, job, JobResult::Done).await?;
        return Ok(None);
    }
    let write = needs_sync(row.wanted_revision, row.posted_revision);
    if !write && row.stale.is_empty() {
        finish(tx, job, JobResult::Done).await?;
        return Ok(None);
    }
    let now = OffsetDateTime::now_utc();
    let route = sync_route(SyncRow {
        private: row.private,
        token_valid: row
            .interaction_token
            .as_ref()
            .is_some_and(|(_, at)| *at > now),
        has_channel: row.channel.is_some(),
        posted: row.message.is_some(),
    });
    if write && route == SyncRoute::MarkGone {
        messages::mark_gone(&mut tx, row.guild, row.id, row.generation).await?;
        finish(tx, job, JobResult::Done).await?;
        synced(&row, Label::Skipped);
        return Ok(None);
    }
    if write && route == SyncRoute::Wait {
        finish(tx, job, JobResult::Done).await?;
        return Ok(None);
    }
    if write && route == SyncRoute::Create {
        // §10.4: after the window the earlier create's outcome is unknown,
        // so the create is made again with a new start time.
        let restart = row.create_started_at.is_some_and(|s| !in_window(s, now));
        messages::start_create(&mut tx, row.guild, row.id, row.generation, restart)
            .await?
            .ok_or(DbError::Stale)?;
    }
    fence(&mut tx, job).await?;
    tx.commit().await?;
    Ok(Some(Read { row, route, write }))
}

enum Written {
    /// Written; the id of a message this run created.
    Posted(Option<SentMessage>),
    Finished(Handled),
}

/// Step 3: the write for the route.
async fn write(
    engine: &Engine,
    job: &ClaimedJob,
    row: &TrackedMessage,
    route: SyncRoute,
    rendered: &Rendered,
) -> Result<Written, DbError> {
    let discord = engine.discord();
    // A staff check, the write, and the moderator's confirmation.
    if !crate::jobs::has_room(job, 3) {
        return Ok(Written::Finished(Handled::Settle(JobResult::Continue)));
    }
    if route == SyncRoute::EditOriginal {
        let Some((token, _)) = &row.interaction_token else {
            return Ok(Written::Posted(None));
        };
        let token = InteractionToken::new(token.clone());
        return Ok(
            match discord
                .edit_interaction_original(&token, &rendered.msg)
                .await
            {
                Outcome::Done(()) => Written::Posted(None),
                Outcome::Refused(r) => {
                    let code = quack_core::messages::sync_error_code(r.status, r.code);
                    Written::Finished(give_up(engine, job, row, &code).await?)
                }
                other => Written::Finished(Handled::Settle(retry(job, other.code()))),
            },
        );
    }
    let Some(channel) = row.channel else {
        return Ok(Written::Posted(None));
    };
    if let Some(handled) = staff_check(engine, job, row).await? {
        return Ok(Written::Finished(handled));
    }
    let outcome = if let (SyncRoute::EditWithBot, Some(message)) = (route, row.message) {
        let files = match (&rendered.attach, row.attachment_id) {
            (Some(file), _) => FileEdit::Replace(file.clone()),
            (None, Some(att)) => FileEdit::Keep(att),
            (None, None) => FileEdit::None,
        };
        discord.edit(channel, message, &rendered.msg, &files).await
    } else {
        let nonce = Nonce::new(create_nonce(row.id, row.generation));
        let files: Vec<_> = rendered.attach.iter().cloned().collect();
        discord.send(channel, &rendered.msg, &files, &nonce).await
    };
    let creating = row.message.is_none();
    match outcome {
        Outcome::Done(sent) => {
            if creating {
                tell_moderator_posted(engine, row).await;
            }
            Ok(Written::Posted(Some(sent)))
        }
        Outcome::Refused(r)
            if is_missing(r.code) || (creating && kind(row) == MessageKind::CaseReceipt) =>
        {
            on_missing(
                engine,
                job,
                row,
                creating,
                matches!(
                    r.kind,
                    RefusalKind::UnknownChannel | RefusalKind::MissingAccess
                ),
            )
            .await
            .map(Written::Finished)
        }
        Outcome::Refused(r) => {
            let code = quack_core::messages::sync_error_code(r.status, r.code);
            give_up(engine, job, row, &code)
                .await
                .map(Written::Finished)
        }
        other => Ok(Written::Finished(Handled::Settle(retry(job, other.code())))),
    }
}

/// Appeal and ticket posts re-check that their channel is still staff-only
/// before any create or edit (§10.3).
async fn staff_check(
    engine: &Engine,
    job: &ClaimedJob,
    row: &TrackedMessage,
) -> Result<Option<Handled>, DbError> {
    let needs: Permissions = match kind(row) {
        MessageKind::AppealPost => Permission::ViewChannel | Permission::SendMessages,
        MessageKind::TicketPost => {
            Permission::ViewChannel | Permission::SendMessages | Permission::AttachFiles
        }
        _ => return Ok(None),
    };
    let Some(channel) = row.channel else {
        return Ok(None);
    };
    match staff_channel(engine, row.guild, channel, needs).await {
        ChannelCheck::Ok => Ok(None),
        ChannelCheck::Problems(_) => give_up(engine, job, row, "not_staff_only").await.map(Some),
        ChannelCheck::Missing => on_missing(engine, job, row, false, true).await.map(Some),
        ChannelCheck::Unavailable => Ok(Some(Handled::Settle(retry(job, "facts_unavailable")))),
    }
}

/// A public receipt's first post: while the token is valid, the
/// moderator's private reply says the receipt was posted.
async fn tell_moderator_posted(engine: &Engine, row: &TrackedMessage) {
    let (MessageKind::CaseReceipt, false, Some((token, at))) =
        (kind(row), row.private, &row.interaction_token)
    else {
        return;
    };
    if *at <= OffsetDateTime::now_utc() {
        return;
    }
    let number = match row.owner {
        quack_db::messages::Owner::Case(case) => {
            let Ok(mut conn) = engine.db().conn().await else {
                return;
            };
            match quack_db::cases::get(&mut conn, row.guild, case).await {
                Ok(Some(c)) => c.number,
                _ => return,
            }
        }
        _ => return,
    };
    let msg = quack_text::Msg::text(
        quack_text::Tone::Success,
        quack_text::case::receipt_posted(number),
    );
    let token = InteractionToken::new(token.clone());
    let _ = engine
        .discord()
        .edit_interaction_original(&token, &msg)
        .await;
}

/// Step 4, one short transaction: record what was written, compared on
/// the generation read. A message created for a generation that moved
/// meanwhile goes to `stale`, so it is cleaned up.
async fn record(
    engine: &Engine,
    job: &ClaimedJob,
    row: &TrackedMessage,
    wrote: bool,
    sent: Option<SentMessage>,
) -> Result<TrackedMessage, DbError> {
    let mut tx = engine.db().begin().await?;
    let created = sent
        .as_ref()
        .filter(|_| row.message.is_none())
        .map(|s| s.message_id);
    if wrote {
        let recorded = messages::record_posted(
            &mut tx,
            row.guild,
            row.id,
            row.generation,
            row.wanted_revision,
            created,
        )
        .await?;
        if !recorded && let (Some(channel), Some(message)) = (row.channel, created) {
            messages::push_stale(
                &mut tx,
                row.guild,
                row.id,
                Location {
                    channel_id: channel,
                    message_id: message,
                },
            )
            .await?;
        }
        if let Some(att) = sent
            .as_ref()
            .filter(|_| recorded)
            .and_then(|s| s.attachments.first())
            .map(|(_, id)| *id)
            .filter(|_| matches!(row.owner, quack_db::messages::Owner::Ticket(_)))
        {
            messages::set_attachment(&mut tx, row.guild, row.id, row.generation, att).await?;
        }
    }
    let after = messages::lock(&mut tx, row.guild, row.id)
        .await?
        .ok_or(DbError::NotFound)?;
    fence(&mut tx, job).await?;
    tx.commit().await?;
    Ok(after)
}

/// Settles a `sync_message` job whose budget is spent: `last_error =
/// "exhausted"`; the next bump tries again.
pub async fn finalize(engine: &Engine, job: &ClaimedJob) -> Result<Handled, DbError> {
    let (Some(guild), Ok(payload)) = (job.guild, job.payload::<Payload>()) else {
        return Ok(dead("bad_payload"));
    };
    let mut tx = engine.db().begin().await?;
    if let Some(row) = messages::lock(&mut tx, guild, payload.message_row_id).await? {
        messages::set_error(&mut tx, row.guild, row.id, row.generation, "exhausted").await?;
        synced(&row, Label::Failed);
    }
    finish(tx, job, JobResult::Done).await
}
