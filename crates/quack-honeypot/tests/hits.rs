//! From post to case (§13.4.2): the happy path, bursts, exemptions,
//! stale settings, cleanup ordering, and recovery after a restart.

mod common;

use anyhow::{Context as _, Result};
use common::world;
use quack_core::modlog::ModLogKind;
use quack_db::jobs::JobKind;
use quack_honeypot::Intake;
use quack_testkit::fake::{Call, MemberSetup};
use quack_testkit::{ADMIN, MEMBER, MEMBER_2, MODERATOR, OWNER, QUACK, sf};

#[tokio::test]
async fn a_post_becomes_a_case_and_is_deleted_after_it_is_saved() -> Result<()> {
    let w = world().await?;
    let (message, intake) = w.post(MEMBER).await?;
    assert_eq!(intake, Intake::Admitted);
    w.h.settle().await?;
    assert_eq!(w.cases().await?, 1);
    let saved = w
        .h
        .db
        .count(&format!(
            "SELECT count(*) FROM evidence WHERE message_id = {message} AND state = 'saved'"
        ))
        .await?;
    assert_eq!(saved, 1, "the post is the case's evidence");
    assert!(!w.exists(message), "the post was deleted");
    assert_eq!(
        w.h.db
            .count("SELECT count(*) FROM honeypot_hits WHERE state = 'case_created' AND cleaned_up_at IS NOT NULL")
            .await?,
        1
    );
    let timed_out = w
        .h
        .fake
        .world(|world| world.member(w.h.guild, sf(MEMBER)).and_then(|m| m.timeout_until));
    assert!(timed_out.is_some(), "the rule's action ran");
    let warning = w.h.fake.world(|world| world.sent_to(w.channel).len());
    assert_eq!(warning, 1, "the warning is in the channel");
    Ok(())
}

#[tokio::test]
async fn a_burst_from_one_member_is_one_case() -> Result<()> {
    let w = world().await?;
    for _ in 0..3 {
        assert_eq!(w.post(MEMBER).await?.1, Intake::Admitted);
    }
    w.h.settle().await?;
    // A post right after the case joins it too.
    w.post(MEMBER).await?;
    w.h.settle().await?;
    assert_eq!(w.cases().await?, 1);
    assert_eq!(
        w.h.db
            .count("SELECT count(DISTINCT case_id) FROM honeypot_hits WHERE state = 'case_created'")
            .await?,
        1
    );
    assert_eq!(
        w.h.db
            .count("SELECT count(*) FROM honeypot_hits WHERE cleaned_up_at IS NOT NULL")
            .await?,
        4
    );
    // Another member is another case.
    w.post(MEMBER_2).await?;
    w.h.settle().await?;
    assert_eq!(w.cases().await?, 2);
    Ok(())
}

#[tokio::test]
async fn bots_webhooks_and_staff_never_get_a_case_or_a_deletion() -> Result<()> {
    let w = world().await?;
    w.h.fake.world(|world| {
        world.add_member(w.h.guild, &MemberSetup::new(sf(77), 5).bot());
    });
    for author in [MODERATOR, ADMIN, OWNER, QUACK, 77] {
        let (message, intake) = w.post(author).await?;
        assert_eq!(intake, Intake::Exempt, "author {author}");
        assert!(w.exists(message));
    }
    let id = w
        .h
        .fake
        .world(|world| world.post(w.h.guild, w.channel, sf(MEMBER), "hook", Vec::new()));
    let facts = w.facts(w.h.guild, id, w.channel, MEMBER, false, true);
    assert_eq!(
        quack_honeypot::on_message(&w.h.engine, &facts).await?,
        Intake::Exempt
    );
    w.h.settle().await?;
    assert_eq!(w.cases().await?, 0);
    assert_eq!(w.h.db.count("SELECT count(*) FROM honeypot_hits").await?, 0);
    assert!(w.exists(id));
    Ok(())
}

#[tokio::test]
async fn a_member_made_staff_before_the_job_is_exempt() -> Result<()> {
    let w = world().await?;
    let (message, _) = w.post(MEMBER).await?;
    w.h.fake.world(|world| {
        if let Some(m) = world.member_mut(w.h.guild, sf(MEMBER)) {
            m.permissions = quack_core::permissions::Permission::ModerateMembers.into();
        }
    });
    w.h.settle().await?;
    assert_eq!(w.cases().await?, 0);
    assert_eq!(w.failures().await?, ["exempt"]);
    assert!(w.exists(message), "never deleted");
    assert!(!w.log_kinds().await?.contains(&ModLogKind::HoneypotCaseFailed));
    Ok(())
}

