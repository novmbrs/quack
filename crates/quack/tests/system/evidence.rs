//! Evidence end to end: `/setup` creates the evidence channel, `/case add`
//! uploads a file with the case, the capture job copies it there, the
//! confirmation offers Add evidence and View evidence, and the member sees
//! the file on their case page only once staff select Show to member.

use anyhow::{Context as _, Result};
use axum::http::{Method, StatusCode};
use quack_core::ids::{CaseId, ChannelId, EvidenceId};
use quack_discord::custom_id::CustomId;
use quack_testkit::{ADMIN, MEMBER, sf};
use quack_text::actions::{SetupArea, SetupOp};
use serde_json::json;

use crate::common::{System, admin, case_add, click, ix, moderator, slash};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
pub(crate) async fn a_file_added_with_a_case_is_copied_and_shown_to_the_member_on_request()
-> Result<()> {
    let sys = System::boot().await?;
    sys.install().await?;
    sys.send(&ix(2, admin(), sys.channel, &slash("setup", &json!([]))))
        .await;
    let create = CustomId::Setup(SetupArea::Evidence, SetupOp::Create).encode();
    sys.send(&click(admin(), sys.channel, &create, true)).await;
    let evidence_channel: ChannelId = sf(u64::try_from(
        sys.db
            .count("SELECT evidence_channel_id FROM guild_settings")
            .await?,
    )?);

    let manager = sys.login(ADMIN).await?;
    let created = sys
        .http(
            Method::POST,
            "/api/guilds/1001/rules",
            Some(&manager),
            Some(json!({
                "name": "Spam", "description": "Unwanted ads.", "reason": "Please don't spam.",
                "appealable": true, "counting_window_days": null, "fields": [],
                "levels": [{"from_case_count": 1, "action": null, "send_dm": false}]
            })),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.json);
    let rule = created.json["id"].as_str().context("rule id")?.to_owned();

    // /case add with a file: an interaction attachment on the fake CDN.
    let url = "https://cdn.discordapp.com/ephemeral-attachments/1/7001/proof.png";
    sys.fake
        .world(|w| w.files.insert(url.to_owned(), vec![137, 80, 78, 71]));
    let mut add = case_add(MEMBER, &rule, None);
    add["options"][0]["options"]
        .as_array_mut()
        .context("options")?
        .push(json!({"name": "file", "type": 11, "value": "7001"}));
    add["resolved"]["attachments"] = json!({"7001": {
        "id": "7001", "filename": "proof.png", "content_type": "image/png",
        "size": 4, "url": url, "proxy_url": url, "ephemeral": true
    }});
    sys.send(&ix(2, moderator(), sys.channel, &add)).await;
    sys.settle().await?;

    let copied = sys.fake.world(|w| {
        w.messages
            .values()
            .filter(|m| m.fetched.channel_id == evidence_channel)
            .flat_map(|m| m.fetched.attachments.iter())
            .filter(|a| a.filename == "SPOILER_proof.png")
            .count()
    });
    assert_eq!(
        copied, 1,
        "the file was copied, spoilered, to the evidence channel"
    );
    let confirmation = sys
        .fake
        .world(|w| w.interaction_edits.last().map(|(_, m)| m.clone()))
        .context("the confirmation")?;
    let labels: Vec<String> = confirmation
        .rows
        .iter()
        .flat_map(|r| match r {
            quack_text::model::Row::Buttons(b) => b.iter().map(|b| b.label.clone()).collect(),
            quack_text::model::Row::Select(_) => Vec::new(),
        })
        .collect();
    assert_eq!(labels, ["Add evidence", "View evidence", "Open case"]);

    // The member doesn't see the upload until staff show it.
    let (case, upload): (uuid::Uuid, uuid::Uuid) = {
        let mut conn = sys.db.db.conn().await?;
        sqlx::query_as("SELECT case_id, id FROM evidence WHERE kind = 'upload'")
            .fetch_one(&mut *conn)
            .await?
    };
    let (case, upload) = (CaseId::from_uuid(case), EvidenceId::from_uuid(upload));
    let member = sys.login(MEMBER).await?;
    let path = format!("/api/me/cases/{}", case.get());
    let before = sys.http(Method::GET, &path, Some(&member), None).await;
    assert_eq!(before.status, StatusCode::OK, "{}", before.json);
    assert!(
        !before.json.to_string().contains("proof.png"),
        "{}",
        before.json
    );

    let show = CustomId::EvidenceVisibility {
        evidence: upload,
        visible: true,
    }
    .encode();
    sys.send(&click(moderator(), sys.channel, &show, true))
        .await;
    let shown = quack_text::render::plain_msg(&sys.answer());
    assert!(
        shown.contains("The member can now see these files"),
        "{shown}"
    );
    let after = sys.http(Method::GET, &path, Some(&member), None).await;
    assert_eq!(after.status, StatusCode::OK, "{}", after.json);
    assert!(
        after.json.to_string().contains("proof.png"),
        "{}",
        after.json
    );
    sys.assert_no_duplicate_effects();
    Ok(())
}
