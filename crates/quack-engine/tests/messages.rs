//! Tracked messages (§10, §20.1.1 scenario 15): receipts and their sync.

mod common;

use anyhow::{Context as _, Result};
use common::void;
use quack_core::ids::{ChannelId, MessageRowId};
use quack_core::messages::MessageState;
use quack_db::jobs::JobKind;
use quack_db::messages::{self, TrackedMessage};
use quack_engine::cases::{Applied, Receipt, apply_rule};
use quack_engine::port::{InteractionToken, RefusalKind};
use quack_testkit::fake::{Call, Script};
use quack_testkit::rules_for_tests::{levels, warning};
use quack_testkit::{Harness, MEMBER};
use time::OffsetDateTime;

async fn row(h: &Harness, id: MessageRowId) -> Result<TrackedMessage> {
    let mut conn = h.db.db.conn().await?;
    messages::get(&mut conn, h.guild, id)
        .await?
        .context("tracked message")
}

/// Applies a warning with a public receipt in a fresh channel.
async fn public(h: &Harness) -> Result<(MessageRowId, ChannelId, quack_core::ids::CaseId)> {
    let channel = h.fake.world(|w| w.add_text_channel(h.guild, false));
    let rule = h.rule(levels("Spam", &[None], false)).await?;
    let mut request = h.request(MEMBER, rule);
    request.receipt = Some(Receipt::Public {
        channel,
        token: Some((
            InteractionToken::new("token-1"),
            OffsetDateTime::now_utc() + time::Duration::minutes(14),
        )),
    });
    let Applied::Created(created) = apply_rule(&h.engine, request).await? else {
        anyhow::bail!("expected a case");
    };
    Ok((
        created.receipt.context("receipt")?,
        channel,
        created.case.id,
    ))
}

fn posted(h: &Harness, channel: ChannelId) -> usize {
    h.fake.world(|w| w.sent_to(channel).len())
}

#[tokio::test]
async fn a_public_receipt_is_posted_once_and_kept_current() -> Result<()> {
    let h = Harness::new().await?;
    let (id, channel, case) = public(&h).await?;
    h.settle().await?;
    let r = row(&h, id).await?;
    assert!(r.message.is_some());
    assert_eq!(r.posted_revision, r.wanted_revision);
    assert_eq!(posted(&h, channel), 1);
    // The moderator's private reply says the receipt was posted.
    assert_eq!(h.fake.count(Call::EditOriginal), 1);
    void(&h, case).await?;
    h.settle().await?;
    assert_eq!(posted(&h, channel), 1);
    assert!(h.fake.count(Call::Edit) >= 1);
    let r = row(&h, id).await?;
    assert_eq!(r.posted_revision, r.wanted_revision);
    Ok(())
}

/// Rehearsal finding: a voided ban's public receipt says the ban was
/// lifted once the unban is done, in its one action sentence.
#[tokio::test]
async fn a_voided_bans_receipt_says_it_was_lifted() -> Result<()> {
    let h = Harness::new().await?;
    let channel = h.fake.world(|w| w.add_text_channel(h.guild, false));
    let rule = h
        .rule(levels(
            "Spam",
            &[Some(quack_testkit::rules_for_tests::BAN)],
            false,
        ))
        .await?;
    let mut request = h.request(MEMBER, rule);
    request.receipt = Some(Receipt::Public {
        channel,
        token: None,
    });
    let Applied::Created(created) = apply_rule(&h.engine, request).await? else {
        anyhow::bail!("expected a case");
    };
    h.settle().await?;
    let text = || {
        h.fake.world(|w| {
            w.sent_to(channel)
                .first()
                .and_then(|m| m.msg.as_ref())
                .map_or_default(quack_text::Msg::plain)
        })
    };
    assert!(text().contains("Action: Quack banned "), "{}", text());
    void(&h, created.case.id).await?;
    h.settle().await?;
    let after = text();
    assert!(after.contains(" · Voided"), "{after}");
    assert!(
        after.contains(", then unbanned them when the case was voided."),
        "{after}"
    );
    Ok(())
}

