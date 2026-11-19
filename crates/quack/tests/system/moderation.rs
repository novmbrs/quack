//! The main story: install, `/setup`, a rule made on the dashboard, three
//! escalating cases (warning, timeout, ban) with DMs, the member's view
//! and appeal over HTTP, the moderator accepting it in Discord, the case
//! voided and the ban undone after its provenance check, with restarts in
//! the middle.

use anyhow::{Context as _, Result};
use axum::http::{Method, StatusCode};
use quack_core::ids::{AppealId, CaseId, ChannelId};
use quack_discord::custom_id::CustomId;
use quack_testkit::{ADMIN, MEMBER, sf};
use quack_text::actions::{SetupArea, SetupOp};
use serde_json::json;

use crate::common::{System, admin, case_add, click, ix, modal_submit, moderator, slash};

/// A setting's channel, read from the database.
async fn setting(sys: &System, column: &str) -> Result<ChannelId> {
    let id = sys
        .db
        .count(&format!("SELECT {column} FROM guild_settings"))
        .await?;
    Ok(sf(u64::try_from(id)?))
}

/// The number of case `n`'s id.
async fn case_id(sys: &System, number: i64) -> Result<CaseId> {
    let mut conn = sys.db.db.conn().await?;
    let id: uuid::Uuid = sqlx::query_scalar("SELECT id FROM cases WHERE number = $1")
        .bind(number)
        .fetch_one(&mut *conn)
        .await?;
    Ok(CaseId::from_uuid(id))
}