#[tokio::test]
async fn changed_settings_refuse_hits_admitted_before() -> Result<()> {
    let w = world().await?;
    let (message, _) = w.post(MEMBER).await?;
    w.save(Some(w.channel), Some(w.rule), Some("Don't post here, please."))
        .await?;
    w.h.settle().await?;
    assert_eq!(w.cases().await?, 0);
    assert_eq!(w.failures().await?, ["policy_changed"]);
    assert!(w.exists(message), "kept for staff");
    assert!(w.log_kinds().await?.contains(&ModLogKind::HoneypotCaseFailed));
    // Turned off mid-flight: the same.
    w.post(MEMBER_2).await?;
    w.enable(false).await?;
    w.h.settle().await?;
    assert_eq!(w.failures().await?, ["policy_changed", "policy_changed"]);
    Ok(())
}

#[tokio::test]
async fn the_post_is_kept_until_the_case_saved_it() -> Result<()> {
    let w = world().await?;
    let (message, _) = w.post(MEMBER).await?;
    w.h.run_one(JobKind::HoneypotCase)
        .await?
        .context("the case job ran")?;
    assert_eq!(w.cases().await?, 1);
    let ran = w
        .h
        .run_one(JobKind::HoneypotCleanup)
        .await?
        .context("the cleanup ran")?;
    assert!(
        matches!(
            ran.end,
            quack_testkit::RunEnd::Settled(quack_db::jobs::JobResult::Defer { .. })
        ),
        "{:?}",
        ran.end
    );
    assert!(w.exists(message), "not deleted while the evidence is saving");
    assert_eq!(w.h.fake.count(Call::DeleteMessage), 0);
    w.h.settle().await?;
    assert!(!w.exists(message));
    Ok(())
}

#[tokio::test]
async fn pending_hits_survive_a_restart() -> Result<()> {
    let w = world().await?;
    w.post(MEMBER).await?;
    w.post(MEMBER_2).await?;
    // The process died and the jobs were lost.
    w.h.db
        .execute("DELETE FROM jobs WHERE kind = 'honeypot_case'")
        .await?;
    assert_eq!(quack_honeypot::recover(&w.h.engine).await?, 2);
    w.h.settle().await?;
    assert_eq!(w.cases().await?, 2);
    assert_eq!(quack_honeypot::recover(&w.h.engine).await?, 0);
    Ok(())
}

#[tokio::test]
async fn a_repeated_event_is_one_hit() -> Result<()> {
    let w = world().await?;
    let (message, _) = w.post(MEMBER).await?;
    let facts = w.facts(w.h.guild, message, w.channel, MEMBER, false, false);
    assert_eq!(
        quack_honeypot::on_message(&w.h.engine, &facts).await?,
        Intake::Duplicate
    );
    Ok(())
}

#[tokio::test]
async fn a_case_path_refusal_fails_the_hits_and_is_logged() -> Result<()> {
    let w = world().await?;
    // Above Quack's role: the case path refuses.
    w.h.fake
        .world(|world| world.add_member(w.h.guild, &MemberSetup::new(sf(88), 55)));
    let (message, intake) = w.post(88).await?;
    assert_eq!(intake, Intake::Admitted);
    w.h.settle().await?;
    assert_eq!(w.cases().await?, 0);
    assert_eq!(w.failures().await?, ["quack_role_too_low"]);
    assert!(w.exists(message));
    assert!(w.log_kinds().await?.contains(&ModLogKind::HoneypotCaseFailed));
    Ok(())
}

#[tokio::test]
async fn discord_unavailable_until_the_budget_ends() -> Result<()> {
    let w = world().await?;
    w.post(MEMBER).await?;
    w.h.fake
        .script_n(Call::Member, 5, &quack_testkit::fake::Script::Unknown);
    w.h.settle().await?;
    assert_eq!(w.failures().await?, ["discord_unavailable"]);
    assert!(w.log_kinds().await?.contains(&ModLogKind::HoneypotCaseFailed));
    Ok(())
}
