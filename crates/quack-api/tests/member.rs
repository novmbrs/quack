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

use axum::http::StatusCode;
use common::{App, MEMBER, MEMBER_2, MODERATOR, g, rules_for_tests};
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
}
