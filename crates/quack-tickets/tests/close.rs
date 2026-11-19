//! Closing tickets (§13.2.4): the happy path, who may close, transcripts
//! from the journal and history, and crashes or failures at each step of
//! the `close_ticket` job. The member's slot is always freed.

mod common;

use anyhow::{Context as _, Result, bail};
use common::{M, world};
use quack_core::DomainError;
use quack_core::modlog::ModLogKind;
use quack_core::modules::{TicketState, TranscriptPartial};
use quack_db::jobs::JobKind;
use quack_engine::port::RefusalKind;
use quack_testkit::fake::{Call, Script};
use quack_testkit::{MEMBER_2, MODERATOR, sf};
use quack_tickets::{OpenAnswer, TicketError};

#[tokio::test]
async fn the_member_closes_and_gets_the_transcript() -> Result<()> {
    let w = world().await?;
    let t = w.opened(M).await?;
    w.say(&t, M, "My account was hacked").await?;
    w.say(&t, MODERATOR, "We can help with that").await?;
    w.close_as(&t, M).await?;
    assert_eq!(w.ticket(t.id).await?.state, TicketState::Closing);
    w.h.settle().await?;
    let closed = w.ticket(t.id).await?;
    assert_eq!(closed.state, TicketState::Closed);
    assert_eq!(closed.closed_by, Some(sf(M)));
    assert!(closed.thread_deleted_at.is_some());
    assert_eq!(w.threads(), 0, "the thread is deleted");
    let transcript = w.transcript(&t).await?;
    assert_eq!(transcript.partial, None);
    assert!(transcript.content.contains("My account was hacked"));
    assert!(transcript.content.contains("We can help with that"));
    assert!(transcript.content.contains("Ticket transcript"));
    let dm = w.h.fake.world(|world| {
        world
            .dms_to(sf(M))
            .iter()
            .map(|m| m.fetched.attachments.len())
            .collect::<Vec<_>>()
    });
    assert_eq!(dm, vec![1], "one DM with the transcript file");
    let kinds = w.log_kinds().await?;
    assert!(kinds.contains(&ModLogKind::TicketClosed));
    assert!(w.live(M).await?.is_none(), "the slot is free");
    assert!(matches!(w.open(M).await?, OpenAnswer::Opening { .. }));
    Ok(())
}

/// The staff post's transcript file and whether its message shows it in a
/// File component (Discord drops an unnamed file from a V2 message).
fn staff_post_file(w: &common::World) -> Vec<(String, bool)> {
    w.h.fake.world(|world| {
        world
            .sent_to(w.queue)
            .iter()
            .flat_map(|m| {
                m.fetched.attachments.iter().map(|a| {
                    let shown = m.msg.as_ref().is_some_and(|msg| {
                        msg.blocks
                            .iter()
                            .any(|b| matches!(b, quack_text::Block::File(n) if *n == a.filename))
                    });
                    (a.filename.clone(), shown)
                })
            })
            .collect()
    })
}

#[tokio::test]
async fn the_staff_post_carries_the_transcript_across_edits() -> Result<()> {
    let w = world().await?;
    let t = w.opened(M).await?;
    w.say(&t, M, "Hello").await?;
    w.close_as(&t, M).await?;
    w.h.settle().await?;
    let first = staff_post_file(&w);
    assert_eq!(first.len(), 1, "the transcript is on the post: {first:?}");
    assert!(first[0].1, "a File component shows it");
    // A later edit keeps the same file.
    let mut tx = w.h.engine.db().begin().await?;
    quack_db::messages::bump(&mut tx, w.h.guild, quack_db::messages::Owner::Ticket(t.id)).await?;
    tx.commit().await?;
    w.h.settle().await?;
    assert_eq!(staff_post_file(&w), first, "kept across edits");
    assert!(w.h.fake.count(Call::Edit) >= 2);
    Ok(())
}

