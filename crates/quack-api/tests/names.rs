//! Server-worded sentences name people (§12.0): when Discord knows the
//! accounts, no dashboard sentence falls back to "a member (ID …)", and the
//! member's own pages still never name staff.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::too_many_lines,
    reason = "test scenarios read top to bottom and fail loudly"
)]

mod common;

use axum::http::StatusCode;
use common::{App, MEMBER, MEMBER_2, MODERATOR, g, rules_for_tests};
use quack_core::rule::LevelAction;
use quack_engine::port::{Refusal, RefusalKind};
use quack_testkit::fake::{Call, Script};
use serde_json::{Value, json};

/// Every string in `value` with the path that leads to it.
fn strings<'a>(value: &'a Value, path: &str, out: &mut Vec<(String, &'a str)>) {
    match value {
        Value::String(s) => out.push((path.to_owned(), s)),
        Value::Array(items) => {
            for (i, v) in items.iter().enumerate() {
                strings(v, &format!("{path}[{i}]"), out);
            }
        }
        Value::Object(map) => {
            for (k, v) in map {
                strings(v, &format!("{path}.{k}"), out);
            }
        }
        _ => {}
    }
}

/// Whether `s` holds the unknown-name wording "ID " plus digits.
fn has_id_wording(s: &str) -> bool {
    s.match_indices("ID ").any(|(i, _)| {
        s[i + 3..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit())
    })
}

fn assert_named(label: &str, body: &Value) {
    let mut found = Vec::new();
    strings(body, "", &mut found);
    for (path, s) in found {
        assert!(
            !has_id_wording(s),
            "{label}{path} names someone by id: {s:?}\n{body}"
        );
    }
}

fn all_text(body: &Value) -> String {
    let mut found = Vec::new();
    strings(body, "", &mut found);
    found
        .into_iter()
        .map(|(_, s)| s)
        .collect::<Vec<_>>()
        .join("\n")
}

const MEMBER_NAME: &str = "user40";
const MODERATOR_NAME: &str = "user30";

#[tokio::test]
async fn dashboard_sentences_name_known_people() {
    let app = App::new().await;
    let ban = app
        .h
        .rule(rules_for_tests::one(
            "Rehearsal",
            rules_for_tests::BAN,
            true,
        ))
        .await
        .unwrap();
    let case = app.h.apply(MEMBER, ban).await.unwrap();
    app.h.settle().await.unwrap();
    // A failed timeout for the failed-actions page.
    let timeout = app
        .h
        .rule(rules_for_tests::one(
            "Timeout",
            LevelAction::Timeout {
                duration_seconds: 600,
            },
            true,
        ))
        .await
        .unwrap();
    app.h.fake.script(
        Call::Timeout,
        Script::Refuse(Refusal::of(RefusalKind::MissingPermissions)),
    );
    app.h.apply(MEMBER_2, timeout).await.unwrap();
    app.h.settle().await.unwrap();
    let member = app.login(MEMBER, true).await;
    let moderator = app.login(MODERATOR, true).await;
    let case_id = case.case.id.get().to_string();
    // The member appeals; staff ask for more.
    let appealed = app
        .send(
            app.post(&format!("/api/me/cases/{case_id}/appeal"))
                .by(&member)
                .json(json!({"statement": "Please."})),
        )
        .await;
    assert_eq!(appealed.status, StatusCode::CREATED, "{}", appealed.json);
    let appeals = app.send(app.get(&g("/appeals")).by(&moderator)).await;
    let appeal_id = appeals.json["appeals"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let asked = app
        .send(
            app.post(&g(&format!("/appeals/{appeal_id}/decision")))
                .by(&moderator)
                .json(json!({
                    "decision": "request_information", "reason": "Which message?",
                    "expected_version": appeals.json["appeals"][0]["version"]
                })),
        )
        .await;
    assert_eq!(asked.status, StatusCode::OK, "{}", asked.json);
    // Staff void the case; the ban is undone.
    let void = app
        .send(
            app.post(&g("/cases/1/void"))
                .by(&moderator)
                .json(json!({"reason": "Wrong member."})),
        )
        .await;
    assert_eq!(void.status, StatusCode::OK, "{}", void.json);
    assert_named("void", &void.json);
    assert!(
        void.json["message"].as_str().unwrap().contains(MEMBER_NAME),
        "{}",
        void.json
    );
    app.h.settle().await.unwrap();

    let detail = app.send(app.get(&g("/cases/1")).by(&moderator)).await;
    assert_eq!(detail.status, StatusCode::OK, "{}", detail.json);
    assert_named("case", &detail.json);
    let text = all_text(&detail.json);
    assert!(
        text.contains(&format!("Quack banned {MEMBER_NAME}")),
        "{text}"
    );
    assert!(
        text.contains(&format!("Quack unbanned {MEMBER_NAME}")),
        "{text}"
    );
    assert!(
        text.contains(&format!("{MODERATOR_NAME} added case #1 for {MEMBER_NAME}")),
        "{text}"
    );
    assert!(text.contains(&format!("sent {MEMBER_NAME} a DM")), "{text}");

    let checks = [
        ("case list", g("/cases")),
        ("member summary", g(&format!("/members/{MEMBER}/summary"))),
        ("moderation log", g("/mod-log")),
        ("appeals", g("/appeals")),
        ("appeal", g(&format!("/appeals/{appeal_id}"))),
        ("failed actions", g("/actions")),
        ("done actions", g("/actions?state=done")),
    ];
    for (label, path) in checks {
        let resp = app.send(app.get(&path).by(&moderator)).await;
        assert_eq!(resp.status, StatusCode::OK, "{label}: {}", resp.json);
        assert_named(label, &resp.json);
    }
    let log = app.send(app.get(&g("/mod-log")).by(&moderator)).await;
    assert!(
        all_text(&log.json).contains(&format!("{MODERATOR_NAME} voided case #1")),
        "{}",
        log.json
    );
    let failed = app.send(app.get(&g("/actions")).by(&moderator)).await;
    assert!(all_text(&failed.json).contains("user41"), "{}", failed.json);

    // The member's own pages: "you", never a staff name.
    for path in [
        "/api/me/cases".to_owned(),
        format!("/api/me/cases/{case_id}"),
    ] {
        let resp = app.send(app.get(&path).by(&member)).await;
        assert_eq!(resp.status, StatusCode::OK, "{path}: {}", resp.json);
        assert_named(&path, &resp.json);
        let text = all_text(&resp.json);
        assert!(!text.contains(MODERATOR_NAME), "{path} names staff: {text}");
    }
}
