//! Everything around the routes: rate limits, unknown paths, security
//! headers, health probes, dashboard files, and replays after access is
//! lost.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::needless_pass_by_value,
    clippy::unused_self,
    reason = "test scenarios read top to bottom and fail loudly"
)]

mod common;

use std::sync::atomic::Ordering;

use axum::http::{Method, StatusCode};
use common::{App, MEMBER, MODERATOR, g, rules_for_tests};
use serde_json::json;

#[tokio::test]
async fn rate_limits_answer_429_with_retry_after() {
    let app = App::new().await;
    for i in 0..20 {
        let resp = app.send(app.get("/api/auth/login")).await;
        assert_eq!(resp.status, StatusCode::FOUND, "attempt {i}");
    }
    let limited = app.send(app.get("/api/auth/login")).await;
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(limited.code(), Some("rate_limited"));
    let after: u64 = limited.header("retry-after").unwrap().parse().unwrap();
    assert!((1..=30).contains(&after), "{after}");
    assert_eq!(limited.json["error"]["retry_after_seconds"], after);
    assert!(
        limited.json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("second")
    );
    // Member writes: 10 per minute per account; another account is apart.
    let member = app.login(MEMBER, true).await;
    let path = "/api/me/cases/0190f5b2-6c1e-7cc0-9a3e-000000000001/appeal";
    for _ in 0..10 {
        let resp = app
            .send(app.post(path).by(&member).json(json!({"statement": "Hi"})))
            .await;
        assert_eq!(resp.code(), Some("not_found"));
    }
    let limited = app
        .send(app.post(path).by(&member).json(json!({"statement": "Hi"})))
        .await;
    assert_eq!(limited.code(), Some("rate_limited"));
    let other = app.login(common::MEMBER_2, true).await;
    let fine = app
        .send(app.post(path).by(&other).json(json!({"statement": "Hi"})))
        .await;
    assert_eq!(fine.code(), Some("not_found"));
}

#[tokio::test]
async fn unknown_api_paths_and_methods_are_not_found() {
    let app = App::new().await;
    let moderator = app.login(MODERATOR, true).await;
    for (method, path) in [
        (Method::GET, "/api/nope".to_owned()),
        (Method::GET, "/api".to_owned()),
        (Method::DELETE, g("/cases")),
        (Method::GET, "/api/templates".to_owned()),
    ] {
        let resp = app
            .send(app.req(method.clone(), &path).by(&moderator))
            .await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND, "{method} {path}");
        assert_eq!(resp.code(), Some("not_found"), "{method} {path}");
        assert_eq!(resp.header("cache-control"), Some("no-store"));
    }
    let bad_guild = app
        .send(app.get("/api/guilds/abc/cases").by(&moderator))
        .await;
    assert_eq!(bad_guild.code(), Some("validation_failed"));
    let not_installed = app
        .send(app.get("/api/guilds/5555/cases").by(&moderator))
        .await;
    assert_eq!(not_installed.code(), Some("server_not_found"));
}

#[tokio::test]
async fn security_headers_on_every_response() {
    let app = App::new().await;
    for path in ["/api/auth/me", "/livez", "/", "/assets/app-3f9a1c.js"] {
        let resp = app.send(app.get(path)).await;
        assert_eq!(
            resp.header("x-content-type-options"),
            Some("nosniff"),
            "{path}"
        );
        assert_eq!(resp.header("referrer-policy"), Some("no-referrer"));
        assert_eq!(resp.header("x-frame-options"), Some("DENY"));
        assert_eq!(
            resp.header("permissions-policy"),
            Some("camera=(), microphone=(), geolocation=()")
        );
        assert_eq!(
            resp.header("strict-transport-security"),
            Some("max-age=31536000")
        );
    }
    let api = app.send(app.get("/api/auth/me")).await;
    assert_eq!(
        api.header("content-security-policy"),
        Some("default-src 'none'; frame-ancestors 'none'")
    );
    assert!(
        api.json["error"]
            .as_object()
            .unwrap()
            .contains_key("trace_id")
    );
}

