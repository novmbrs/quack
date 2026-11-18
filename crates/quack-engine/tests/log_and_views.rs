//! The log channel (§9.4) and the staff and member case views (§4.10).

mod common;

use std::collections::BTreeSet;

use anyhow::{Context as _, Result};
use common::{log_kinds, logged, void};
use quack_core::DomainError;
use quack_core::modlog::ModLogKind;
use quack_engine::CaseRef;
use quack_engine::cases::{CaseError, get, member_case};
use quack_engine::port::RefusalKind;
use quack_testkit::fake::{Call, Script};
use quack_testkit::rules_for_tests::{BAN, levels, one};
use quack_testkit::{GUILD, Harness, MEMBER, MEMBER_2, sf};

async fn set_log_channel(h: &Harness, staff_only: bool) -> Result<quack_core::ids::ChannelId> {
    let channel = h.fake.world(|w| w.add_text_channel(h.guild, staff_only));
    h.db.execute(&format!(
        "UPDATE guild_settings SET log_channel_id = {channel} WHERE guild_id = {GUILD}"
    ))
    .await?;
    Ok(channel)
}

#[tokio::test]
async fn posted_entries_reach_the_log_channel_once() -> Result<()> {
    let h = Harness::new().await?;
    let log = set_log_channel(&h, true).await?;
    let rule = h.rule(levels("Spam", &[None], false)).await?;
    h.apply(MEMBER, rule).await?;
    h.fake.script(Call::Send, Script::LoseResponse);
    h.settle().await?;
    // The lost answer was retried with the entry's nonce: one post.
    let posts = h.fake.world(|w| w.sent_to(log).len());
    assert_eq!(posts, 1);
    Ok(())
}

#[tokio::test]
async fn a_log_channel_open_to_everyone_gets_nothing() -> Result<()> {
    let h = Harness::new().await?;
    let log = set_log_channel(&h, false).await?;
    let rule = h.rule(levels("Spam", &[None], false)).await?;
    h.apply(MEMBER, rule).await?;
    h.apply(MEMBER, rule).await?;
    h.settle().await?;
    assert_eq!(h.fake.world(|w| w.sent_to(log).len()), 0);
    // Once per outage.
    assert_eq!(logged(&h, ModLogKind::LogChannelFailed).await?, 1);
    Ok(())
}

#[tokio::test]
async fn a_deleted_log_channel_is_cleared() -> Result<()> {
    let h = Harness::new().await?;
    let log = set_log_channel(&h, true).await?;
    h.fake.world(|w| w.delete_channel(h.guild, log));
    let rule = h.rule(levels("Spam", &[None], false)).await?;
    h.apply(MEMBER, rule).await?;
    h.settle().await?;
    let mut conn = h.db.db.conn().await?;
    let settings = quack_db::settings::get(&mut conn, h.guild)
        .await?
        .context("settings")?;
    assert_eq!(settings.log_channel, None);
    assert!(
        log_kinds(&h)
            .await?
            .contains(&ModLogKind::ChannelSettingCleared)
    );
    Ok(())
}

#[tokio::test]
async fn a_refused_post_is_not_retried() -> Result<()> {
    let h = Harness::new().await?;
    let log = set_log_channel(&h, true).await?;
    let rule = h.rule(levels("Spam", &[None], false)).await?;
    h.apply(MEMBER, rule).await?;
    h.fake
        .script(Call::Send, Script::refuse(RefusalKind::MissingPermissions));
    h.settle().await?;
    assert_eq!(h.fake.count(Call::Send), 1);
    assert_eq!(h.fake.world(|w| w.sent_to(log).len()), 0);
    assert_eq!(logged(&h, ModLogKind::LogChannelFailed).await?, 1);
    Ok(())
}

#[tokio::test]
async fn a_second_outage_after_a_successful_post_is_logged_again() -> Result<()> {
    let h = Harness::new().await?;
    set_log_channel(&h, true).await?;
    let rule = h.rule(levels("Spam", &[None], false)).await?;
    let refuse = || {
        h.fake
            .script(Call::Send, Script::refuse(RefusalKind::MissingPermissions));
    };
    refuse();
    h.apply(MEMBER, rule).await?;
    h.settle().await?;
    assert_eq!(logged(&h, ModLogKind::LogChannelFailed).await?, 1);
    // The log channel works again: the next post succeeds and is recorded.
    h.apply(MEMBER, rule).await?;
    h.settle().await?;
    let mut conn = h.db.db.conn().await?;
    let posted = quack_db::settings::log_channel_posted_at(&mut conn, h.guild).await?;
    assert!(posted.is_some(), "a successful post is recorded");
    drop(conn);
    // A new outage is logged once more.
    refuse();
    h.apply(MEMBER, rule).await?;
    h.settle().await?;
    assert_eq!(logged(&h, ModLogKind::LogChannelFailed).await?, 2);
    Ok(())
}