#[tokio::test]
async fn a_bump_during_a_sync_is_synced_again() -> Result<()> {
    let h = Harness::new().await?;
    let (id, channel, case) = public(&h).await?;
    // Let the DM settle first so only the receipt is in play.
    h.run_one(JobKind::SendDm).await?;
    let gate = h.fake.hold(Call::Send);
    let task = h.spawn_one(JobKind::SyncMessage).await?;
    gate.reached().await;
    void(&h, case).await?;
    gate.release();
    task.await?;
    h.settle().await?;
    let r = row(&h, id).await?;
    assert_eq!(r.posted_revision, r.wanted_revision);
    assert_eq!(posted(&h, channel), 1);
    assert!(h.fake.count(Call::Edit) >= 1, "the bump was written");
    Ok(())
}

#[tokio::test]
async fn a_move_during_a_create_cleans_up_the_created_message() -> Result<()> {
    let h = Harness::new().await?;
    let (id, channel, _) = public(&h).await?;
    let elsewhere = h.fake.world(|w| w.add_text_channel(h.guild, false));
    let gate = h.fake.hold_after(Call::Send);
    let task = h.spawn_one(JobKind::SyncMessage).await?;
    gate.reached().await;
    let mut tx = h.db.db.begin().await?;
    assert!(messages::relocate(&mut tx, h.guild, id, 1, Some(elsewhere)).await?);
    tx.commit().await?;
    gate.release();
    task.await?;
    h.settle().await?;
    assert_eq!(posted(&h, channel), 0, "the stale create was deleted");
    assert_eq!(posted(&h, elsewhere), 1);
    let r = row(&h, id).await?;
    assert_eq!(r.stale.len(), 0);
    assert_eq!(r.channel, Some(elsewhere));
    Ok(())
}

#[tokio::test]
async fn a_lost_create_is_not_duplicated_inside_the_window() -> Result<()> {
    let h = Harness::new().await?;
    let (_, channel, _) = public(&h).await?;
    h.fake.script(Call::Send, Script::LoseResponse);
    h.settle().await?;
    assert_eq!(posted(&h, channel), 1);
    Ok(())
}

#[tokio::test]
async fn a_lost_create_after_the_window_is_created_again() -> Result<()> {
    let h = Harness::new().await?;
    let (id, channel, _) = public(&h).await?;
    h.fake.script(Call::Send, Script::LoseResponse);
    h.run_jobs().await?;
    // Discord's window passed, and so did Quack's.
    h.fake.world(quack_testkit::fake::World::forget_nonces);
    h.db.execute(
        "UPDATE messages SET create_started_at = create_started_at - interval '61 seconds'",
    )
    .await?;
    h.settle().await?;
    // The known gap of §20.4: one duplicate public post is possible.
    assert_eq!(posted(&h, channel), 2);
    assert!(row(&h, id).await?.message.is_some());
    Ok(())
}

#[tokio::test]
async fn a_private_receipt_past_its_token_is_gone_and_never_created() -> Result<()> {
    let h = Harness::new().await?;
    let channel = h.fake.world(|w| w.add_text_channel(h.guild, false));
    let rule = h.rule(warning("Spam")).await?;
    let mut request = h.request(MEMBER, rule);
    request.receipt = Some(Receipt::Private {
        channel,
        token: InteractionToken::new("token-2"),
        expires_at: OffsetDateTime::now_utc() - time::Duration::seconds(1),
    });
    let Applied::Created(created) = apply_rule(&h.engine, request).await? else {
        anyhow::bail!("expected a case");
    };
    h.settle().await?;
    let r = row(&h, created.receipt.context("receipt")?).await?;
    assert_eq!(r.state, MessageState::Gone);
    assert_eq!(posted(&h, channel), 0);
    assert_eq!(h.fake.count(Call::EditOriginal), 0);
    Ok(())
}

#[tokio::test]
async fn a_private_receipt_is_the_interactions_reply() -> Result<()> {
    let h = Harness::new().await?;
    let channel = h.fake.world(|w| w.add_text_channel(h.guild, false));
    let rule = h.rule(warning("Spam")).await?;
    let mut request = h.request(MEMBER, rule);
    request.receipt = Some(Receipt::Private {
        channel,
        token: InteractionToken::new("token-3"),
        expires_at: OffsetDateTime::now_utc() + time::Duration::minutes(14),
    });
    apply_rule(&h.engine, request).await?;
    h.settle().await?;
    assert_eq!(posted(&h, channel), 0);
    let edits = h.fake.world(|w| w.interaction_edits.clone());
    assert_ne!(edits.len(), 0);
    assert!(edits.iter().all(|(t, _)| t == "token-3"));
    Ok(())
}

