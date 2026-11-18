//! Actions, rules, settings, health, statistics, the directory, and the
//! moderation log: happy paths and their refusals.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::needless_pass_by_value,
    clippy::unused_self,
    reason = "test scenarios read top to bottom and fail loudly"
)]

mod common;

use axum::http::{Method, StatusCode};
use common::{ADMIN, App, MEMBER, MEMBER_2, MODERATOR, OWNER, g, rules_for_tests};
use quack_core::rule::LevelAction;
use quack_engine::port::{Refusal, RefusalKind};
use quack_testkit::fake::{Call, Script};
use serde_json::json;

fn timeout_rule() -> quack_core::rule::RuleDefinition {
    rules_for_tests::one(
        "Timeout",
        LevelAction::Timeout {
            duration_seconds: 600,
        },
        false,
    )
}

#[tokio::test]
async fn failed_actions_list_retry_dismiss_and_undo() {
    let app = App::new().await;
    let rule = app.h.rule(timeout_rule()).await.unwrap();
    app.h.fake.script(
        Call::Timeout,
        Script::Refuse(Refusal::of(RefusalKind::MissingPermissions)),
    );
    let failed = app.h.apply(MEMBER, rule).await.unwrap();
    let done = app.h.apply(MEMBER_2, rule).await.unwrap();
    app.h.settle().await.unwrap();
    let moderator = app.login(MODERATOR, true).await;
    let list = app.send(app.get(&g("/actions")).by(&moderator)).await;
    assert_eq!(list.status, StatusCode::OK, "{}", list.json);
    let actions = list.json["actions"].as_array().unwrap();
    assert_eq!(actions.len(), 1);
    let failed_id = failed.action.unwrap().get().to_string();
    assert_eq!(actions[0]["id"], failed_id);
    assert_eq!(actions[0]["state"], "failed");
    assert_eq!(actions[0]["member_id"], MEMBER.to_string());
    assert_eq!(actions[0]["rule_name"], "Timeout");
    assert_eq!(actions[0]["can_retry"], true);
    assert_eq!(actions[0]["purpose"], "original");
    // Retry twice with one key: one new round.
    let retry = || {
        app.post(&g(&format!("/actions/{failed_id}/retry")))
            .by(&moderator)
            .key("retry-key-01")
    };
    let first = app.send(retry()).await;
    assert_eq!(first.status, StatusCode::OK, "{}", first.json);
    assert_eq!(first.json["state"], "pending");
    let second = app.send(retry()).await;
    assert_eq!(second.status, StatusCode::OK);
    assert_eq!(second.json["id"], first.json["id"]);
    let rounds = app
        .h
        .db
        .count("SELECT count(*) FROM mod_log WHERE kind = 'action_retry_requested'")
        .await
        .unwrap();
    assert_eq!(rounds, 1);
    // Retrying a pending action is refused with the action named.
    let again = app
        .send(
            app.post(&g(&format!("/actions/{failed_id}/retry")))
                .by(&moderator),
        )
        .await;
    assert_eq!(again.code(), Some("action_not_retryable"));
    assert!(
        again.json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("timeout"),
        "{}",
        again.json
    );
    // Undo the done timeout.
    let done_id = done.action.unwrap().get().to_string();
    let undo = app
        .send(
            app.post(&g(&format!("/actions/{done_id}/undo")))
                .by(&moderator),
        )
        .await;
    assert_eq!(undo.status, StatusCode::CREATED, "{}", undo.json);
    assert_eq!(undo.json["kind"], "remove_timeout");
    assert_eq!(undo.json["purpose"], "undo");
    assert_eq!(undo.json["undo_of_action_id"], done_id);
    let dismiss = app
        .send(
            app.post(&g(&format!("/actions/{done_id}/dismiss")))
                .by(&moderator),
        )
        .await;
    assert_eq!(dismiss.code(), Some("action_not_dismissable"));
    let unknown = app
        .send(
            app.post(&g("/actions/0190f5b2-6c1e-7cc0-9a3e-000000000001/retry"))
                .by(&moderator),
        )
        .await;
    assert_eq!(unknown.code(), Some("action_not_found"));
    let paged = app
        .send(app.get(&g("/actions?state=done&limit=1")).by(&moderator))
        .await;
    assert_eq!(paged.json["actions"].as_array().unwrap().len(), 1);
    // The only done action fills the page exactly: no cursor to an empty page.
    assert!(paged.json["next_cursor"].is_null());
    let bad = app
        .send(app.get(&g("/actions?state=exploded")).by(&moderator))
        .await;
    assert_eq!(bad.code(), Some("validation_failed"));
}

fn definition(name: &str) -> serde_json::Value {
    json!({
        "name": name, "description": "", "reason": "Please be kind.", "appealable": true,
        "counting_window_days": 30,
        "fields": [{"key": "where", "label": "Where", "kind": "short_text", "required": false}],
        "levels": [
            {"from_case_count": 1, "action": null, "send_dm": true},
            {"from_case_count": 3, "action": {"kind": "timeout", "duration_seconds": 3600}, "send_dm": true}
        ]
    })
}