#[tokio::test]
async fn health_probes() {
    let app = App::new().await;
    let live = app.send(app.get("/livez")).await;
    assert_eq!(live.json, json!({"status": "ok"}));
    let ready = app.send(app.get("/readyz")).await;
    assert_eq!(ready.status, StatusCode::OK, "{}", ready.json);
    assert_eq!(ready.json["ready"], true);
    app.readiness.gateway.store(false, Ordering::Relaxed);
    app.readiness.shutdown.store(true, Ordering::Relaxed);
    let not_ready = app.send(app.get("/readyz")).await;
    assert_eq!(not_ready.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(not_ready.json["checks"]["gateway"], "failing");
    assert_eq!(not_ready.json["checks"]["shutdown"], "failing");
    assert_eq!(not_ready.json["checks"]["database"], "ok");
    assert_eq!(app.send(app.get("/livez")).await.status, StatusCode::OK);
}

#[tokio::test]
async fn dashboard_files_and_spa_fallback() {
    let app = App::new().await;
    let asset = app.send(app.get("/assets/app-3f9a1c.js")).await;
    assert_eq!(asset.status, StatusCode::OK);
    assert_eq!(
        asset.header("cache-control"),
        Some("public, max-age=31536000, immutable")
    );
    assert_eq!(
        asset.header("content-type"),
        Some("text/javascript; charset=utf-8")
    );
    let gz = app
        .send(
            app.get("/assets/app-3f9a1c.js")
                .header("accept-encoding", "gzip"),
        )
        .await;
    assert_eq!(gz.header("content-encoding"), Some("gzip"));
    let icon = app.send(app.get("/favicon.svg")).await;
    assert_eq!(icon.header("cache-control"), Some("no-cache"));
    for path in ["/", "/guilds/1001/cases", "/login"] {
        let page = app.send(app.get(path)).await;
        assert_eq!(page.status, StatusCode::OK, "{path}");
        assert!(String::from_utf8_lossy(&page.body).contains("<title>Quack</title>"));
        assert_eq!(page.header("cache-control"), Some("no-cache"));
        assert!(
            page.header("content-security-policy")
                .unwrap()
                .contains("cdn.visitors.now")
        );
    }
    for path in [
        "/assets/missing.js",
        "/missing.png",
        "/..%2fsecret.txt",
        "/assets/..%2f..%2fsecret.txt",
        "/%2e%2e/secret.txt",
        "/assets/%2e%2e/%2e%2e/secret.txt",
    ] {
        let resp = app.send(app.get(path)).await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND, "{path}");
        assert!(
            !String::from_utf8_lossy(&resp.body).contains("outside"),
            "{path}"
        );
    }
    let post = app.send(app.post("/guilds")).await;
    assert_eq!(post.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(post.header("allow"), Some("GET, HEAD"));
    let head = app.send(app.req(Method::HEAD, "/")).await;
    assert_eq!(head.status, StatusCode::OK);
    assert_eq!(head.body.len(), 0);
}

#[tokio::test]
async fn a_replay_is_refused_once_access_is_lost() {
    let app = App::new().await;
    let rule = app.h.rule(rules_for_tests::warning("Spam")).await.unwrap();
    let moderator = app.login(MODERATOR, true).await;
    let body = json!({"member_id": MEMBER.to_string(), "rule_id": rule.get().to_string(),
                      "context": {}, "evidence_links": []});
    let first = app
        .send(
            app.post(&g("/cases"))
                .by(&moderator)
                .key("replay-key-1")
                .json(body.clone()),
        )
        .await;
    assert_eq!(first.status, StatusCode::CREATED);
    // The moderator loses Moderate Members.
    let guild: quack_core::ids::GuildId = common::sf(common::GUILD);
    app.h.fake.world(|w| {
        if let Some(m) = w.member_mut(guild, common::sf(MODERATOR)) {
            m.permissions = quack_core::permissions::Permissions::NONE;
        }
    });
    let replay = app
        .send(
            app.post(&g("/cases"))
                .by(&moderator)
                .key("replay-key-1")
                .json(body),
        )
        .await;
    assert_eq!(replay.status, StatusCode::FORBIDDEN);
    assert_eq!(replay.code(), Some("missing_capability"));
    assert!(replay.json["error"].get("id").is_none());
}
