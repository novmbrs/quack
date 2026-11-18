//! Sign-in, sessions, sign-out, and the CSRF and origin checks (§14.4,
//! §14.5) against a fake Discord OAuth server.

mod common;

use axum::http::{Method, StatusCode};
use common::{App, MODERATOR, PUBLIC, Session, g};
use httpmock::Method::{GET, POST};
use serde_json::json;

/// The query parameter `name` of a URL.
fn param(url: &str, name: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    parsed
        .query_pairs()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.into_owned())
}

/// The value of a `Set-Cookie` named `name`.
fn set_cookie(resp: &common::Resp, name: &str) -> Option<String> {
    resp.headers
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with(&format!("{name}=")))
        .map(str::to_owned)
}

fn cookie_value(set_cookie: &str) -> String {
    set_cookie
        .split(';')
        .next()
        .and_then(|kv| kv.split_once('='))
        .map(|(_, v)| v.to_owned())
        .unwrap_or_default()
}

/// Starts a sign-in; returns the state and the nonce cookie.
async fn start(app: &App, redirect_to: &str) -> (String, String) {
    let path = format!(
        "/api/auth/login?redirect_to={}",
        url::form_urlencoded::byte_serialize(redirect_to.as_bytes()).collect::<String>()
    );
    let resp = app.send(app.get(&path)).await;
    assert_eq!(resp.status, StatusCode::FOUND);
    let location = resp.header("location").unwrap().to_owned();
    assert!(location.starts_with(&app.discord.url("/oauth2/authorize")));
    assert_eq!(param(&location, "client_id").as_deref(), Some("4242"));
    assert_eq!(
        param(&location, "scope").as_deref(),
        Some("identify guilds")
    );
    assert_eq!(
        param(&location, "code_challenge_method").as_deref(),
        Some("S256")
    );
    assert_eq!(
        param(&location, "redirect_uri").as_deref(),
        Some("https://quack.test/api/auth/callback")
    );
    assert!(param(&location, "code_challenge").is_some());
    let oauth = set_cookie(&resp, "__Host-quack_oauth").unwrap();
    assert!(oauth.contains("Max-Age=600"));
    assert!(
        oauth.contains("HttpOnly") && oauth.contains("SameSite=Lax") && oauth.contains("Secure")
    );
    (param(&location, "state").unwrap(), cookie_value(&oauth))
}

async fn mock_discord(app: &App, account: &str, mfa: bool) {
    app.discord
        .mock_async(|when, then| {
            when.method(POST)
                .path("/api/v10/oauth2/token")
                .body_includes("code=good-code")
                .body_includes("code_verifier=");
            then.status(200).json_body(json!({
                "access_token": "discord-access-token",
                "token_type": "Bearer",
                "expires_in": 3600,
                "scope": "identify guilds"
            }));
        })
        .await;
    app.discord
        .mock_async(|when, then| {
            when.method(POST).path("/api/v10/oauth2/token");
            then.status(400)
                .json_body(json!({"error": "invalid_grant"}));
        })
        .await;
    let user = json!({
        "id": account, "username": "someone", "global_name": null,
        "avatar": null, "mfa_enabled": mfa
    });
    app.discord
        .mock_async(move |when, then| {
            when.method(GET)
                .path("/api/v10/users/@me")
                .header("authorization", "Bearer discord-access-token");
            then.status(200).json_body(user);
        })
        .await;
}

async fn callback(
    app: &App,
    query: &str,
    nonce: Option<&str>,
    extra_cookie: Option<&str>,
) -> common::Resp {
    let mut req = app.get(&format!("/api/auth/callback?{query}"));
    let mut cookies = Vec::new();
    if let Some(n) = nonce {
        cookies.push(format!("__Host-quack_oauth={n}"));
    }
    if let Some(c) = extra_cookie {
        cookies.push(c.to_owned());
    }
    if !cookies.is_empty() {
        req = req.header("cookie", &cookies.join("; "));
    }
    app.send(req).await
}