#[tokio::test]
async fn rules_create_edit_archive_restore_export_import() {
    let app = App::new().await;
    let manager = app.login(common::RULES_MANAGER, true).await;
    let created = app
        .send(app.post(&g("/rules")).by(&manager).json(definition("Spam")))
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.json);
    assert_eq!(created.json["current_version"], 1);
    assert_eq!(
        created.json["definition"]["levels"][1]["action"]["kind"],
        "timeout"
    );
    let id = created.json["id"].as_str().unwrap().to_owned();
    let taken = app
        .send(app.post(&g("/rules")).by(&manager).json(definition("Spam")))
        .await;
    assert_eq!(taken.code(), Some("rule_name_taken"));
    assert!(
        taken.json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Spam")
    );
    let mut invalid = definition("");
    invalid["levels"][1]["from_case_count"] = json!(1);
    let invalid = app
        .send(app.post(&g("/rules")).by(&manager).json(invalid))
        .await;
    assert_eq!(invalid.code(), Some("validation_failed"));
    let fields: Vec<&str> = invalid.json["error"]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["field"].as_str())
        .collect();
    assert!(
        fields.contains(&"name") && fields.contains(&"levels[1].from_case_count"),
        "{fields:?}"
    );
    let edited = app
        .send(
            app.req(Method::PUT, &g(&format!("/rules/{id}")))
                .by(&manager)
                .json(json!({"expected_version": 1, "definition": definition("Spam and ads")})),
        )
        .await;
    assert_eq!(edited.json["current_version"], 2, "{}", edited.json);
    assert_eq!(edited.json["versions"].as_array().unwrap().len(), 2);
    let stale = app
        .send(
            app.req(Method::PUT, &g(&format!("/rules/{id}")))
                .by(&manager)
                .json(json!({"expected_version": 1, "definition": definition("Spam")})),
        )
        .await;
    assert_eq!(stale.code(), Some("rule_changed"));
    let v1 = app
        .send(app.get(&g(&format!("/rules/{id}/versions/1"))).by(&manager))
        .await;
    assert_eq!(v1.json["definition"]["name"], "Spam");
    let archived = app
        .send(app.post(&g(&format!("/rules/{id}/archive"))).by(&manager))
        .await;
    assert!(
        archived.json["archived_at"].is_string(),
        "{}",
        archived.json
    );
    let list = app
        .send(app.get(&g("/rules?archived=true")).by(&manager))
        .await;
    assert_eq!(list.json["rules"][0]["name"], "Spam and ads");
    assert_eq!(list.json["rules"][0]["field_count"], 1);
    // Dashboard review C3: the list has what the rules page shows.
    assert!(list.json["rules"][0]["appealable"].is_boolean());
    assert!(list.json["rules"][0].get("counting_window_days").is_some());
    assert!(
        list.json["rules"][0]["levels"][0]["from_case_count"].is_number(),
        "{}",
        list.json
    );
    let restored = app
        .send(app.post(&g(&format!("/rules/{id}/restore"))).by(&manager))
        .await;
    assert!(restored.json["archived_at"].is_null());
    let export = app
        .send(app.get(&g(&format!("/rules/{id}/export"))).by(&manager))
        .await;
    assert_eq!(export.json["format"], "quack-rule");
    let not_confirmed = app
        .send(
            app.post(&g("/rules/import"))
                .by(&manager)
                .json(json!({"confirm": false, "file": export.json})),
        )
        .await;
    assert_eq!(not_confirmed.json["error"]["fields"][0]["field"], "confirm");
    let imported = app
        .send(app.post(&g("/rules/import")).by(&manager).json(json!({
            "confirm": true, "file": export.json, "name": "Imported spam"
        })))
        .await;
    assert_eq!(imported.status, StatusCode::CREATED, "{}", imported.json);
    assert_eq!(imported.json["definition"]["name"], "Imported spam");
    let mut wrong = export.json.clone();
    wrong["format"] = json!("other");
    let wrong = app
        .send(
            app.post(&g("/rules/import"))
                .by(&manager)
                .json(json!({"confirm": true, "file": wrong})),
        )
        .await;
    assert_eq!(wrong.json["error"]["fields"][0]["field"], "file.format");
    let missing = app
        .send(
            app.get(&g("/rules/0190f5b2-6c1e-7cc0-9a3e-000000000001"))
                .by(&manager),
        )
        .await;
    assert_eq!(missing.code(), Some("not_found"));
}

