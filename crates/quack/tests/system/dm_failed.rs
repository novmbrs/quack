//! A case DM Discord refuses (the member's DMs are closed): the case is
//! saved and the same sentence reaches the receipt, `/case view`, the log
//! channel, and the API.

use anyhow::{Context as _, Result};
use axum::http::{Method, StatusCode};
use quack_core::ids::ChannelId;
use quack_discord::custom_id::CustomId;
use quack_engine::port::RefusalKind;
use quack_testkit::fake::{Call, Script};
use quack_testkit::{ADMIN, MEMBER, sf};
use quack_text::actions::{SetupArea, SetupOp};
use serde_json::json;

use crate::common::{System, admin, case_add, click, ix, moderator, slash};

/// The words every surface uses for this failure.
const SAME: [&str; 2] = [
    "a DM about case #1. Discord didn't allow the DM.",
    "can sign in to the Quack dashboard to see the case.",
];

fn says_it(surface: &str, text: &str) {
    for part in SAME {
        assert!(text.contains(part), "{surface} lacks {part:?}: {text}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_dm_reads_the_same_everywhere() -> Result<()> {
    let mut sys = System::boot().await?;
    sys.install().await?;
    let log_create = CustomId::Setup(SetupArea::Log, SetupOp::Create).encode();
    sys.send(&click(admin(), sys.channel, &log_create, true))
        .await;
    let log: ChannelId = sf(u64::try_from(
        sys.db
            .count("SELECT log_channel_id FROM guild_settings")
            .await?,
    )?);
    let admin_session = sys.login(ADMIN).await?;
    let created = sys
        .http(
            Method::POST,
            "/api/guilds/1001/rules",
            Some(&admin_session),
            Some(json!({
                "name": "Spam", "description": "", "reason": "Please don't spam.",
                "appealable": true, "counting_window_days": null, "fields": [],
                "levels": [{"from_case_count": 1, "action": null, "send_dm": true}]
            })),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.json);
    let rule = created.json["id"].as_str().context("rule id")?.to_owned();

    // The member's DMs are closed.
    sys.fake
        .script(Call::OpenDm, Script::refuse(RefusalKind::CannotDm));
    sys.send(&ix(
        2,
        moderator(),
        sys.channel,
        &case_add(MEMBER, &rule, Some(true)),
    ))
    .await;
    // A restart before the DM job runs changes nothing.
    sys.restart().await?;
    sys.settle().await?;
    let state = sys
        .db
        .count("SELECT count(*) FROM dms WHERE state = 'failed' AND failure = 'dms_closed'")
        .await?;
    assert_eq!(state, 1, "the DM failed for a definite reason");
    assert_eq!(sys.dms(MEMBER).len(), 0);

    // The private receipt (the command's answer, kept current by its sync).
    let receipt = sys
        .fake
        .world(|w| w.interaction_edits.last().map(|(_, m)| m.clone()))
        .context("the receipt")?;
    let receipt = quack_text::render::plain_msg(&receipt);
    assert!(receipt.contains("Case #1"), "{receipt}");
    says_it("the receipt", &receipt);

    // /case view.
    sys.send(&ix(
        2,
        moderator(),
        sys.channel,
        &json!({"id": "100", "name": "case", "type": 1,
                "options": [{"name": "view", "type": 1,
                             "options": [{"name": "number", "type": 4, "value": 1}]}]}),
    ))
    .await;
    says_it("/case view", &quack_text::render::plain_msg(&sys.answer()));

    // The log channel.
    let entries = sys.posted(log).join("\n");
    says_it("the log channel", &entries);

    // The API.
    let moderator_session = sys.login(quack_testkit::MODERATOR).await?;
    let case = sys
        .http(
            Method::GET,
            "/api/guilds/1001/cases/1",
            Some(&moderator_session),
            None,
        )
        .await;
    assert_eq!(case.status, StatusCode::OK, "{}", case.json);
    let api = case.json["dm"]["outcome_text"]
        .as_str()
        .context("the DM's outcome")?;
    says_it("the API", api);

    // The setup panel and the slash command flow still answered.
    sys.send(&ix(2, admin(), sys.channel, &slash("help", &json!([]))))
        .await;
    sys.assert_no_duplicate_effects();
    Ok(())
}
