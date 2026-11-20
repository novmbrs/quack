//! Capability per route (§14.2) for every kind of person: anonymous,
//! member, moderator, rules manager, administrator, owner, staff without
//! 2FA in a 2FA server, and someone from another server.

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
use common::{App, GUILD, MEMBER, MEMBER_2, Resp, Who, g, rules_for_tests};
use quack_core::ids::GuildId;
use quack_core::rule::LevelAction;
use quack_engine::Origin;
use serde_json::{Value, json};

/// What a route requires.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Need {
    Moderate,
    ReadRules,
    ManageRules,
    ManageSettings,
    InServer,
}

/// Whether `who` passes `need` (the standard server requires 2FA in this
/// test; everyone but `NoMfa` has it).
fn allowed(who: Who, need: Need) -> Result<(), &'static str> {
    use Need::*;
    use Who::*;
    match (who, need) {
        (Anonymous, _) => Err("sign_in_required"),
        (Outsider, _) => Err("server_not_found"),
        (_, InServer)
        | (Administrator | Owner, _)
        | (Moderator, Moderate | ReadRules)
        | (RulesManager, ReadRules | ManageRules) => Ok(()),
        (NoMfa, Moderate | ReadRules) => Err("mfa_required"),
        _ => Err("missing_capability"),
    }
}

fn check(resp: &Resp, who: Who, need: Need, what: &str) {
    match allowed(who, need) {
        Ok(()) => {
            assert!(
                !matches!(
                    resp.code(),
                    Some(
                        "sign_in_required"
                            | "session_expired"
                            | "missing_capability"
                            | "mfa_required"
                            | "server_not_found"
                            | "csrf_rejected"
                            | "origin_rejected"
                    )
                ),
                "{who:?} {what}: {} {}",
                resp.status,
                resp.json
            );
            assert!(
                resp.status != StatusCode::INTERNAL_SERVER_ERROR,
                "{who:?} {what}: {}",
                resp.json
            );
        }
        Err(code) => {
            assert_eq!(resp.code(), Some(code), "{who:?} {what}: {}", resp.json);
            let status = match code {
                "sign_in_required" => 401,
                "server_not_found" => 404,
                _ => 403,
            };
            assert_eq!(resp.status.as_u16(), status, "{who:?} {what}");
        }
    }
}

struct Ids {
    rule: String,
    case: u64,
    action: String,
    appeal: String,
    appeal_version: u32,
    evidence: String,
}

async fn world(app: &App) -> Ids {
    let rule = app.h.rule(rules_for_tests::warning("Spam")).await.unwrap();
    let timeout = app
        .h
        .rule(rules_for_tests::one(
            "Timeout",
            LevelAction::Timeout {
                duration_seconds: 600,
            },
            false,
        ))
        .await
        .unwrap();
    let case = app.h.apply(MEMBER, rule).await.unwrap();
    let timed = app.h.apply(MEMBER_2, timeout).await.unwrap();
    let submitted = quack_engine::appeals::submit(
        &app.h.engine,
        common::sf(MEMBER),
        Origin::Dashboard,
        case.case.id,
        "Please reconsider.",
        &app.h.key("appeal"),
    )
    .await
    .unwrap();
    let link = format!("https://discord.com/channels/{GUILD}/5/6");
    let added = quack_engine::cases::add_evidence(
        &app.h.engine,
        &app.h.moderator(),
        quack_engine::CaseRef::Id(case.case.id),
        &quack_engine::cases::AddEvidence::links(&[link]),
        &app.h.key("evidence"),
    )
    .await
    .unwrap();
    let mut conn = app.h.db.db.conn().await.unwrap();
    let evidence = quack_db::evidence::for_case(&mut conn, app.h.guild, added.case)
        .await
        .unwrap()[0]
        .id;
    Ids {
        evidence: evidence.get().to_string(),
        rule: rule.get().to_string(),
        case: case.case.number,
        action: timed.action.unwrap().get().to_string(),
        appeal: submitted.appeal.get().to_string(),
        appeal_version: 1,
    }
}