#[tokio::test]
async fn settings_health_statistics_directory_and_mod_log() {
    let app = App::new().await;
    let admin = app.login(ADMIN, true).await;
    let settings = app.send(app.get(&g("/settings")).by(&admin)).await;
    assert_eq!(settings.status, StatusCode::OK, "{}", settings.json);
    assert_eq!(
        settings.json["rules_manager_role_ids"],
        json!([common::RULES_ROLE.to_string()])
    );
    assert_eq!(settings.json["modules"]["tickets"]["enabled"], false);
    let patched = app
        .send(
            app.req(Method::PATCH, &g("/settings"))
                .by(&admin)
                .json(json!({
                    "dm_footer": "Questions? Ask a moderator.", "log_channel_id": null
                })),
        )
        .await;
    assert_eq!(patched.status, StatusCode::OK, "{}", patched.json);
    assert_eq!(patched.json["dm_footer"], "Questions? Ask a moderator.");
    let bad = app
        .send(
            app.req(Method::PATCH, &g("/settings"))
                .by(&admin)
                .json(json!({
                    "moderator_role_ids": ["1", "0"], "expected_version": 1
                })),
        )
        .await;
    assert_eq!(
        bad.json["error"]["fields"][0]["field"],
        "moderator_role_ids[1]"
    );
    let invite = app
        .send(
            app.req(Method::PATCH, &g("/settings"))
                .by(&admin)
                .json(json!({
                    "appeal_invite_url": "https://example.com/not-an-invite"
                })),
        )
        .await;
    assert_eq!(
        invite.json["error"]["fields"][0]["code"], "invite_url",
        "{}",
        invite.json
    );
    let ack = app
        .send(
            app.post(&g("/settings/starter-notice/acknowledge"))
                .by(&admin),
        )
        .await;
    assert_eq!(ack.status, StatusCode::OK);
    let health = app.send(app.get(&g("/health")).by(&admin)).await;
    assert_eq!(health.json["quack_in_server"], true, "{}", health.json);
    assert_eq!(health.json["missing_permissions"], json!([]));
    let roles = app.send(app.get(&g("/directory/roles")).by(&admin)).await;
    assert_eq!(roles.json["roles"][0]["name"], "Rules");
    let channels = app
        .send(app.get(&g("/directory/channels")).by(&admin))
        .await;
    assert!(channels.json["channels"].is_array());
    let moderator = app.login(MODERATOR, true).await;
    let search = app
        .send(app.get(&g("/directory/members?query=user4")).by(&moderator))
        .await;
    assert_eq!(search.status, StatusCode::OK, "{}", search.json);
    let ids = app
        .send(
            app.get(&g(&format!("/directory/members?ids={MEMBER},{OWNER},999")))
                .by(&moderator),
        )
        .await;
    let found: Vec<&str> = ids.json["members"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["id"].as_str())
        .collect();
    assert!(found.contains(&MEMBER.to_string().as_str()));
    assert_eq!(ids.json["members"][0]["in_server"], true);
    for bad in ["", "?query=", "?ids=0", "?query=a&limit=26"] {
        let resp = app
            .send(
                app.get(&g(&format!("/directory/members{bad}")))
                    .by(&moderator),
            )
            .await;
        assert_eq!(resp.code(), Some("validation_failed"), "{bad}");
    }
    let rule = app.h.rule(rules_for_tests::warning("Spam")).await.unwrap();
    app.h.apply(MEMBER, rule).await.unwrap();
    let stats = app.send(app.get(&g("/statistics")).by(&moderator)).await;
    assert_eq!(stats.json["cases_total"], 1, "{}", stats.json);
    assert_eq!(stats.json["cases_by_rule"][0]["label"], "Spam");
    let range = app
        .send(
            app.get(&g(
                "/statistics?from=2026-01-01T00:00:00Z&to=2025-01-01T00:00:00Z",
            ))
            .by(&moderator),
        )
        .await;
    assert_eq!(range.code(), Some("validation_failed"));
    let log = app
        .send(
            app.get(&g("/mod-log?kind=case_created&limit=5"))
                .by(&moderator),
        )
        .await;
    assert_eq!(log.status, StatusCode::OK, "{}", log.json);
    let entry = &log.json["entries"][0];
    assert_eq!(entry["kind"], "case_created");
    assert_eq!(
        entry["links"]["case"],
        format!("/guilds/{}/cases/1", common::GUILD)
    );
    assert!(entry["text"].as_str().unwrap().contains("#1"));
    let by_case = app
        .send(app.get(&g("/mod-log?case_number=1")).by(&moderator))
        .await;
    assert_ne!(by_case.json["entries"].as_array().unwrap().len(), 0);
    let none = app
        .send(app.get(&g("/mod-log?case_number=77")).by(&moderator))
        .await;
    assert_eq!(none.json["entries"], json!([]));
    let bad_kind = app
        .send(app.get(&g("/mod-log?kind=audit")).by(&moderator))
        .await;
    assert_eq!(bad_kind.code(), Some("validation_failed"));
    let me = app.send(app.get(&g("/me")).by(&moderator)).await;
    assert_eq!(me.json["capabilities"]["moderate"], true);
    assert_eq!(me.json["counts"]["failed_actions"], 0);
    let member = app.login(MEMBER, true).await;
    let plain = app.send(app.get(&g("/me")).by(&member)).await;
    assert!(plain.json["counts"].is_null());
    let enable = app
        .send(app.post(&g("/modules/tickets/enable")).by(&admin))
        .await;
    assert_eq!(
        enable.code(),
        Some("unavailable"),
        "module hooks are not wired in tests"
    );
    let unknown = app
        .send(app.post(&g("/modules/logging/enable")).by(&admin))
        .await;
    assert_eq!(unknown.code(), Some("not_found"));
}
