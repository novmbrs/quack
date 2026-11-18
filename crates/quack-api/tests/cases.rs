//! Case routes (20–25, 58): adding through operation keys, reading,
//! listing with pagination, voiding, editing context, adding evidence.

mod common;

use axum::http::StatusCode;
use common::{App, GUILD, MEMBER, MEMBER_2, MODERATOR, MODERATOR_2, g, rules_for_tests};
use quack_core::rule::{Field, FieldKey, FieldKind, RuleDefinition};
use serde_json::json;

async fn rule(app: &App, def: RuleDefinition) -> String {
    app.h.rule(def).await.unwrap().get().to_string()
}

fn with_fields(name: &str) -> RuleDefinition {
    let mut def = rules_for_tests::warning(name);
    def.fields = vec![
        Field {
            key: FieldKey("amount".to_owned()),
            label: "Amount".to_owned(),
            kind: FieldKind::Number,
            required: true,
        },
        Field {
            key: FieldKey("note".to_owned()),
            label: "Note".to_owned(),
            kind: FieldKind::ShortText,
            required: false,
        },
    ];
    def
}

fn add(member: u64, rule: &str, context: serde_json::Value) -> serde_json::Value {
    json!({"member_id": member.to_string(), "rule_id": rule, "context": context, "evidence_links": []})
}

#[tokio::test]
async fn adding_a_case_is_idempotent_per_account_and_operation() {
    let app = App::new().await;
    let rule = rule(&app, with_fields("Spam")).await;
    let moderator = app.login(MODERATOR, true).await;
    let body = add(MEMBER, &rule, json!({"amount": 2.50, "note": "first"}));
    let first = app
        .send(
            app.post(&g("/cases"))
                .by(&moderator)
                .key("case-key-0001")
                .json(body.clone()),
        )
        .await;
    assert_eq!(first.status, StatusCode::CREATED, "{}", first.json);
    assert_eq!(first.json["number"], 1);
    assert_eq!(first.json["member_id"], MEMBER.to_string());
    assert_eq!(first.json["context"]["amount"], "2.5");
    assert_eq!(first.json["source"], "dashboard");
    assert_eq!(first.json["dm_state"], "pending");
    assert!(first.json["created_at"].as_str().unwrap().ends_with('Z'));
    // The same request again returns the same case and writes nothing.
    let replay = app
        .send(
            app.post(&g("/cases"))
                .by(&moderator)
                .key("case-key-0001")
                .json(body.clone()),
        )
        .await;
    assert_eq!(replay.status, StatusCode::CREATED);
    assert_eq!(replay.json["id"], first.json["id"]);
    assert_eq!(
        app.h.db.count("SELECT count(*) FROM cases").await.unwrap(),
        1
    );
    // The same key with a different body is refused.
    let reused = app
        .send(
            app.post(&g("/cases"))
                .by(&moderator)
                .key("case-key-0001")
                .json(add(MEMBER_2, &rule, json!({"amount": 1}))),
        )
        .await;
    assert_eq!(reused.status, StatusCode::CONFLICT);
    assert_eq!(reused.code(), Some("idempotency_key_reused"));
    // Another account's identical key is its own operation.
    let other = app.login(MODERATOR_2, true).await;
    let theirs = app
        .send(
            app.post(&g("/cases"))
                .by(&other)
                .key("case-key-0001")
                .json(body),
        )
        .await;
    assert_eq!(theirs.status, StatusCode::CREATED);
    assert_ne!(theirs.json["id"], first.json["id"]);
    // The same key on another operation doesn't collide either.
    let void = app
        .send(
            app.post(&g("/cases/1/void"))
                .by(&moderator)
                .key("case-key-0001")
                .json(json!({"reason": "Added by mistake."})),
        )
        .await;
    assert_eq!(void.status, StatusCode::OK, "{}", void.json);
    assert_eq!(void.json["void"]["cause"], "staff");
    assert_eq!(
        app.h.db.count("SELECT count(*) FROM cases").await.unwrap(),
        2
    );
}