#[tokio::test]
async fn a_refused_public_receipt_becomes_private() -> Result<()> {
    let h = Harness::new().await?;
    let (id, channel, _) = public(&h).await?;
    h.fake
        .script(Call::Send, Script::refuse(RefusalKind::MissingPermissions));
    h.run_one(JobKind::SyncMessage).await?;
    let r = row(&h, id).await?;
    assert!(r.private);
    assert_eq!(posted(&h, channel), 0);
    let edits = h.fake.world(|w| w.interaction_edits.clone());
    let plain = edits.last().map_or_default(|(_, m)| m.plain());
    assert!(plain.contains("couldn't post the receipt"), "{plain}");
    Ok(())
}

/// Engine review: the private fallback records its progress only after
/// the moderator's reply carries the note. When that edit's answer is lost,
/// the retry takes the fallback again and the note is still there.
#[tokio::test]
async fn a_refused_public_receipt_keeps_its_note_when_the_edit_is_retried() -> Result<()> {
    let h = Harness::new().await?;
    let (id, channel, _) = public(&h).await?;
    h.fake.script_n(
        Call::Send,
        2,
        &Script::refuse(RefusalKind::MissingPermissions),
    );
    h.fake.script(Call::EditOriginal, Script::LoseResponse);
    h.run_one(JobKind::SyncMessage).await?;
    // The edit may have landed; nothing is recorded yet.
    let r = row(&h, id).await?;
    assert!(!r.private);
    assert!(r.posted_revision < r.wanted_revision);
    h.settle().await?;
    let r = row(&h, id).await?;
    assert!(r.private);
    assert_eq!(r.posted_revision, r.wanted_revision);
    assert_eq!(posted(&h, channel), 0);
    let edits = h.fake.world(|w| w.interaction_edits.clone());
    assert_eq!(edits.len(), 2);
    for (_, msg) in &edits {
        let plain = msg.plain();
        assert!(plain.contains("couldn't post the receipt"), "{plain}");
    }
    Ok(())
}

#[tokio::test]
async fn a_deleted_public_receipt_is_gone_and_the_case_stands() -> Result<()> {
    let h = Harness::new().await?;
    let (id, channel, case) = public(&h).await?;
    h.settle().await?;
    let message = row(&h, id).await?.message.context("posted")?;
    h.fake.world(|w| {
        w.messages.remove(&message);
    });
    void(&h, case).await?;
    h.settle().await?;
    assert_eq!(row(&h, id).await?.state, MessageState::Gone);
    assert_eq!(posted(&h, channel), 0);
    Ok(())
}

/// Engine review MAJOR 9: a stale copy Quack can't delete (403) stays
/// listed with the reason, so a later sync tries again; one already gone
/// (404) is cleaned up.
#[tokio::test]
async fn a_refused_stale_cleanup_keeps_the_location() -> Result<()> {
    let h = Harness::new().await?;
    let (id, channel, case) = public(&h).await?;
    h.settle().await?;
    let elsewhere = h.fake.world(|w| w.add_text_channel(h.guild, false));
    let r = row(&h, id).await?;
    let mut tx = h.db.db.begin().await?;
    assert!(messages::relocate(&mut tx, h.guild, id, r.generation, Some(elsewhere)).await?);
    tx.commit().await?;
    h.fake.script(
        Call::DeleteMessage,
        Script::refuse(RefusalKind::MissingPermissions),
    );
    h.settle().await?;
    let r = row(&h, id).await?;
    assert_eq!(r.stale.len(), 1, "kept for a later try");
    assert_eq!(posted(&h, channel), 1, "the old copy is still there");
    assert!(
        r.last_error
            .as_deref()
            .is_some_and(|e| e.starts_with("stale_cleanup_")),
        "{:?}",
        r.last_error
    );
    // Access is back: the next sync cleans it up.
    let mut tx = h.db.db.begin().await?;
    messages::bump(&mut tx, h.guild, messages::Owner::Case(case)).await?;
    tx.commit().await?;
    h.settle().await?;
    assert_eq!(row(&h, id).await?.stale.len(), 0);
    assert_eq!(posted(&h, channel), 0);
    Ok(())
}