#[tokio::test]
async fn sign_in_happy_path_and_replayed_state() {
    let app = App::new().await;
    mock_discord(&app, "30", true).await;
    let (state, nonce) = start(&app, "/guilds/1001/cases?state=valid").await;
    let resp = callback(
        &app,
        &format!("code=good-code&state={state}"),
        Some(&nonce),
        None,
    )
    .await;
    assert_eq!(resp.status, StatusCode::FOUND);
    assert_eq!(
        resp.header("location"),
        Some("/guilds/1001/cases?state=valid")
    );
    let session = set_cookie(&resp, "__Host-quack_session").unwrap();
    assert!(session.contains("Max-Age=3600"), "{session}");
    assert!(
        session.contains("Path=/") && session.contains("HttpOnly") && session.contains("Secure")
    );
    assert!(!session.contains("Domain"));
    let cleared = set_cookie(&resp, "__Host-quack_oauth").unwrap();
    assert!(cleared.contains("Max-Age=0"));
    // The 2FA flag was captured.
    let mfa = app
        .h
        .db
        .count("SELECT count(*) FROM discord_accounts WHERE account_id = 30 AND mfa_enabled")
        .await
        .unwrap();
    assert_eq!(mfa, 1);
    // The session works.
    let cookie = cookie_value(&session);
    let me = app
        .send(
            app.get("/api/auth/me")
                .header("cookie", &format!("__Host-quack_session={cookie}")),
        )
        .await;
    assert_eq!(me.status, StatusCode::OK);
    assert_eq!(me.json["account"]["id"], "30");
    assert_eq!(me.json["csrf_token"].as_str().map(str::len), Some(43));
    // The state is single use.
    let replay = callback(
        &app,
        &format!("code=good-code&state={state}"),
        Some(&nonce),
        None,
    )
    .await;
    assert_eq!(replay.header("location"), Some("/login?error=failed"));
    // The access token is stored sealed, never in the clear.
    let plain = app
        .h
        .db
        .count("SELECT count(*) FROM sessions WHERE position(convert_to('discord-access-token', 'UTF8') in discord_token_ciphertext) > 0")
        .await
        .unwrap();
    assert_eq!(plain, 0);
}

#[tokio::test]
async fn tampered_state_wrong_browser_and_bad_codes_fail() {
    let app = App::new().await;
    mock_discord(&app, "30", false).await;
    let (state, nonce) = start(&app, "/guilds").await;
    let mut tampered = state.clone();
    tampered.replace_range(0..1, if state.starts_with('A') { "B" } else { "A" });
    for (query, cookie) in [
        (
            format!("code=good-code&state={tampered}"),
            Some(nonce.as_str()),
        ),
        (format!("code=good-code&state={state}"), None),
        (
            format!("code=good-code&state={state}"),
            Some("bm90LXRoZS1yaWdodC1ub25jZS1hdC1hbGwtMzItYnl0ZQ"),
        ),
        ("code=good-code".to_owned(), Some(nonce.as_str())),
    ] {
        let resp = callback(&app, &query, cookie, None).await;
        assert_eq!(
            resp.header("location"),
            Some("/login?error=failed"),
            "{query}"
        );
        assert!(set_cookie(&resp, "__Host-quack_session").is_none());
    }
    // The untouched state still works once, but a bad code fails it.
    let resp = callback(
        &app,
        &format!("code=bad-code&state={state}"),
        Some(&nonce),
        None,
    )
    .await;
    assert_eq!(resp.header("location"), Some("/login?error=failed"));
    let sessions = app
        .h
        .db
        .count("SELECT count(*) FROM sessions")
        .await
        .unwrap();
    assert_eq!(sessions, 0);
}

#[tokio::test]
async fn cancelled_sign_in_and_hostile_redirects() {
    let app = App::new().await;
    mock_discord(&app, "30", false).await;
    let (state, nonce) = start(&app, "/guilds").await;
    let resp = callback(
        &app,
        &format!("error=access_denied&state={state}"),
        Some(&nonce),
        None,
    )
    .await;
    assert_eq!(resp.header("location"), Some("/login?error=denied"));
    for hostile in [
        "https://evil.example/steal",
        "//evil.example/x",
        "/\\evil.example",
        "javascript:alert(1)",
        "http://quack.test/guilds",
    ] {
        let (state, nonce) = start(&app, hostile).await;
        let resp = callback(
            &app,
            &format!("code=good-code&state={state}"),
            Some(&nonce),
            None,
        )
        .await;
        assert_eq!(resp.header("location"), Some("/guilds"), "{hostile}");
    }
}

