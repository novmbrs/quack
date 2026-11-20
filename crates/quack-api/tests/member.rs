//! Member routes (51–55): only the member's own cases, only the member
//! projection, and the appeal flow from the member's side, with staff
//! decisions in between.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::needless_pass_by_value,
    clippy::unused_self,
    reason = "test scenarios read top to bottom and fail loudly"
)]

mod common;

use std::collections::BTreeSet;

use axum::http::Method;
use axum::http::StatusCode;
use common::{App, GUILD, MEMBER, MEMBER_2, MODERATOR, evidence_channel, g, rules_for_tests};
use quack_core::ids::GuildId;
use serde_json::{Value, json};

/// Staff-only keys that must never reach a member.
const STAFF_ONLY: &[&str] = &[
    "moderator_id",
    "source",
    "attempt_log",
    "attempts",
    "http_status",
    "discord_code",
    "author_id",
    "url",
    "channel_id",
    "message_id",
    "decided_by",
    "actor_id",
    "requested_by",
    "dismissed_by",
    "added_by",
    "member_id",
    "rule_id",
    "fields",
    "timeline_staff",
    "description",
    "closed_by",
    "imported",
    // Evidence: staff notes, the messages around a saved message, removal,
    // and the staff-only switch never reach the member (§7.4).
    "note",
    "note_revision",
    "note_updated_by",
    "note_updated_at",
    "surrounding",
    "removed",
    "visible_to_member",
];

fn keys(value: &Value, out: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                out.insert(k.clone());
                keys(v, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|v| keys(v, out)),
        _ => {}
    }
}

fn assert_member_projection(value: &Value) {
    let mut found = BTreeSet::new();
    keys(value, &mut found);
    for key in STAFF_ONLY {
        assert!(
            !found.contains(*key),
            "member JSON has staff-only key {key}: {value}"
        );
    }
}