#[tokio::test]
async fn who_may_close() -> Result<()> {
    let w = world().await?;
    let t = w.opened(M).await?;
    let refused = quack_tickets::close(
        &w.h.engine,
        &w.member_principal(MEMBER_2),
        t.id,
        &w.h.key("close"),
    )
    .await;
    assert!(
        matches!(refused, Err(TicketError::NotAllowed)),
        "{refused:?}"
    );
    let text = refused.err().and_then(|e| e.text());
    assert!(text.is_some(), "ticket.close.not_allowed");
    w.close_as(&t, MODERATOR).await?;
    let again =
        quack_tickets::close(&w.h.engine, &w.member_principal(M), t.id, &w.h.key("close")).await;
    assert!(
        matches!(again, Err(TicketError::Refused(DomainError::TicketNotOpen))),
        "{again:?}"
    );
    w.h.settle().await?;
    assert_eq!(w.ticket(t.id).await?.closed_by, Some(sf(MODERATOR)));
    Ok(())
}

#[tokio::test]
async fn a_close_replays_by_its_key_and_only_by_it() -> Result<()> {
    let w = world().await?;
    let t = w.opened(M).await?;
    let other = w.opened(MEMBER_2).await?;
    let moderator = w.member_principal(MODERATOR);
    let key = w.h.key("close");
    quack_tickets::close(&w.h.engine, &moderator, t.id, &key).await?;
    // The same request again answers like the first time.
    let replay = quack_tickets::close(&w.h.engine, &moderator, t.id, &key).await;
    assert!(replay.is_ok(), "{replay:?}");
    // Another request for the same ticket is a second close.
    let second = quack_tickets::close(&w.h.engine, &moderator, t.id, &w.h.key("close")).await;
    assert!(
        matches!(
            second,
            Err(TicketError::Refused(DomainError::TicketNotOpen))
        ),
        "{second:?}"
    );
    // The key reused for another ticket is refused, and that ticket stays open.
    let reused = quack_tickets::close(&w.h.engine, &moderator, other.id, &key).await;
    assert!(
        matches!(reused, Err(TicketError::OperationKeyReused)),
        "{reused:?}"
    );
    assert_eq!(w.ticket(other.id).await?.state, TicketState::Open);
    Ok(())
}

#[tokio::test]
async fn messages_arriving_while_closing_come_from_history() -> Result<()> {
    let w = world().await?;
    let t = w.opened(M).await?;
    let thread = t.thread.context("thread")?;
    w.say(&t, M, "first").await?;
    // Posted before the freeze, but its event arrives after it.
    let late =
        w.h.fake
            .world(|world| world.post(w.h.guild, thread, sf(M), "late but posted", Vec::new()));
    w.close_as(&t, M).await?;
    let freeze = w.h.fake.hold_after(Call::SetThreadState);
    let job = w.h.spawn_one(JobKind::CloseTicket).await?;
    freeze.reached().await;
    freeze.release();
    job.await?;
    w.journal
        .on_message(
            &w.h.engine,
            &w.entry_for(thread, late, M, "late but posted"),
        )
        .await;
    assert_eq!(
        w.h.db.count("SELECT count(*) FROM ticket_messages").await?,
        1,
        "the frozen journal admitted nothing more"
    );
    w.h.settle().await?;
    let transcript = w.transcript(&t).await?;
    assert!(transcript.content.contains("first"));
    assert!(transcript.content.contains("late but posted"));
    assert_eq!(transcript.partial, None);
    Ok(())
}

