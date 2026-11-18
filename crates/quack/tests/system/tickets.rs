//! Tickets: turned on from `/setup`, a member opens one, the thread's
//! messages are journaled (across a restart), the member closes it, and
//! the transcript is saved, sent, and readable on the dashboard.

use anyhow::{Context as _, Result};
use axum::http::{Method, StatusCode};
use quack_core::ids::{ChannelId, TicketId};
use quack_discord::custom_id::CustomId;
use quack_testkit::{MEMBER, MODERATOR, sf};
use quack_text::actions::{SetupArea, SetupOp};
use quack_text::render::plain_msg;

use crate::common::{System, admin, click, member};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_ticket_opens_journals_closes_and_keeps_its_transcript() -> Result<()> {
    let mut sys = System::boot().await?;
    sys.install().await?;

    // /setup: Quack creates #support and #ticket-log, then tickets go on.
    let create = CustomId::Setup(SetupArea::Tickets, SetupOp::Create).encode();
    sys.send(&click(admin(), sys.channel, &create, true)).await;
    let made = plain_msg(&sys.answer());
    assert!(made.contains("so members can see it"), "{made}");
    let on = CustomId::Setup(SetupArea::Tickets, SetupOp::TurnOn).encode();
    sys.send(&click(admin(), sys.channel, &on, true)).await;
    let line = plain_msg(&sys.answer());
    assert!(line.contains("Tickets are on"), "{line}");
    sys.settle().await?;
    let entry: ChannelId = sf(u64::try_from(
        sys.db
            .count("SELECT entry_channel_id FROM ticket_settings")
            .await?,
    )?);
    assert!(
        sys.posted(entry)
            .iter()
            .any(|p| p.contains("Open a ticket")),
        "the ticket panel"
    );

    // The member opens a ticket; the job makes the private thread.
    sys.send(&click(
        member(),
        entry,
        &CustomId::OpenTicket.encode(),
        false,
    ))
    .await;
    sys.settle().await?;
    let (ticket, thread): (uuid::Uuid, i64) = {
        let mut conn = sys.db.db.conn().await?;
        sqlx::query_as("SELECT id, thread_id FROM tickets WHERE state = 'open'")
            .fetch_one(&mut *conn)
            .await?
    };
    let ticket = TicketId::from_uuid(ticket);
    let thread: ChannelId = sf(u64::try_from(thread)?);

    // Messages are journaled, before and after a restart.
    sys.say(thread, MEMBER, "Hello, I lost my role after the update.")
        .await;
    sys.say(thread, MODERATOR, "Thanks, which role was it?")
        .await;
    sys.restart().await?;
    sys.say(thread, MEMBER, "The Artists role.").await;
    let journaled = sys.db.count("SELECT count(*) FROM ticket_messages").await?;
    assert_eq!(journaled, 3);

    // The member closes it.
    sys.send(&click(
        member(),
        thread,
        &CustomId::CloseTicket(ticket).encode(),
        false,
    ))
    .await;
    let closing = plain_msg(&sys.answer());
    assert!(closing.to_lowercase().contains("clos"), "{closing}");
    sys.settle().await?;
    let closed = sys
        .db
        .count(
            "SELECT count(*) FROM tickets WHERE state = 'closed' AND thread_deleted_at IS NOT NULL",
        )
        .await?;
    assert_eq!(closed, 1, "closed and its thread deleted");
    assert_eq!(sys.dms(MEMBER).len(), 1, "the transcript DM");

    // The member reads the transcript on the dashboard.
    let session = sys.login(MEMBER).await?;
    let transcript = sys
        .http(
            Method::GET,
            &format!("/api/guilds/1001/tickets/{}/transcript", ticket.get()),
            Some(&session),
            None,
        )
        .await;
    assert_eq!(transcript.status, StatusCode::OK, "{}", transcript.json);
    let content = transcript.json["content"].as_str().context("content")?;
    for said in [
        "I lost my role after the update.",
        "which role was it?",
        "The Artists role.",
    ] {
        assert!(content.contains(said), "{said} in {content}");
    }
    sys.assert_no_duplicate_effects();
    Ok(())
}