fn routes(ids: &Ids) -> Vec<(Need, Method, String, Option<Value>)> {
    use Need::*;
    let def = json!({
        "name": "Matrix rule", "description": "", "reason": "Be kind.", "appealable": true,
        "counting_window_days": null, "fields": [],
        "levels": [{"from_case_count": 1, "action": null, "send_dm": false}]
    });
    let link = format!("https://discord.com/channels/{GUILD}/5/6");
    let unknown = "0190f5b2-6c1e-7cc0-9a3e-000000000001";
    let (r, c, a, p) = (&ids.rule, ids.case, &ids.action, &ids.appeal);
    let item = &ids.evidence;
    vec![
        (InServer, Method::GET, g("/me"), None),
        (ManageSettings, Method::GET, g("/health"), None),
        (Moderate, Method::GET, g("/statistics"), None),
        (
            Moderate,
            Method::GET,
            g("/directory/members?query=user"),
            None,
        ),
        (ManageSettings, Method::GET, g("/directory/channels"), None),
        (ManageSettings, Method::GET, g("/directory/roles"), None),
        (ReadRules, Method::GET, g("/rules"), None),
        (ManageRules, Method::POST, g("/rules"), Some(def.clone())),
        (ReadRules, Method::GET, g(&format!("/rules/{r}")), None),
        (
            ManageRules,
            Method::PUT,
            g(&format!("/rules/{r}")),
            Some(json!({"expected_version": 1, "definition": def})),
        ),
        (
            ManageRules,
            Method::POST,
            g(&format!("/rules/{r}/archive")),
            None,
        ),
        (
            ManageRules,
            Method::POST,
            g(&format!("/rules/{r}/restore")),
            None,
        ),
        (
            ManageRules,
            Method::GET,
            g(&format!("/rules/{r}/export")),
            None,
        ),
        (
            ManageRules,
            Method::POST,
            g("/rules/import"),
            Some(json!({"confirm": true, "file": {
            "format": "quack-rule", "format_version": 1, "exported_at": "2026-01-01T00:00:00Z",
            "rule": def}, "name": null})),
        ),
        (
            ReadRules,
            Method::GET,
            g(&format!("/rules/{r}/versions/1")),
            None,
        ),
        (Moderate, Method::GET, g("/cases"), None),
        (
            Moderate,
            Method::POST,
            g("/cases"),
            Some(json!({"member_id": MEMBER_2.to_string(),
            "rule_id": r, "context": {}, "evidence_links": []})),
        ),
        (
            Moderate,
            Method::GET,
            g(&format!("/cases/preview?member_id={MEMBER_2}&rule_id={r}")),
            None,
        ),
        (Moderate, Method::GET, g(&format!("/cases/{c}")), None),
        (
            Moderate,
            Method::PUT,
            g(&format!("/cases/{c}/context")),
            Some(json!({"context": {}, "expected_context_revision": 0})),
        ),
        (
            Moderate,
            Method::POST,
            g(&format!("/cases/{c}/evidence")),
            Some(json!({"links": [link]})),
        ),
        (
            Moderate,
            Method::GET,
            g(&format!("/cases/{c}/evidence/{unknown}")),
            None,
        ),
        // A JSON body: allowed people get 415, after authorization.
        (
            Moderate,
            Method::POST,
            g(&format!("/cases/{c}/evidence/uploads")),
            Some(json!({"files": []})),
        ),
        (
            Moderate,
            Method::PUT,
            g(&format!("/cases/{c}/evidence/{item}/note")),
            Some(json!({"note": "Seen.", "expected_note_revision": 0})),
        ),
        (
            Moderate,
            Method::PUT,
            g(&format!("/cases/{c}/evidence/{item}/visibility")),
            Some(json!({"visible_to_member": true})),
        ),
        (
            Moderate,
            Method::POST,
            g(&format!("/cases/{c}/evidence/{item}/remove")),
            None,
        ),
        (
            Moderate,
            Method::GET,
            g(&format!("/members/{MEMBER}/summary")),
            None,
        ),
        (Moderate, Method::GET, g("/actions"), None),
        (
            Moderate,
            Method::POST,
            g(&format!("/actions/{a}/retry")),
            None,
        ),
        (
            Moderate,
            Method::POST,
            g(&format!("/actions/{a}/dismiss")),
            None,
        ),
        (
            Moderate,
            Method::POST,
            g(&format!("/actions/{a}/undo")),
            None,
        ),
        (Moderate, Method::GET, g("/appeals"), None),
        (Moderate, Method::GET, g(&format!("/appeals/{p}")), None),
        (
            Moderate,
            Method::POST,
            g(&format!("/appeals/{p}/decision")),
            Some(json!({
            "decision": "request_information", "reason": "Tell us more.",
            "expected_version": ids.appeal_version})),
        ),
        (Moderate, Method::GET, g("/mod-log"), None),
        (ManageSettings, Method::GET, g("/settings"), None),
        (
            ManageSettings,
            Method::PATCH,
            g("/settings"),
            Some(json!({"dm_footer": "Thanks."})),
        ),
        (
            ManageSettings,
            Method::POST,
            g("/settings/starter-notice/acknowledge"),
            None,
        ),
        (
            ManageSettings,
            Method::POST,
            g("/modules/tickets/enable"),
            None,
        ),
        (
            ManageSettings,
            Method::POST,
            g("/modules/honeypot/disable"),
            None,
        ),
        (ManageSettings, Method::GET, g("/modules/tickets"), None),
        (
            ManageSettings,
            Method::PUT,
            g("/modules/tickets/settings"),
            Some(json!({
            "entry_channel_id": null, "queue_channel_id": null, "transcript_retention_days": 30})),
        ),
        (
            ManageSettings,
            Method::GET,
            g("/modules/activity-log"),
            None,
        ),
        (
            ManageSettings,
            Method::PUT,
            g("/modules/activity-log/settings"),
            Some(json!({
            "routes": {}, "include_message_content": false, "include_file_names": false,
            "include_embed_types": false})),
        ),
        (ManageSettings, Method::GET, g("/modules/honeypot"), None),
        (
            ManageSettings,
            Method::PUT,
            g("/modules/honeypot/settings"),
            Some(json!({
            "channel_id": null, "rule_id": null, "warning_text": null})),
        ),
        (Moderate, Method::GET, g("/tickets"), None),
        // The case's moderator voids it last, so earlier writes see it valid.
        (
            Moderate,
            Method::POST,
            g(&format!("/cases/{c}/void")),
            Some(json!({"reason": "Mistake."})),
        ),
    ]
}