#[tokio::test]
async fn a_message_sent_before_the_freeze_still_reaches_the_transcript() -> Result<()> {
    let w = world().await?;
    let t = w.opened(M).await?;
    let thread = t.thread.context("thread")?;
    // Posted before the freeze and deleted before the history is read; its
    // event arrives after the freeze.
    let late = w
        .h
        .fake
        .world(|world| world.post(w.h.guild, thread, sf(M), "posted, then deleted", Vec::new()));
    let mut entry = w.entry_for(thread, late, M, "posted, then deleted");
    entry.sent_at -= time::Duration::minutes(1);
    w.h.fake
        .world(|world| world.messages.retain(|id, _| *id != late));
    w.close_as(&t, M).await?;
    let history = w.h.fake.hold(Call::FetchHistory);
    let job = w.h.spawn_one(JobKind::CloseTicket).await?;
    history.reached().await;
    w.journal.on_message(&w.h.engine, &entry).await;
    history.release();
    job.await?;
    w.h.settle().await?;
    let transcript = w.transcript(&t).await?;
    assert!(
        transcript.content.contains("posted, then deleted"),
        "{}",
        transcript.content
    );
    Ok(())
}

#[tokio::test]
async fn an_archived_thread_is_unarchived_and_locked_before_it_is_archived() -> Result<()> {
    let w = world().await?;
    let t = w.opened(M).await?;
    w.close_as(&t, M).await?;
    // Auto-archived: Discord takes no other change until it is unarchived.
    w.h.fake
        .script(Call::SetThreadState, Script::refuse(RefusalKind::Other));
    w.h.settle().await?;
    assert_eq!(w.h.fake.count(Call::SetThreadState), 3);
    assert!(w.ticket(t.id).await?.frozen_at.is_some());
    Ok(())
}

#[tokio::test]
async fn a_refused_freeze_is_not_taken_as_frozen() -> Result<()> {
    let w = world().await?;
    let t = w.opened(M).await?;
    w.close_as(&t, M).await?;
    w.h.fake.script(
        Call::SetThreadState,
        Script::refuse(RefusalKind::MissingPermissions),
    );
    w.h.run_one(JobKind::CloseTicket)
        .await?
        .context("the close job ran")?;
    let after = w.ticket(t.id).await?;
    assert!(after.frozen_at.is_none(), "members could still post");
    assert_eq!(after.state, TicketState::Closing);
    // Once Discord lets Quack, the close goes on.
    w.h.settle().await?;
    assert!(w.ticket(t.id).await?.frozen_at.is_some());
    Ok(())
}

#[tokio::test]
async fn journal_text_wins_over_an_edited_history() -> Result<()> {
    let w = world().await?;
    let t = w.opened(M).await?;
    w.say(&t, M, "the original words").await?;
    w.h.fake.world(|world| {
        for m in world.messages.values_mut() {
            if m.fetched.content == "the original words" {
                m.fetched.content = "edited away".to_owned();
            }
        }
    });
    w.close_as(&t, M).await?;
    w.h.settle().await?;
    let transcript = w.transcript(&t).await?;
    assert!(transcript.content.contains("the original words"));
    assert!(!transcript.content.contains("edited away"));
    Ok(())
}

#[tokio::test]
async fn a_crash_at_each_close_step_resumes_once() -> Result<()> {
    for call in [
        Call::SetThreadState,
        Call::FetchHistory,
        Call::DeleteChannel,
    ] {
        let w = world().await?;
        let t = w.opened(M).await?;
        w.say(&t, M, "hello").await?;
        w.close_as(&t, M).await?;
        let gate = w.h.fake.hold_after(call);
        w.h.crash_at(JobKind::CloseTicket, &gate).await?;
        w.h.settle().await?;
        let closed = w.ticket(t.id).await?;
        assert_eq!(
            closed.state,
            TicketState::Closed,
            "after a crash at {call:?}"
        );
        assert!(closed.thread_deleted_at.is_some());
        assert_eq!(
            w.h.db
                .count("SELECT count(*) FROM ticket_transcripts")
                .await?,
            1
        );
        assert_eq!(w.h.db.count("SELECT count(*) FROM dms").await?, 1);
        assert!(w.live(M).await?.is_none());
    }
    Ok(())
}

