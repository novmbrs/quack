//! Opening tickets (§13.2.3): the happy path, the one live slot, refusals,
//! and crashes or lost answers at each step of the `open_ticket` job.

mod common;

use anyhow::{Result, bail};
use common::{M, admin, configured, world};
use quack_core::modlog::ModLogKind;
use quack_core::modules::{ModuleProblem, TicketCloseReason, TicketState};
use quack_db::jobs::JobKind;
use quack_engine::port::RefusalKind;
use quack_engine::settings::{SettingsError, set_enabled};
use quack_testkit::fake::{Call, Script};
use quack_testkit::{MEMBER_2, sf};
use quack_tickets::OpenAnswer;

#[tokio::test]
async fn a_member_opens_a_ticket_in_a_private_thread() -> Result<()> {
    let w = world().await?;
    let answer = w.open(M).await?;
    let OpenAnswer::Opening { entry, ticket } = answer else {
        bail!("expected opening, got {answer:?}");
    };
    assert_eq!(entry, w.entry);
    assert_eq!(w.ticket(ticket).await?.state, TicketState::Opening);
    w.h.settle().await?;
    let t = w.ticket(ticket).await?;
    assert_eq!(t.state, TicketState::Open);
    let thread = t.thread.ok_or_else(|| anyhow::anyhow!("thread"))?;
    let (name, members, welcome) = w.h.fake.world(|world| {
        let name = world.guilds[&w.h.guild].channels[&thread].name.clone();
        let members = world
            .thread_members
            .get(&thread)
            .cloned()
            .unwrap_or_default();
        let welcome = world.sent_to(thread).len();
        (name, members, welcome)
    });
    assert_eq!(t.number, 1);
    assert_eq!(
        name, "ticket-0001",
        "a readable name, by the server's number"
    );
    assert_eq!(members, vec![sf(M)]);
    assert_eq!(welcome, 1, "one welcome message");
    let posts = w.h.fake.world(|world| world.sent_to(w.queue).len());
    assert_eq!(posts, 1, "the staff post is in the queue channel");
    assert!(w.log_kinds().await?.contains(&ModLogKind::TicketOpened));
    // The panel is in the entry channel.
    let panel = w.h.fake.world(|world| world.sent_to(w.entry).len());
    assert_eq!(panel, 1);
    assert_eq!(
        w.open(M).await?,
        OpenAnswer::Exists { thread },
        "one live ticket per member"
    );
    Ok(())
}

#[tokio::test]
async fn two_simultaneous_opens_make_one_ticket() -> Result<()> {
    let w = world().await?;
    let (a, b) = tokio::join!(w.open(M), w.open(M));
    let answers = [a?, b?];
    let tickets: Vec<_> = answers
        .iter()
        .filter_map(|a| match a {
            OpenAnswer::Opening { ticket, .. } => Some(*ticket),
            _ => None,
        })
        .collect();
    assert_eq!(tickets.len(), 2, "both are told it is opening");
    assert_eq!(tickets[0], tickets[1], "the same ticket");
    w.h.settle().await?;
    assert_eq!(w.threads(), 1);
    assert_eq!(w.h.db.count("SELECT count(*) FROM tickets").await?, 1);
    Ok(())
}

#[tokio::test]
async fn tickets_off_or_not_set_up() -> Result<()> {
    let w = configured().await?;
    assert_eq!(w.open(M).await?, OpenAnswer::Disabled);
    // Enabling is refused until the channels are set up.
    w.h.db
        .execute("UPDATE ticket_settings SET queue_channel_id = NULL")
        .await?;
    let refused = set_enabled(
        &w.h.engine,
        &admin(&w.h),
        &quack_tickets::hooks(),
        true,
        &w.h.key("enable"),
    )
    .await;
    let Err(SettingsError::ModuleNotReady(problems)) = refused else {
        bail!("expected module_not_ready, got {refused:?}");
    };
    assert_eq!(problems, vec![ModuleProblem::TicketQueueMissing]);
    w.h.db
        .execute(&format!(
            "UPDATE ticket_settings SET queue_channel_id = {}",
            w.entry
        ))
        .await?;
    let refused = set_enabled(
        &w.h.engine,
        &admin(&w.h),
        &quack_tickets::hooks(),
        true,
        &w.h.key("enable"),
    )
    .await;
    let Err(SettingsError::ModuleNotReady(problems)) = refused else {
        bail!("expected module_not_ready, got {refused:?}");
    };
    assert_eq!(problems, vec![ModuleProblem::TicketSameChannel]);
    Ok(())
}

#[tokio::test]
async fn a_lost_thread_create_is_adopted_not_duplicated() -> Result<()> {
    let w = world().await?;
    w.h.fake.script(Call::CreateThread, Script::LoseResponse);
    let OpenAnswer::Opening { ticket, .. } = w.open(M).await? else {
        bail!("expected opening");
    };
    w.h.settle().await?;
    assert_eq!(
        w.h.fake.count(Call::CreateThread),
        1,
        "found, not created again"
    );
    assert_eq!(w.threads(), 1);
    assert_eq!(w.ticket(ticket).await?.state, TicketState::Open);
    Ok(())
}