#[tokio::test]
async fn sign_in_replaces_the_browsers_previous_session() {
    let app = App::new().await;
    mock_discord(&app, "30", true).await;
    let old = app.login(MODERATOR, true).await;
    let (state, nonce) = start(&app, "/guilds").await;
    let resp = callback(
        &app,
        &format!("code=good-code&state={state}"),
        Some(&nonce),
        Some(&format!("__Host-quack_session={}", old.cookie)),
    )
    .await;
    assert!(set_cookie(&resp, "__Host-quack_session").is_some());
    let me = app.send(app.get("/api/auth/me").by(&old)).await;
    assert_eq!(me.code(), Some("session_expired"));
    assert_eq!(
        app.h
            .db
            .count("SELECT count(*) FROM sessions")
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn sessions_end_on_expiry_rotation_and_garbage() {
    let app = App::new().await;
    let anon = app.send(app.get("/api/auth/me")).await;
    assert_eq!(anon.status, StatusCode::UNAUTHORIZED);
    assert_eq!(anon.code(), Some("sign_in_required"));
    let garbage = Session {
        cookie: "not-a-session".to_owned(),
        csrf: String::new(),
        account: 30,
    };
    assert_eq!(
        app.send(app.get("/api/auth/me").by(&garbage)).await.code(),
        Some("session_expired")
    );
    let session = app.login(MODERATOR, true).await;
    assert_eq!(
        app.send(app.get("/api/auth/me").by(&session)).await.status,
        StatusCode::OK
    );
    app.h
        .db
        .execute("UPDATE sessions SET expires_at = now() - interval '1 second'")
        .await
        .unwrap();
    let expired = app.send(app.get("/api/auth/me").by(&session)).await;
    assert_eq!(expired.code(), Some("session_expired"));
    // A session sealed under another secret doesn't load.
    let rotated = app.login(MODERATOR, true).await;
    app.h
        .db
        .execute("UPDATE sessions SET key_fingerprint = '\\x0000000000000000'")
        .await
        .unwrap();
    assert_eq!(
        app.send(app.get("/api/auth/me").by(&rotated)).await.code(),
        Some("session_expired")
    );
}

#[tokio::test]
async fn logout_and_csrf_and_origin() {
    let app = App::new().await;
    let session = app.login(MODERATOR, true).await;
    // Missing or wrong CSRF token.
    let resp = app
        .send(app.post("/api/auth/logout").by(&session).no_csrf())
        .await;
    assert_eq!(resp.status, StatusCode::FORBIDDEN);
    assert_eq!(resp.code(), Some("csrf_rejected"));
    let wrong = Session {
        csrf: "x".repeat(43),
        ..session.clone()
    };
    assert_eq!(
        app.send(app.post("/api/auth/logout").by(&wrong))
            .await
            .code(),
        Some("csrf_rejected")
    );
    // Another origin, or no origin without Sec-Fetch-Site.
    for origin in [Some("https://evil.example"), Some("null"), None] {
        let resp = app
            .send(app.post("/api/auth/logout").by(&session).origin(origin))
            .await;
        assert_eq!(resp.code(), Some("origin_rejected"), "{origin:?}");
    }
    // CSRF guards every mutation, not only logout.
    let resp = app
        .send(
            app.post(&g("/rules"))
                .by(&session)
                .no_csrf()
                .json(serde_json::json!({})),
        )
        .await;
    assert_eq!(resp.code(), Some("csrf_rejected"));
    // Same-origin fetch metadata stands in for Origin.
    let resp = app
        .send(
            app.post("/api/auth/logout")
                .by(&session)
                .origin(None)
                .header("sec-fetch-site", "same-origin"),
        )
        .await;
    assert_eq!(resp.status, StatusCode::NO_CONTENT);
    let cleared = set_cookie(&resp, "__Host-quack_session").unwrap();
    assert!(cleared.contains("Max-Age=0"));
    let after = app.send(app.get("/api/auth/me").by(&session)).await;
    assert_eq!(after.code(), Some("session_expired"));
    // Logging out again without a session is 401.
    let again = app.send(app.post("/api/auth/logout").by(&session)).await;
    assert_eq!(again.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn guild_list_comes_from_discord_and_is_cached() {
    let app = App::new().await;
    mock_discord(&app, "30", true).await;
    let guilds = app
        .discord
        .mock_async(|when, then| {
            when.method(GET)
                .path("/api/v10/users/@me/guilds")
                .header("authorization", "Bearer discord-access-token");
            then.status(200).json_body(json!([
                {"id": "1001", "name": "Server", "icon": null, "owner": false, "permissions": "0"},
                {"id": "3003", "name": "Elsewhere", "icon": "abc", "owner": false, "permissions": "32"},
                {"id": "4004", "name": "Nothing", "icon": null, "owner": false, "permissions": "0"}
            ]));
        })
        .await;
    let (state, nonce) = start(&app, "/guilds").await;
    let resp = callback(
        &app,
        &format!("code=good-code&state={state}"),
        Some(&nonce),
        None,
    )
    .await;
    let cookie = cookie_value(&set_cookie(&resp, "__Host-quack_session").unwrap());
    let req = || {
        app.get("/api/guilds")
            .header("cookie", &format!("__Host-quack_session={cookie}"))
    };
    let list = app.send(req()).await;
    assert_eq!(list.status, StatusCode::OK, "{}", list.json);
    let rows = list.json["guilds"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["guild_id"], "1001");
    assert_eq!(rows[0]["installed"], true);
    assert_eq!(rows[0]["capabilities"]["moderate"], true);
    assert_eq!(rows[0]["capabilities"]["manage_settings"], false);
    assert_eq!(rows[1]["installed"], false);
    assert_eq!(rows[1]["can_install"], true);
    assert_eq!(
        rows[1]["icon_url"],
        "https://cdn.discordapp.com/icons/3003/abc.png"
    );
    assert_eq!(rows[2]["can_install"], false);
    let again = app.send(req()).await;
    assert_eq!(again.status, StatusCode::OK);
    assert_eq!(guilds.calls_async().await, 1);
    let _ = PUBLIC;
    let _ = Method::GET;
}