#[tokio::test]
async fn a_thread_quack_cannot_delete_is_recorded() -> Result<()> {
    let w = world().await?;
    let t = w.opened(M).await?;
    w.close_as(&t, M).await?;
    w.h.fake.script(
        Call::DeleteChannel,
        Script::refuse(RefusalKind::MissingPermissions),
    );
    w.h.settle().await?;
    let closed = w.ticket(t.id).await?;
    assert_eq!(closed.state, TicketState::Closed);
    assert!(closed.thread_delete_failed);
    assert_eq!(closed.thread_deleted_at, None);
    assert_eq!(w.threads(), 1);
    Ok(())
}

#[tokio::test]
async fn a_spent_budget_still_closes_with_the_journal() -> Result<()> {
    let w = world().await?;
    let t = w.opened(M).await?;
    w.say(&t, M, "kept in the journal").await?;
    w.close_as(&t, M).await?;
    w.h.fake.script_n(Call::SetThreadState, 6, &Script::Unknown);
    w.h.fake.script_n(Call::DeleteChannel, 6, &Script::Unknown);
    w.h.settle().await?;
    let closed = w.ticket(t.id).await?;
    assert_eq!(closed.state, TicketState::Closed);
    assert!(closed.thread_delete_failed);
    let transcript = w.transcript(&t).await?;
    assert_eq!(transcript.partial, Some(TranscriptPartial::History));
    assert!(transcript.content.contains("kept in the journal"));
    assert!(w.live(M).await?.is_none(), "the slot is free");
    Ok(())
}

#[tokio::test]
async fn gaps_make_the_transcript_partial_only_without_history() -> Result<()> {
    let w = world().await?;
    let t = w.opened(M).await?;
    let thread = t.thread.context("thread")?;
    w.say(&t, M, "one").await?;
    w.journal
        .on_event_dropped(&w.h.engine, w.h.guild, thread)
        .await;
    assert!(w.ticket(t.id).await?.journal_gaps);
    // History reaches the first message: the gap is filled.
    w.close_as(&t, M).await?;
    w.h.settle().await?;
    assert_eq!(w.transcript(&t).await?.partial, None);

    let t = w.opened(M).await?;
    let thread = t.thread.context("thread")?;
    w.say(&t, M, "two").await?;
    w.journal
        .on_event_dropped(&w.h.engine, w.h.guild, thread)
        .await;
    w.close_as(&t, M).await?;
    w.h.fake.script(
        Call::FetchHistory,
        Script::refuse(RefusalKind::MissingAccess),
    );
    w.h.settle().await?;
    let transcript = w.transcript(&t).await?;
    assert_eq!(transcript.partial, Some(TranscriptPartial::History));
    assert!(transcript.content.contains("two"));
    Ok(())
}

#[tokio::test]
async fn a_huge_transcript_is_cut_at_8_mib() -> Result<()> {
    let w = world().await?;
    let t = w.opened(M).await?;
    // 2,500 journal rows of 4,000 characters: about 10 MB of text.
    w.h.db
        .execute(&format!(
            "INSERT INTO ticket_messages (guild_id, ticket_id, message_id, author_id, author_name, text, sent_at)
             SELECT {}, '{}', 1000 + n, {M}, 'duck', repeat('é', 4000), now() FROM generate_series(1, 2500) n",
            w.h.guild,
            t.id.get()
        ))
        .await?;
    w.close_as(&t, M).await?;
    w.h.settle().await?;
    let transcript = w.transcript(&t).await?;
    assert_eq!(transcript.partial, Some(TranscriptPartial::Trimmed));
    assert!(transcript.content.len() <= 8 * 1024 * 1024);
    assert!(transcript.content.len() > 8 * 1024 * 1024 - 4096);
    Ok(())
}

#[tokio::test]
async fn opening_waits_while_the_previous_ticket_closes() -> Result<()> {
    let w = world().await?;
    let t = w.opened(M).await?;
    w.close_as(&t, M).await?;
    assert_eq!(w.open(M).await?, OpenAnswer::Closing);
    w.h.settle().await?;
    let answer = w.open(M).await?;
    if !matches!(answer, OpenAnswer::Opening { .. }) {
        bail!("expected a new ticket, got {answer:?}");
    }
    Ok(())
}