#[tokio::test]
async fn the_staff_view_has_everything() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(one("Spam", BAN, true)).await?;
    let created = h.apply(MEMBER, rule).await?;
    h.settle().await?;
    let view = get(&h.engine, &h.moderator(), CaseRef::Number(1)).await?;
    assert_eq!(view.id, created.case.id);
    assert_eq!(view.actions.len(), 1);
    assert!(view.actions[0].can_undo);
    assert_ne!(view.actions[0].attempt_log.len(), 0);
    assert!(view.dm.is_some());
    assert!(
        view.timeline
            .iter()
            .any(|t| t.kind == ModLogKind::CaseCreated)
    );
    let missing = get(&h.engine, &h.moderator(), CaseRef::Number(9)).await;
    assert!(matches!(
        missing,
        Err(CaseError::Refused(DomainError::CaseNotFound))
    ));
    Ok(())
}

fn keys(value: &serde_json::Value, prefix: &str, out: &mut BTreeSet<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                let path = format!("{prefix}{k}");
                out.insert(path.clone());
                keys(v, &format!("{path}."), out);
            }
        }
        serde_json::Value::Array(items) => {
            for v in items {
                keys(v, prefix, out);
            }
        }
        _ => {}
    }
}

#[tokio::test]
async fn the_member_view_carries_no_staff_only_field() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(one("Spam", BAN, true)).await?;
    let created = h.apply(MEMBER, rule).await?;
    h.settle().await?;
    void(&h, created.case.id).await?;
    h.settle().await?;
    let view = member_case(&h.engine, sf(MEMBER), created.case.id).await?;
    let json = serde_json::to_value(&view)?;
    let mut found = BTreeSet::new();
    keys(&json, "", &mut found);
    for staff_only in [
        "moderator_id",
        "moderator",
        "source",
        "voided_by",
        "by",
        "author_id",
        "added_by",
        "channel_id",
        "message_id",
        "url",
        "attempt_log",
        "http_status",
        "discord_code",
        "failure_reason",
        "requested_by",
        "dismissed_by",
        "decided_by",
        "case_count",
        "level_index",
        "level_from_case_count",
        "timeline_staff",
        "actor_id",
    ] {
        assert!(
            !found
                .iter()
                .any(|k| k == staff_only || k.ends_with(&format!(".{staff_only}"))),
            "{staff_only} reached the member view: {found:?}"
        );
    }
    assert!(view.void_reason.is_some());
    assert_ne!(view.timeline.len(), 0);
    // Another member's case reads as missing.
    let other = member_case(&h.engine, sf(MEMBER_2), created.case.id).await;
    assert!(matches!(
        other,
        Err(CaseError::Refused(DomainError::CaseNotFound))
    ));
    Ok(())
}

/// Engine review MAJOR 18: two posts refused at the same time in one
/// outage record `log_channel_failed` once. Both runs are held right at
/// their entry insert (the test holds a table lock), after reading "no
/// failure yet"; serialized on the settings row, the second reads the
/// first's entry.
#[tokio::test]
async fn two_failures_at_once_are_one_outage() -> Result<()> {
    let h = Harness::new().await?;
    set_log_channel(&h, false).await?;
    let rule = h.rule(levels("Spam", &[None], false)).await?;
    h.apply(MEMBER, rule).await?;
    h.apply(MEMBER_2, rule).await?;
    let mut hold = h.db.db.begin().await?;
    sqlx::query("LOCK TABLE mod_log IN SHARE MODE")
        .execute(&mut *hold)
        .await?;
    let a = h.spawn_one(quack_db::jobs::JobKind::PostModLog).await?;
    let b = h.spawn_one(quack_db::jobs::JobKind::PostModLog).await?;
    let waiting = "SELECT count(*) FROM pg_stat_activity
                   WHERE datname = current_database() AND wait_event_type = 'Lock'";
    let both = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while h.db.count(waiting).await? < 2 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        anyhow::Ok(())
    })
    .await;
    assert!(both.is_ok(), "both runs reach their writes");
    hold.rollback().await?;
    a.await?;
    b.await?;
    assert_eq!(logged(&h, ModLogKind::LogChannelFailed).await?, 1);
    Ok(())
}

/// Engine review MINOR 25: a case's log sentence names the rule as the
/// case's version had it, even after the rule is renamed.
#[tokio::test]
async fn a_case_sentence_keeps_the_rule_name_it_was_added_under() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(levels("Spam", &[None], false)).await?;
    h.apply(MEMBER, rule).await?;
    let admin = h.principal(quack_testkit::ADMIN, quack_engine::Origin::Dashboard);
    quack_engine::rules::edit(
        &h.engine,
        &admin,
        rule,
        1,
        levels("Harassment", &[None], false),
        &h.key("PUT rules"),
    )
    .await?;
    let page = quack_engine::modlog::list(
        &h.engine,
        &h.moderator(),
        &quack_db::modlog::ModLogFilter::default(),
        None,
        50,
    )
    .await??;
    let created = page
        .entries
        .iter()
        .find(|e| e.kind == ModLogKind::CaseCreated)
        .context("case_created")?;
    assert!(created.text.contains("Spam"), "{}", created.text);
    assert!(!created.text.contains("Harassment"), "{}", created.text);
    Ok(())
}