#[tokio::test]
async fn every_staff_route_checks_its_capability() {
    let app = App::new().await;
    let ids = world(&app).await;
    app.set_mfa_required(true);
    let routes = routes(&ids);
    for who in Who::ALL {
        let session = app.session_for(who).await;
        for (need, method, path, body) in &routes {
            let mut req = app.req(method.clone(), path).by_opt(session.as_ref());
            if let Some(body) = body {
                req = req.json(body.clone());
            }
            let resp = app.send(req).await;
            check(&resp, who, *need, &format!("{method} {path}"));
        }
    }
}

#[tokio::test]
async fn member_routes_need_a_session_only() {
    let app = App::new().await;
    for who in Who::ALL {
        let session = app.session_for(who).await;
        let resp = app
            .send(app.get("/api/me/cases").by_opt(session.as_ref()))
            .await;
        if who == Who::Anonymous {
            assert_eq!(resp.code(), Some("sign_in_required"));
        } else {
            assert_eq!(resp.status, StatusCode::OK, "{who:?}");
            assert!(resp.json["cases"].is_array());
        }
    }
}

#[tokio::test]
async fn staff_ids_from_another_server_are_not_found_here() {
    let app = App::new().await;
    let ids = world(&app).await;
    // The same records seen from the other server's path: its owner is
    // staff there, but the records are not.
    let owner = app.login(common::OUTSIDER, true).await;
    let other = |rest: &str| format!("/api/guilds/{}{rest}", common::OTHER_GUILD);
    for path in [
        other(&format!("/cases/{}", ids.case)),
        other(&format!("/rules/{}", ids.rule)),
        other(&format!("/appeals/{}", ids.appeal)),
        other(&format!("/rules/{}/versions/1", ids.rule)),
    ] {
        let resp = app.send(app.get(&path).by(&owner)).await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND, "{path}: {}", resp.json);
        assert_eq!(resp.code(), Some("not_found"), "{path}");
    }
    let retry = app
        .send(
            app.post(&other(&format!("/actions/{}/retry", ids.action)))
                .by(&owner),
        )
        .await;
    assert_eq!(retry.code(), Some("action_not_found"));
    let void = app
        .send(
            app.post(&other(&format!("/cases/{}/void", ids.case)))
                .by(&owner)
                .json(json!({"reason": "Not mine."})),
        )
        .await;
    assert_eq!(void.code(), Some("not_found"));
    let removed = app
        .send(
            app.post(&other(&format!(
                "/cases/{}/evidence/{}/remove",
                ids.case, ids.evidence
            )))
            .by(&owner),
        )
        .await;
    assert_eq!(removed.code(), Some("not_found"), "{}", removed.json);
    let upload = app
        .send(
            app.post(&other(&format!("/cases/{}/evidence/uploads", ids.case)))
                .by(&owner)
                .multipart(&[("files", Some("a.png"), "image/png", b"x".as_slice())]),
        )
        .await;
    assert_eq!(upload.code(), Some("not_found"), "{}", upload.json);
    // And nothing changed in the standard server.
    let _: GuildId = common::sf(GUILD);
    let voided = app
        .h
        .db
        .count("SELECT count(*) FROM cases WHERE voided_at IS NOT NULL")
        .await
        .unwrap();
    assert_eq!(voided, 0);
    let removed = app
        .h
        .db
        .count("SELECT count(*) FROM evidence WHERE removed_at IS NOT NULL")
        .await
        .unwrap();
    assert_eq!(removed, 0);
}