async fn installed_and_set_up(sys: &System) -> Result<(ChannelId, ChannelId)> {
    sys.install().await?;
    // /setup: the home panel, then Quack creates the log and appeal channels.
    sys.send(&ix(2, admin(), sys.channel, &slash("setup", &json!([]))))
        .await;
    assert!(
        quack_text::render::plain_msg(&sys.answer()).contains("Log channel"),
        "the home panel"
    );
    for area in [SetupArea::Log, SetupArea::Appeals] {
        let id = CustomId::Setup(area, SetupOp::Create).encode();
        sys.send(&click(admin(), sys.channel, &id, true)).await;
        let line = quack_text::render::plain_msg(&sys.answer());
        assert!(line.contains("Quack created"), "{line}");
    }
    let log = setting(sys, "log_channel_id").await?;
    let appeals = setting(sys, "appeal_channel_id").await?;
    sys.settle().await?;
    Ok((log, appeals))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
pub(crate) async fn warning_timeout_ban_appeal_accepted_and_ban_undone() -> Result<()> {
    let mut sys = System::boot().await?;
    let (log, appeals) = installed_and_set_up(&sys).await?;

    // The dashboard makes a rule: a warning, then a timeout, then a ban.
    let manager = sys.login(ADMIN).await?;
    let created = sys
        .http(
            Method::POST,
            "/api/guilds/1001/rules",
            Some(&manager),
            Some(json!({
                "name": "Spam", "description": "Unwanted ads.", "reason": "Please don't spam.",
                "appealable": true, "counting_window_days": null, "fields": [],
                "levels": [
                    {"from_case_count": 1, "action": null, "send_dm": true},
                    {"from_case_count": 2, "action": {"kind": "timeout", "duration_seconds": 3600}, "send_dm": true},
                    {"from_case_count": 3, "action": {"kind": "ban", "delete_message_seconds": 0}, "send_dm": true}
                ]
            })),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.json);
    let rule = created.json["id"].as_str().context("rule id")?.to_owned();

    // Case #1: a warning and its DM.
    sys.send(&ix(
        2,
        moderator(),
        sys.channel,
        &case_add(MEMBER, &rule, None),
    ))
    .await;
    sys.settle().await?;
    assert_eq!(sys.dms(MEMBER).len(), 1, "the warning's DM");

    // Case #2: a timeout. Quack restarts before any job runs; nothing is lost.
    sys.send(&ix(
        2,
        moderator(),
        sys.channel,
        &case_add(MEMBER, &rule, None),
    ))
    .await;
    sys.restart().await?;
    sys.settle().await?;
    let timed_out = sys.fake.world(|w| {
        w.member(sys.guild, sf(MEMBER))
            .and_then(|m| m.timeout_until)
    });
    assert!(timed_out.is_some(), "the timeout applied once");
    assert_eq!(sys.dms(MEMBER).len(), 2);

    // Case #3: a ban; the DM goes out first, then the ban with its marker.
    sys.send(&ix(
        2,
        moderator(),
        sys.channel,
        &case_add(MEMBER, &rule, None),
    ))
    .await;
    sys.settle().await?;
    let marker = sys.fake.world(|w| {
        w.ban(sys.guild, sf(MEMBER))
            .and_then(quack_core::permissions::BanFacts::case_marker)
    });
    assert_eq!(marker, Some(3), "banned for case #3");
    assert_eq!(sys.dms(MEMBER).len(), 3);
    let ban_dm_first = {
        let calls = sys.fake.calls();
        let dm = calls
            .iter()
            .rposition(|c| c.call == quack_testkit::fake::Call::OpenDm);
        let ban = calls
            .iter()
            .position(|c| c.call == quack_testkit::fake::Call::Ban);
        dm.zip(ban).is_some_and(|(d, b)| d < b)
    };
    assert!(ban_dm_first, "the ban's DM was sent before the ban");

    // The member, banned, still sees the case and appeals on the dashboard.
    let member_session = sys.login(MEMBER).await?;
    let third = case_id(&sys, 3).await?;
    let mine = sys
        .http(Method::GET, "/api/me/cases", Some(&member_session), None)
        .await;
    assert_eq!(mine.status, StatusCode::OK, "{}", mine.json);
    assert_eq!(mine.json["cases"].as_array().map(Vec::len), Some(3));
    let appealed = sys
        .http(
            Method::POST,
            &format!("/api/me/cases/{}/appeal", third.get()),
            Some(&member_session),
            Some(json!({"statement": "I posted my own server's invite by mistake."})),
        )
        .await;
    assert_eq!(appealed.status, StatusCode::CREATED, "{}", appealed.json);

    // Quack restarts; the appeal post still reaches the appeal channel.
    sys.restart().await?;
    sys.settle().await?;
    let posts = sys.posted(appeals);
    assert!(
        posts.iter().any(|p| p.contains("posted my own server")),
        "the appeal post: {posts:#?}"
    );

    // The moderator accepts it in Discord: the button opens the reason
    // modal, the submit decides.
    let (appeal, version): (uuid::Uuid, i32) = {
        let mut conn = sys.db.db.conn().await?;
        sqlx::query_as("SELECT id, version FROM appeals")
            .fetch_one(&mut *conn)
            .await?
    };
    let appeal = AppealId::from_uuid(appeal);
    let version = u32::try_from(version)?;
    let accept = CustomId::AcceptAppeal { appeal, version }.encode();
    sys.send(&click(moderator(), appeals, &accept, false)).await;
    assert!(sys.modal().is_some(), "the reason modal");
    let submit = CustomId::AcceptAppealModal { appeal, version }.encode();
    sys.send(&modal_submit(
        moderator(),
        appeals,
        &submit,
        &[("reason", "It was a mistake; welcome back.")],
    ))
    .await;
    sys.settle().await?;

    // The case is voided and the ban undone (Quack's own marker of a
    // voided case); the member got one DM about the decision.
    let voided = sys
        .db
        .count("SELECT count(*) FROM cases WHERE number = 3 AND void_cause = 'appeal_accepted'")
        .await?;
    assert_eq!(voided, 1);
    assert!(
        sys.fake.world(|w| w.ban(sys.guild, sf(MEMBER)).is_none()),
        "unbanned"
    );
    let unbans = sys
        .db
        .count("SELECT count(*) FROM actions WHERE kind = 'unban' AND state = 'done'")
        .await?;
    assert_eq!(unbans, 1);
    assert_eq!(sys.dms(MEMBER).len(), 4, "three case DMs and one appeal DM");
    assert!(
        sys.dms(MEMBER)
            .last()
            .is_some_and(|d| d.contains("accepted"))
    );

    // The log channel has an entry for each step; the appeal post shows
    // the decision.
    let entries = sys.posted(log);
    let expected = [
        "created the rule Spam",
        "added case #1",
        "added case #2",
        "timeout to end",
        "added case #3",
        "banned a member (ID 40) for case #3",
        "appealed case #3",
        "so case #3 is voided",
        "unbanned a member (ID 40) for case #3",
    ];
    let mut at = 0;
    for needle in expected {
        let found = entries[at..]
            .iter()
            .position(|e| e.contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} after entry {at}: {entries:#?}"));
        at += found + 1;
    }
    let post = sys.posted(appeals).join("\n");
    assert!(
        post.contains("Accepted") || post.contains("accepted"),
        "{post}"
    );
    let posts_pending = sys
        .db
        .count("SELECT count(*) FROM messages WHERE posted_revision < wanted_revision AND state = 'active'")
        .await?;
    assert_eq!(posts_pending, 0, "every tracked message is up to date");
    sys.assert_no_duplicate_effects();
    Ok(())
}
