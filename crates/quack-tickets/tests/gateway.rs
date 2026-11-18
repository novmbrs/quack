//! Gateway events (§11.9, §13.1.5), fixing access, reading tickets, and
//! cross-server isolation.

mod common;

use anyhow::{Context as _, Result};
use common::{M, second_guild, world};
use quack_core::modules::TicketState;
use quack_testkit::{MEMBER_2, MODERATOR, sf};
use quack_tickets::{FixAnswer, TicketError};

#[tokio::test]
async fn a_deleted_thread_closes_the_ticket() -> Result<()> {
    let w = world().await?;
    let t = w.opened(M).await?;
    let thread = t.thread.context("thread")?;
    w.say(&t, M, "before the deletion").await?;
    w.h.fake
        .world(|world| world.delete_channel(w.h.guild, thread));
    assert!(quack_tickets::on_thread_deleted(&w.h.engine, w.h.guild, thread).await?);
    w.h.settle().await?;
    let closed = w.ticket(t.id).await?;
    assert_eq!(closed.state, TicketState::Closed);
    assert_eq!(closed.closed_by, None, "Quack closed it");
    let transcript = w.transcript(&t).await?;
    assert!(transcript.content.contains("before the deletion"));
    assert!(w.live(M).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn a_thread_deleted_while_quack_was_away_is_found() -> Result<()> {
    let w = world().await?;
    let gone = w.opened(M).await?;
    let kept = w.opened(MEMBER_2).await?;
    let gone_thread = gone.thread.context("thread")?;
    let kept_thread = kept.thread.context("thread")?;
    w.h.fake
        .world(|world| world.delete_channel(w.h.guild, gone_thread));
    // The kept thread is archived, so it is not among the active threads,
    // but Discord still has it.
    let closed = quack_tickets::on_guild_create(&w.h.engine, w.h.guild, &[]).await?;
    assert_eq!(closed, 1);
    w.h.settle().await?;
    assert_eq!(w.ticket(gone.id).await?.state, TicketState::Closed);
    assert_eq!(w.ticket(kept.id).await?.state, TicketState::Open);
    assert_eq!(
        quack_tickets::on_guild_create(&w.h.engine, w.h.guild, &[kept_thread]).await?,
        0
    );
    Ok(())
}

#[tokio::test]
async fn an_invalid_session_marks_live_journals_gapped() -> Result<()> {
    let w = world().await?;
    let t = w.opened(M).await?;
    assert!(!t.journal_gaps);
    assert_eq!(quack_tickets::on_invalid_session(&w.h.engine).await?, 1);
    assert!(w.ticket(t.id).await?.journal_gaps);
    Ok(())
}

#[tokio::test]
async fn the_journal_ignores_channels_and_other_threads() -> Result<()> {
    let w = world().await?;
    let t = w.opened(M).await?;
    let thread = t.thread.context("thread")?;
    let mut entry = w.entry_for(thread, sf(77), M, "not a thread");
    entry.parent = None;
    w.journal.on_message(&w.h.engine, &entry).await;
    let mut entry = w.entry_for(w.entry, sf(78), M, "another thread");
    entry.channel = sf(999);
    w.journal.on_message(&w.h.engine, &entry).await;
    let mut entry = w.entry_for(thread, sf(79), M, "a twice-delivered event");
    w.journal.on_message(&w.h.engine, &entry).await;
    entry.text = "the same message again".to_owned();
    w.journal.on_message(&w.h.engine, &entry).await;
    assert_eq!(
        w.h.db.count("SELECT count(*) FROM ticket_messages").await?,
        1
    );
    assert_eq!(w.journal.pending(), 0);
    Ok(())
}

#[tokio::test]
async fn fixing_access_removes_people_who_may_not_read() -> Result<()> {
    let w = world().await?;
    let t = w.opened(M).await?;
    let thread = t.thread.context("thread")?;
    w.h.fake.world(|world| {
        let members = world.thread_members.entry(thread).or_default();
        members.push(sf(MEMBER_2));
        members.push(sf(MODERATOR));
    });
    let answer = quack_tickets::fix_access(&w.h.engine, &w.h.moderator(), t.id).await?;
    assert_eq!(
        answer,
        FixAnswer::Done {
            member: sf(M),
            removed: 1
        }
    );
    let members = w.h.fake.world(|world| {
        world
            .thread_members
            .get(&thread)
            .cloned()
            .unwrap_or_default()
    });
    assert!(members.contains(&sf(M)) && members.contains(&sf(MODERATOR)));
    assert!(!members.contains(&sf(MEMBER_2)));
    let refused = quack_tickets::fix_access(&w.h.engine, &w.member_principal(M), t.id).await;
    assert!(
        matches!(refused, Err(TicketError::Refused(_))),
        "{refused:?}"
    );
    Ok(())
}

#[tokio::test]
async fn members_read_their_own_tickets_and_staff_read_all() -> Result<()> {
    let w = world().await?;
    let t = w.opened(M).await?;
    let mine = quack_tickets::get(&w.h.engine, &w.member_principal(M), t.id).await?;
    assert_eq!(mine.id, t.id);
    let other = quack_tickets::get(&w.h.engine, &w.member_principal(MEMBER_2), t.id).await;
    assert!(matches!(other, Err(TicketError::NotAllowed)), "{other:?}");
    let all = quack_tickets::list(&w.h.engine, &w.h.moderator(), None, None, 50).await?;
    assert_eq!(all.len(), 1);
    let none = quack_tickets::transcript(&w.h.engine, &w.h.moderator(), t.id).await;
    assert!(matches!(none, Err(TicketError::TranscriptUnavailable)));
    w.close_as(&t, M).await?;
    w.h.settle().await?;
    let transcript = quack_tickets::transcript(&w.h.engine, &w.member_principal(M), t.id).await?;
    assert_eq!(transcript.partial, None);
    // Expired transcripts are no longer served.
    w.h.db
        .execute("UPDATE ticket_transcripts SET expires_at = now() - interval '1 second'")
        .await?;
    let expired = quack_tickets::transcript(&w.h.engine, &w.h.moderator(), t.id).await;
    assert!(matches!(expired, Err(TicketError::TranscriptUnavailable)));
    Ok(())
}

#[tokio::test]
async fn other_servers_see_nothing() -> Result<()> {
    let w = world().await?;
    let t = w.opened(M).await?;
    let thread = t.thread.context("thread")?;
    let (other, moderator) = second_guild(&w.h).await?;
    let got = quack_tickets::get(&w.h.engine, &moderator, t.id).await;
    assert!(matches!(got, Err(TicketError::NotFound)), "{got:?}");
    let closed = quack_tickets::close(&w.h.engine, &moderator, t.id).await;
    assert!(matches!(closed, Err(TicketError::NotFound)), "{closed:?}");
    let fixed = quack_tickets::fix_access(&w.h.engine, &moderator, t.id).await;
    assert!(matches!(fixed, Err(TicketError::NotFound)), "{fixed:?}");
    let read = quack_tickets::transcript(&w.h.engine, &moderator, t.id).await;
    assert!(matches!(read, Err(TicketError::NotFound)), "{read:?}");
    assert!(
        quack_tickets::list(&w.h.engine, &moderator, None, None, 50)
            .await?
            .is_empty()
    );
    assert!(!quack_tickets::on_thread_deleted(&w.h.engine, other, thread).await?);
    let mut entry = w.entry_for(thread, sf(55), M, "from elsewhere");
    entry.guild = other;
    w.journal.on_message(&w.h.engine, &entry).await;
    w.journal.on_event_dropped(&w.h.engine, other, thread).await;
    let t = w.ticket(t.id).await?;
    assert_eq!(t.state, TicketState::Open);
    assert!(!t.journal_gaps);
    assert_eq!(
        w.h.db.count("SELECT count(*) FROM ticket_messages").await?,
        0
    );
    assert_eq!(
        quack_tickets::on_guild_create(&w.h.engine, other, &[]).await?,
        0
    );
    Ok(())
}