#[tokio::test]
async fn writes_need_a_key_json_and_a_bounded_body() {
    let app = App::new().await;
    let rule = rule(&app, rules_for_tests::warning("Spam")).await;
    let moderator = app.login(MODERATOR, true).await;
    let missing =
        app.send(app.post(&g("/cases")).by(&moderator).no_key().json(add(
            MEMBER,
            &rule,
            json!({}),
        )))
        .await;
    assert_eq!(missing.status, StatusCode::PRECONDITION_REQUIRED);
    assert_eq!(missing.code(), Some("idempotency_key_required"));
    let short = app
        .send(app.post(&g("/cases")).by(&moderator).key("short").json(add(
            MEMBER,
            &rule,
            json!({}),
        )))
        .await;
    assert_eq!(short.code(), Some("idempotency_key_required"));
    let text = app
        .send(
            app.post(&g("/cases"))
                .by(&moderator)
                .header("content-type", "text/plain"),
        )
        .await;
    assert_eq!(text.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(text.code(), Some("unsupported_media_type"));
    let mut bad = app.post(&g("/cases")).by(&moderator).json(json!("x"));
    bad.body = Some(json!("not an object"));
    let bad = app.send(bad).await;
    assert_eq!(bad.code(), Some("invalid_json"));
    let unknown_field = app
        .send(
            app.post(&g("/cases"))
                .by(&moderator)
                .json(json!({"member_id": "40", "nope": 1})),
        )
        .await;
    assert_eq!(unknown_field.code(), Some("invalid_json"));
    let huge = "x".repeat(300_000);
    let too_big = app
        .send(
            app.post(&g("/cases"))
                .by(&moderator)
                .json(json!({"note": huge})),
        )
        .await;
    assert_eq!(too_big.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(too_big.code(), Some("body_too_large"));
    let declared = app
        .send(
            app.post(&g("/cases"))
                .by(&moderator)
                .json(json!({}))
                .header("content-length", "999999"),
        )
        .await;
    assert_eq!(declared.code(), Some("body_too_large"));
    assert_eq!(
        app.h.db.count("SELECT count(*) FROM cases").await.unwrap(),
        0
    );
}

#[tokio::test]
async fn validation_names_each_field() {
    let app = App::new().await;
    let rule = rule(&app, with_fields("Spam")).await;
    let moderator = app.login(MODERATOR, true).await;
    let bad_member = app
        .send(
            app.post(&g("/cases"))
                .by(&moderator)
                .json(add(0, &rule, json!({}))),
        )
        .await;
    assert_eq!(bad_member.code(), Some("validation_failed"));
    assert_eq!(bad_member.json["error"]["fields"][0]["field"], "member_id");
    let context = app
        .send(app.post(&g("/cases")).by(&moderator).json(add(
            MEMBER,
            &rule,
            json!({"amount": "lots", "extra": true}),
        )))
        .await;
    assert_eq!(context.status, StatusCode::BAD_REQUEST, "{}", context.json);
    let fields = context.json["error"]["fields"].as_array().unwrap();
    let names: Vec<&str> = fields.iter().filter_map(|f| f["field"].as_str()).collect();
    assert!(
        names.contains(&"context.amount") && names.contains(&"context.extra"),
        "{names:?}"
    );
    let amount = fields
        .iter()
        .find(|f| f["field"] == "context.amount")
        .unwrap();
    assert_eq!(amount["code"], "not_a_number");
    assert!(amount["message"].as_str().unwrap().starts_with("Amount"));
    let links = app
        .send(app.post(&g("/cases")).by(&moderator).json(json!({
            "member_id": MEMBER.to_string(), "rule_id": rule, "context": {"amount": 1},
            "evidence_links": ["https://example.com/x"]
        })))
        .await;
    assert_eq!(
        links.json["error"]["fields"][0]["field"],
        "evidence_links[0]"
    );
    let refused = app
        .send(app.post(&g("/cases")).by(&moderator).json(add(
            MODERATOR,
            &rule,
            json!({"amount": 1}),
        )))
        .await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(refused.code(), Some("member_refused"));
    assert_eq!(refused.json["error"]["fields"][0]["code"], "member_is_self");
}

#[tokio::test]
async fn preview_then_a_changed_rule_is_rule_changed() {
    let app = App::new().await;
    let rule = rule(&app, rules_for_tests::warning("Spam")).await;
    let moderator = app.login(MODERATOR, true).await;
    let preview = app
        .send(
            app.get(&g(&format!(
                "/cases/preview?member_id={MEMBER}&rule_id={rule}"
            )))
            .by(&moderator),
        )
        .await;
    assert_eq!(preview.status, StatusCode::OK, "{}", preview.json);
    assert_eq!(preview.json["rule_version"], 1);
    assert_eq!(preview.json["case_count"], 1);
    assert_eq!(preview.json["level"]["from_case_count"], 1);
    let admin = app.login(common::ADMIN, true).await;
    let mut def = serde_json::to_value(rules_for_tests::warning("Spam")).unwrap();
    def["reason"] = json!("New reason.");
    let edit = app
        .send(
            app.req(axum::http::Method::PUT, &g(&format!("/rules/{rule}")))
                .by(&admin)
                .json(json!({"expected_version": 1, "definition": def})),
        )
        .await;
    assert_eq!(edit.status, StatusCode::OK, "{}", edit.json);
    let mut body = add(MEMBER, &rule, json!({}));
    body["rule_version"] = json!(1);
    let stale = app
        .send(app.post(&g("/cases")).by(&moderator).json(body))
        .await;
    assert_eq!(stale.status, StatusCode::CONFLICT);
    assert_eq!(stale.code(), Some("rule_changed"));
    assert!(
        stale.json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Spam")
    );
}

#[tokio::test]
async fn lists_page_with_bound_cursors() {
    let app = App::new().await;
    let rule_id = app.h.rule(rules_for_tests::warning("Spam")).await.unwrap();
    for member in [MEMBER, MEMBER_2, MEMBER] {
        app.h.apply(member, rule_id).await.unwrap();
    }
    let moderator = app.login(MODERATOR, true).await;
    let page1 = app.send(app.get(&g("/cases?limit=2")).by(&moderator)).await;
    assert_eq!(page1.status, StatusCode::OK);
    let numbers: Vec<u64> = page1.json["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["number"].as_u64().unwrap())
        .collect();
    assert_eq!(numbers, [3, 2]);
    let cursor = page1.json["next_cursor"].as_str().unwrap().to_owned();
    let page2 = app
        .send(
            app.get(&g(&format!("/cases?limit=2&cursor={cursor}")))
                .by(&moderator),
        )
        .await;
    assert_eq!(page2.json["cases"][0]["number"], 1);
    assert!(page2.json["next_cursor"].is_null());
    let filtered = app
        .send(
            app.get(&g(&format!(
                "/cases?limit=2&member_id={MEMBER}&cursor={cursor}"
            )))
            .by(&moderator),
        )
        .await;
    assert_eq!(filtered.code(), Some("validation_failed"));
    assert_eq!(filtered.json["error"]["fields"][0]["field"], "cursor");
    for bad in [
        "limit=0",
        "limit=101",
        "state=deleted",
        "member_id=0",
        "created_after=yesterday",
    ] {
        let resp = app
            .send(app.get(&g(&format!("/cases?{bad}"))).by(&moderator))
            .await;
        assert_eq!(resp.status, StatusCode::BAD_REQUEST, "{bad}");
    }
    let by_member = app
        .send(
            app.get(&g(&format!("/cases?member_id={MEMBER}&state=valid")))
                .by(&moderator),
        )
        .await;
    assert_eq!(by_member.json["cases"].as_array().unwrap().len(), 2);
    let summary = app
        .send(
            app.get(&g(&format!("/members/{MEMBER}/summary")))
                .by(&moderator),
        )
        .await;
    assert_eq!(summary.json["total"], 2);
    assert_eq!(summary.json["by_rule"][0]["rule_name"], "Spam");
}

#[tokio::test]
async fn read_void_edit_context_and_add_evidence() {
    let app = App::new().await;
    let rule_id = app.h.rule(with_fields("Spam")).await.unwrap();
    let moderator = app.login(MODERATOR, true).await;
    let created = app
        .send(app.post(&g("/cases")).by(&moderator).json(add(
            MEMBER,
            &rule_id.get().to_string(),
            json!({"amount": 3}),
        )))
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.json);
    let id = created.json["id"].as_str().unwrap().to_owned();
    let revision = created.json["context_revision"].as_u64().unwrap();
    let by_id = app
        .send(app.get(&g(&format!("/cases/{id}"))).by(&moderator))
        .await;
    assert_eq!(by_id.json["number"], 1);
    let missing = app.send(app.get(&g("/cases/99")).by(&moderator)).await;
    assert_eq!(missing.code(), Some("not_found"));
    assert!(
        missing.json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("#99")
    );
    let edited = app
        .send(
            app.req(axum::http::Method::PUT, &g("/cases/1/context"))
                .by(&moderator)
                .json(json!({"context": {"amount": "4", "note": "later"}, "expected_context_revision": revision})),
        )
        .await;
    assert_eq!(edited.status, StatusCode::OK, "{}", edited.json);
    assert_eq!(edited.json["context"]["note"], "later");
    let stale = app
        .send(
            app.req(axum::http::Method::PUT, &g("/cases/1/context"))
                .by(&moderator)
                .json(json!({"context": {"amount": "5"}, "expected_context_revision": revision})),
        )
        .await;
    assert_eq!(stale.code(), Some("context_changed"));
    let link = format!("https://discord.com/channels/{GUILD}/5/6");
    let evidence = app
        .send(
            app.post(&g("/cases/1/evidence"))
                .by(&moderator)
                .json(json!({"links": [link]})),
        )
        .await;
    assert_eq!(evidence.status, StatusCode::OK, "{}", evidence.json);
    assert_eq!(evidence.json["evidence"][0]["state"], "saving");
    let empty = app
        .send(
            app.post(&g("/cases/1/evidence"))
                .by(&moderator)
                .json(json!({"links": []})),
        )
        .await;
    assert_eq!(empty.code(), Some("validation_failed"));
    let too_many: Vec<String> = (0..11)
        .map(|i| format!("https://discord.com/channels/{GUILD}/5/{}", i + 10))
        .collect();
    let over = app
        .send(
            app.post(&g("/cases/1/evidence"))
                .by(&moderator)
                .json(json!({"links": too_many})),
        )
        .await;
    assert_eq!(over.code(), Some("too_many_links"));
    let empty_reason = app
        .send(
            app.post(&g("/cases/1/void"))
                .by(&moderator)
                .json(json!({"reason": "  "})),
        )
        .await;
    assert_eq!(empty_reason.json["error"]["fields"][0]["field"], "reason");
    let void = app
        .send(
            app.post(&g("/cases/1/void"))
                .by(&moderator)
                .json(json!({"reason": "Wrong member."})),
        )
        .await;
    assert_eq!(void.status, StatusCode::OK);
    assert!(void.json["voided_at"].is_string());
    let again = app
        .send(
            app.post(&g("/cases/1/void"))
                .by(&moderator)
                .json(json!({"reason": "Again."})),
        )
        .await;
    assert_eq!(again.code(), Some("case_already_voided"));
    assert!(
        again.json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("#1")
    );
}