#[tokio::test]
async fn members_see_only_their_own_cases_in_the_member_projection() {
    let app = App::new().await;
    let rule = app.h.rule(rules_for_tests::warning("Spam")).await.unwrap();
    let mine = app.h.apply(MEMBER, rule).await.unwrap();
    let theirs = app.h.apply(MEMBER_2, rule).await.unwrap();
    let member = app.login(MEMBER, true).await;
    let list = app.send(app.get("/api/me/cases").by(&member)).await;
    assert_eq!(list.status, StatusCode::OK);
    let cases = list.json["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 1);
    assert_eq!(cases[0]["id"], mine.case.id.get().to_string());
    assert_eq!(
        cases[0]["guild"]["name"],
        format!("Server {}", common::GUILD)
    );
    assert_eq!(cases[0]["can_appeal"], true);
    assert_member_projection(&list.json);
    let detail = app
        .send(
            app.get(&format!("/api/me/cases/{}", mine.case.id.get()))
                .by(&member),
        )
        .await;
    assert_eq!(detail.status, StatusCode::OK, "{}", detail.json);
    assert_eq!(detail.json["number"], 1);
    assert!(detail.json["dm_text"].is_string());
    assert_member_projection(&detail.json);
    // Someone else's case, a number, or garbage: all not found.
    for path in [
        format!("/api/me/cases/{}", theirs.case.id.get()),
        "/api/me/cases/1".to_owned(),
        "/api/me/cases/garbage".to_owned(),
    ] {
        let resp = app.send(app.get(&path).by(&member)).await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND, "{path}");
        assert_eq!(resp.code(), Some("not_found"));
    }
    let file = app
        .send(
            app.get(&format!(
                "/api/me/cases/{}/evidence/0190f5b2-6c1e-7cc0-9a3e-000000000001",
                theirs.case.id.get()
            ))
            .by(&member),
        )
        .await;
    assert_eq!(file.code(), Some("not_found"));
    // Filter by server, and page.
    let other = app
        .send(
            app.get(&format!("/api/me/cases?guild_id={}", common::OTHER_GUILD))
                .by(&member),
        )
        .await;
    assert_eq!(other.json["cases"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn appeal_flow_from_both_sides() {
    let app = App::new().await;
    let rule = app.h.rule(rules_for_tests::warning("Spam")).await.unwrap();
    let case = app.h.apply(MEMBER, rule).await.unwrap();
    let case_id = case.case.id.get().to_string();
    let member = app.login(MEMBER, true).await;
    let other = app.login(MEMBER_2, true).await;
    let moderator = app.login(MODERATOR, true).await;
    // Someone else can't appeal it.
    let stranger = app
        .send(
            app.post(&format!("/api/me/cases/{case_id}/appeal"))
                .by(&other)
                .json(json!({"statement": "Not me."})),
        )
        .await;
    assert_eq!(stranger.code(), Some("not_found"));
    let empty = app
        .send(
            app.post(&format!("/api/me/cases/{case_id}/appeal"))
                .by(&member)
                .json(json!({"statement": " "})),
        )
        .await;
    assert_eq!(empty.json["error"]["fields"][0]["field"], "statement");
    let submitted = app
        .send(
            app.post(&format!("/api/me/cases/{case_id}/appeal"))
                .by(&member)
                .key("appeal-key-1")
                .json(json!({"statement": "I was quoting someone."})),
        )
        .await;
    assert_eq!(submitted.status, StatusCode::CREATED, "{}", submitted.json);
    assert_eq!(submitted.json["appeal"]["state"], "pending");
    assert_member_projection(&submitted.json);
    let again = app
        .send(
            app.post(&format!("/api/me/cases/{case_id}/appeal"))
                .by(&member)
                .json(json!({"statement": "Again."})),
        )
        .await;
    assert_eq!(again.code(), Some("appeal_exists"));
    // Staff list, read, and ask for information.
    let list = app
        .send(app.get(&g("/appeals?state=pending")).by(&moderator))
        .await;
    let appeal_id = list.json["appeals"][0]["id"].as_str().unwrap().to_owned();
    let version = list.json["appeals"][0]["version"].as_u64().unwrap();
    let detail = app
        .send(app.get(&g(&format!("/appeals/{appeal_id}"))).by(&moderator))
        .await;
    assert_eq!(detail.json["statement"], "I was quoting someone.");
    assert_eq!(detail.json["case"]["number"], 1);
    let asked = app
        .send(app.post(&g(&format!("/appeals/{appeal_id}/decision"))).by(&moderator).json(json!({
            "decision": "request_information", "reason": "Which message?", "expected_version": version
        })))
        .await;
    assert_eq!(asked.status, StatusCode::OK, "{}", asked.json);
    assert_eq!(asked.json["state"], "needs_information");
    let stale = app
        .send(
            app.post(&g(&format!("/appeals/{appeal_id}/decision")))
                .by(&moderator)
                .json(json!({
                    "decision": "reject", "reason": "No.", "expected_version": version
                })),
        )
        .await;
    assert_eq!(stale.code(), Some("appeal_changed"));
    // The member replies with the version they saw.
    let seen = app
        .send(app.get(&format!("/api/me/cases/{case_id}")).by(&member))
        .await;
    let member_version = seen.json["appeal"]["version"].as_u64().unwrap();
    assert_eq!(seen.json["appeal"]["can_reply"], true);
    let outdated = app
        .send(
            app.post(&format!("/api/me/cases/{case_id}/appeal/information"))
                .by(&member)
                .json(json!({"text": "This one.", "expected_version": member_version + 5})),
        )
        .await;
    assert_eq!(outdated.code(), Some("appeal_changed"));
    let replied = app
        .send(
            app.post(&format!("/api/me/cases/{case_id}/appeal/information"))
                .by(&member)
                .json(json!({"text": "This one.", "expected_version": member_version})),
        )
        .await;
    assert_eq!(replied.status, StatusCode::OK, "{}", replied.json);
    assert_member_projection(&replied.json);
    let events = replied.json["appeal"]["events"].as_array().unwrap();
    assert!(events.iter().any(|e| e["by_you"] == true));
    // Accepting voids the case; the member sees it.
    let current = app
        .send(app.get(&g(&format!("/appeals/{appeal_id}"))).by(&moderator))
        .await;
    let accepted = app
        .send(
            app.post(&g(&format!("/appeals/{appeal_id}/decision")))
                .by(&moderator)
                .json(json!({
                    "decision": "accept", "reason": "Fair enough.",
                    "expected_version": current.json["version"]
                })),
        )
        .await;
    assert_eq!(accepted.json["state"], "accepted", "{}", accepted.json);
    let after = app
        .send(app.get(&format!("/api/me/cases/{case_id}")).by(&member))
        .await;
    assert!(after.json["voided_at"].is_string());
    assert_member_projection(&after.json);
    let events = after.json["appeal"]["events"].as_array().unwrap();
    assert!(events.iter().all(|e| e["closed_by_void"] == false));
}

/// Dashboard review C1: a member's appeal closed because staff voided the
/// case says so, instead of the page guessing from an empty text.
#[tokio::test]
async fn an_appeal_closed_by_a_void_says_so() {
    let app = App::new().await;
    let rule = app.h.rule(rules_for_tests::warning("Spam")).await.unwrap();
    let case = app.h.apply(MEMBER, rule).await.unwrap();
    let case_id = case.case.id.get().to_string();
    let member = app.login(MEMBER, true).await;
    let moderator = app.login(MODERATOR, true).await;
    let submitted = app
        .send(
            app.post(&format!("/api/me/cases/{case_id}/appeal"))
                .by(&member)
                .json(json!({"statement": "Not me."})),
        )
        .await;
    assert_eq!(submitted.status, StatusCode::CREATED, "{}", submitted.json);
    let void = app
        .send(
            app.post(&g("/cases/1/void"))
                .by(&moderator)
                .json(json!({"reason": "Wrong member."})),
        )
        .await;
    assert_eq!(void.status, StatusCode::OK, "{}", void.json);
    let after = app
        .send(app.get(&format!("/api/me/cases/{case_id}")).by(&member))
        .await;
    let events = after.json["appeal"]["events"].as_array().unwrap();
    let closed = events.iter().find(|e| e["kind"] == "closed").unwrap();
    assert_eq!(closed["closed_by_void"], true, "{}", after.json);
}

/// The member's own case page shows only what §7.4 lets them see: saved
/// messages without their surroundings, uploads only once staff show them,
/// never a note or a removed item, and no file of an item they can't see.
/// A leak here could name a reporter, so the raw body is checked too.
#[tokio::test]
async fn members_never_receive_staff_only_evidence() {
    let app = App::new().await;
    let rule = app.h.rule(rules_for_tests::warning("Spam")).await.unwrap();
    let case = app.h.apply(MEMBER, rule).await.unwrap();
    let case_id = case.case.id.get().to_string();
    evidence_channel(&app).await;
    let guild: GuildId = common::sf(GUILD);
    let channel = app.h.fake.world(|w| w.add_text_channel(guild, false));
    let post = |text: &str| {
        app.h
            .fake
            .world(|w| w.post(guild, channel, common::sf(MEMBER_2), text, vec![]))
    };
    post("REPORTER-CONTEXT-BEFORE");
    let target = post("the saved message");
    post("REPORTER-CONTEXT-AFTER");
    let removed_target = post("a message added by mistake");
    let moderator = app.login(MODERATOR, true).await;
    let link = |m| format!("https://discord.com/channels/{GUILD}/{channel}/{m}");
    let linked = app
        .send(
            app.post(&g("/cases/1/evidence"))
                .by(&moderator)
                .json(json!({
                    "links": [link(target), link(removed_target)],
                    "note": "STAFF-NOTE-ON-MESSAGE reported by a member"
                })),
        )
        .await;
    assert_eq!(linked.status, StatusCode::OK, "{}", linked.json);
    let uploaded = app
        .send(
            app.post(&g("/cases/1/evidence/uploads"))
                .by(&moderator)
                .multipart(&[
                    (
                        "files",
                        Some("STAFF-ONLY-FILE.png"),
                        "image/png",
                        b"png".as_slice(),
                    ),
                    (
                        "note",
                        None,
                        "text/plain",
                        b"STAFF-NOTE-ON-UPLOAD".as_slice(),
                    ),
                ]),
        )
        .await;
    assert_eq!(uploaded.status, StatusCode::OK, "{}", uploaded.json);
    app.h.settle().await.unwrap();
    let staff = app.send(app.get(&g("/cases/1")).by(&moderator)).await;
    let items = staff.json["evidence"].as_array().unwrap().clone();
    let find = |pred: &dyn Fn(&Value) -> bool| items.iter().find(|e| pred(e)).unwrap().clone();
    let upload = find(&|e| e["kind"] == "upload");
    let mistake = find(&|e| e["message_id"] == removed_target.to_string());
    let saved = find(&|e| e["message_id"] == target.to_string());
    assert!(saved["surrounding"].is_array(), "{saved}");
    let upload_id = upload["id"].as_str().unwrap();
    let upload_file = upload["files"][0]["id"].as_str().unwrap().to_owned();
    let removed = app
        .send(
            app.post(&g(&format!(
                "/cases/1/evidence/{}/remove",
                mistake["id"].as_str().unwrap()
            )))
            .by(&moderator),
        )
        .await;
    assert_eq!(removed.status, StatusCode::OK, "{}", removed.json);

    let member = app.login(MEMBER, true).await;
    let path = format!("/api/me/cases/{case_id}");
    let file_route = |id: &str| format!("/api/me/cases/{case_id}/evidence/{id}");
    let assert_hidden = |resp: &common::Resp| {
        assert_member_projection(&resp.json);
        let raw = String::from_utf8_lossy(&resp.body);
        for secret in [
            "STAFF-NOTE",
            "REPORTER-CONTEXT",
            "STAFF-ONLY-FILE",
            "added by mistake",
            upload_id,
        ] {
            assert!(!raw.contains(secret), "member body has {secret}: {raw}");
        }
    };

    // Staff-only upload: not listed, and its file is not found.
    let seen = app.send(app.get(&path).by(&member)).await;
    assert_eq!(seen.status, StatusCode::OK, "{}", seen.json);
    assert_hidden(&seen);
    let evidence = seen.json["evidence"].as_array().unwrap();
    assert_eq!(evidence.len(), 1, "only the saved message: {}", seen.json);
    assert_eq!(evidence[0]["content"], "the saved message");
    let hidden_file = app
        .send(app.get(&file_route(&upload_file)).by(&member))
        .await;
    assert_eq!(hidden_file.status, StatusCode::NOT_FOUND);
    assert_eq!(hidden_file.code(), Some("not_found"));
    // The removed message's file route answers the same way as any other
    // item the member can't see (it has no files here; an unknown id is
    // also not found).
    let other_file = app
        .send(
            app.get(&file_route("0190f5b2-6c1e-7cc0-9a3e-000000000001"))
                .by(&member),
        )
        .await;
    assert_eq!(other_file.code(), Some("not_found"));

    // Once staff show the upload, its files are listed (never its note).
    let shown = app
        .send(
            app.req(
                Method::PUT,
                &g(&format!("/cases/1/evidence/{upload_id}/visibility")),
            )
            .by(&moderator)
            .json(json!({"visible_to_member": true})),
        )
        .await;
    assert_eq!(shown.status, StatusCode::OK, "{}", shown.json);
    let seen = app.send(app.get(&path).by(&member)).await;
    assert_member_projection(&seen.json);
    let raw = String::from_utf8_lossy(&seen.body);
    assert!(!raw.contains("STAFF-NOTE"), "{raw}");
    assert!(!raw.contains("REPORTER-CONTEXT"), "{raw}");
    let evidence = seen.json["evidence"].as_array().unwrap();
    assert_eq!(evidence.len(), 2, "{}", seen.json);
    let files: Vec<&str> = evidence
        .iter()
        .flat_map(|e| e["files"].as_array().unwrap())
        .map(|f| f["id"].as_str().unwrap())
        .collect();
    assert_eq!(files, [upload_file.as_str()]);
    let served = app
        .send(app.get(&file_route(&upload_file)).by(&member))
        .await;
    assert_eq!(served.status, StatusCode::FOUND, "{}", served.json);
    // Another member never sees it.
    let stranger = app.login(MEMBER_2, true).await;
    let theirs = app
        .send(app.get(&file_route(&upload_file)).by(&stranger))
        .await;
    assert_eq!(theirs.code(), Some("not_found"));

    // Hidden again, and then removed: gone from the member's page.
    let hidden = app
        .send(
            app.req(
                Method::PUT,
                &g(&format!("/cases/1/evidence/{upload_id}/visibility")),
            )
            .by(&moderator)
            .json(json!({"visible_to_member": false})),
        )
        .await;
    assert_eq!(hidden.status, StatusCode::OK);
    assert_hidden(&app.send(app.get(&path).by(&member)).await);
    let shown_again = app
        .send(
            app.req(
                Method::PUT,
                &g(&format!("/cases/1/evidence/{upload_id}/visibility")),
            )
            .by(&moderator)
            .json(json!({"visible_to_member": true})),
        )
        .await;
    assert_eq!(shown_again.status, StatusCode::OK);
    let removed = app
        .send(
            app.post(&g(&format!("/cases/1/evidence/{upload_id}/remove")))
                .by(&moderator),
        )
        .await;
    assert_eq!(removed.status, StatusCode::OK, "{}", removed.json);
    assert_hidden(&app.send(app.get(&path).by(&member)).await);
    let removed_file = app
        .send(app.get(&file_route(&upload_file)).by(&member))
        .await;
    assert_eq!(removed_file.code(), Some("not_found"));
}