#[tokio::test]
async fn a_crash_after_the_create_adopts_the_thread() -> Result<()> {
    let w = world().await?;
    let gate = w.h.fake.hold_after(Call::CreateThread);
    let OpenAnswer::Opening { ticket, .. } = w.open(M).await? else {
        bail!("expected opening");
    };
    w.h.crash_at(JobKind::OpenTicket, &gate).await?;
    assert_eq!(w.threads(), 1, "the thread exists but was never saved");
    assert_eq!(w.ticket(ticket).await?.thread, None);
    w.h.settle().await?;
    assert_eq!(w.threads(), 1);
    assert_eq!(w.ticket(ticket).await?.state, TicketState::Open);
    Ok(())
}

#[tokio::test]
async fn a_crash_while_welcoming_posts_one_welcome() -> Result<()> {
    let w = world().await?;
    let gate = w.h.fake.hold_after(Call::Send);
    let OpenAnswer::Opening { ticket, .. } = w.open(M).await? else {
        bail!("expected opening");
    };
    w.h.crash_at(JobKind::OpenTicket, &gate).await?;
    w.h.settle().await?;
    let t = w.ticket(ticket).await?;
    assert_eq!(t.state, TicketState::Open);
    let thread = t.thread.ok_or_else(|| anyhow::anyhow!("thread"))?;
    let welcomes = w.h.fake.world(|world| world.sent_to(thread).len());
    assert_eq!(welcomes, 1, "the nonce made the repeat return the first");
    Ok(())
}

#[tokio::test]
async fn a_refused_thread_frees_the_slot() -> Result<()> {
    let w = world().await?;
    w.h.fake.script(
        Call::CreateThread,
        Script::refuse(RefusalKind::MissingPermissions),
    );
    let OpenAnswer::Opening { ticket, .. } = w.open(M).await? else {
        bail!("expected opening");
    };
    w.h.settle().await?;
    let t = w.ticket(ticket).await?;
    assert_eq!(t.state, TicketState::Closed);
    assert_eq!(t.close_reason, Some(TicketCloseReason::OpenFailed));
    assert!(w.live(M).await?.is_none(), "the slot is free");
    assert!(matches!(w.open(M).await?, OpenAnswer::Opening { .. }));
    Ok(())
}

#[tokio::test]
async fn a_refused_member_add_deletes_the_thread_and_frees_the_slot() -> Result<()> {
    let w = world().await?;
    w.h.fake.script(
        Call::AddThreadMember,
        Script::refuse(RefusalKind::MissingAccess),
    );
    let OpenAnswer::Opening { ticket, .. } = w.open(M).await? else {
        bail!("expected opening");
    };
    w.h.settle().await?;
    let t = w.ticket(ticket).await?;
    assert_eq!(t.state, TicketState::Closed);
    assert_eq!(t.close_reason, Some(TicketCloseReason::OpenFailed));
    assert!(t.thread_deleted_at.is_some());
    assert_eq!(w.threads(), 0, "the thread was deleted");
    assert_eq!(
        w.h.db
            .count("SELECT count(*) FROM ticket_transcripts")
            .await?,
        0,
        "no transcript for a ticket that never opened"
    );
    assert_eq!(
        w.h.db.count("SELECT count(*) FROM dms").await?,
        0,
        "no DM for a ticket that never opened"
    );
    Ok(())
}

#[tokio::test]
async fn a_spent_budget_fails_the_open_and_frees_the_slot() -> Result<()> {
    let w = world().await?;
    w.h.fake.script_n(Call::FindThreads, 4, &Script::Unknown);
    let OpenAnswer::Opening { ticket, .. } = w.open(M).await? else {
        bail!("expected opening");
    };
    w.h.settle().await?;
    let t = w.ticket(ticket).await?;
    assert_eq!(t.state, TicketState::Closed);
    assert!(w.live(M).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn another_members_ticket_is_separate() -> Result<()> {
    let w = world().await?;
    let a = w.opened(M).await?;
    let b = w.opened(MEMBER_2).await?;
    assert_ne!(a.id, b.id);
    assert_ne!(a.thread, b.thread);
    assert_eq!(w.threads(), 2);
    assert_eq!((a.number, b.number), (1, 2), "numbered per server");
    let names = w.h.fake.world(|world| {
        [a.thread, b.thread].map(|t| {
            t.and_then(|t| world.guilds[&w.h.guild].channels.get(&t))
                .map(|c| c.name.clone())
        })
    });
    assert_eq!(
        names,
        [
            Some("ticket-0001".to_owned()),
            Some("ticket-0002".to_owned())
        ]
    );
    // Another server counts its own tickets from 1.
    let (other, _) = common::second_guild(&w.h).await?;
    let mut tx = w.h.engine.db().begin().await?;
    let first = quack_db::tickets::insert(
        &mut tx,
        quack_core::ids::TicketId::from_uuid(uuid::Uuid::now_v7()),
        other,
        sf(M),
        w.entry,
    )
    .await?;
    tx.commit().await?;
    assert_eq!(first, 1);
    Ok(())
}
