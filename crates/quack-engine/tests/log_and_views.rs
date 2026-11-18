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
