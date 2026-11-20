//! Evidence beyond message links (routes 25, 60–63, §7): uploading files
//! part by part, the up-front refusals, notes with their revision check,
//! showing uploads to the member, removal, and what staff read back.

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
use common::{App, GUILD, MEMBER, MODERATOR, Resp, Session, evidence_channel, g, rules_for_tests};
use quack_core::ids::GuildId;
use serde_json::{Value, json};

async fn case(app: &App) -> u64 {
    let rule = app.h.rule(rules_for_tests::warning("Spam")).await.unwrap();
    app.h.apply(MEMBER, rule).await.unwrap().case.number
}

async fn upload(app: &App, by: &Session, parts: &[common::Part<'_>]) -> Resp {
    app.send(
        app.post(&g("/cases/1/evidence/uploads"))
            .by(by)
            .multipart(parts),
    )
    .await
}

fn item<'a>(case: &'a Value, kind: &str) -> &'a Value {
    case["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == kind)
        .unwrap_or_else(|| panic!("no {kind} item: {case}"))
}

fn put(app: &App, path: &str) -> common::Req {
    app.req(Method::PUT, path)
}

#[tokio::test]
async fn uploads_are_copied_staff_only_with_their_note() {
    let app = App::new().await;
    case(&app).await;
    evidence_channel(&app).await;
    let moderator = app.login(MODERATOR, true).await;
    let parts = [
        (
            "files",
            Some("proof.png"),
            "image/png",
            b"png bytes".as_slice(),
        ),
        ("note", None, "text/plain", b"  From a ticket.  ".as_slice()),
        ("files", Some("chat.txt"), "text/plain", b"hello".as_slice()),
    ];
    let added = app
        .send(
            app.post(&g("/cases/1/evidence/uploads"))
                .by(&moderator)
                .key("upload-key-1")
                .multipart(&parts),
        )
        .await;
    assert_eq!(added.status, StatusCode::OK, "{}", added.json);
    assert_eq!(added.json["files"], 2);
    assert_eq!(added.json["new"], 0);
    assert!(
        added.json["message"]
            .as_str()
            .unwrap()
            .contains("Quack is copying 2 files to the evidence channel for case #1."),
        "{}",
        added.json
    );
    let up = item(&added.json["case"], "upload");
    assert_eq!(up["state"], "saving");
    assert_eq!(up["visible_to_member"], false);
    assert_eq!(up["note"], "From a ticket.");
    assert_eq!(up["note_revision"], 0);
    assert_eq!(up["note_updated_by"], MODERATOR.to_string());
    assert!(up["channel_id"].is_null() && up["message_id"].is_null() && up["url"].is_null());
    assert_eq!(up["added_by"], MODERATOR.to_string());
    assert_eq!(up["files"][0]["filename"], "proof.png");
    assert_eq!(up["files"][0]["copy_state"], "pending");
    assert_eq!(up["files"][1]["content_type"], "text/plain");
    assert!(up["removed"].is_null() && up["surrounding"].is_null());

    // The same request again replays: no second item.
    let again = app
        .send(
            app.post(&g("/cases/1/evidence/uploads"))
                .by(&moderator)
                .key("upload-key-1")
                .multipart(&parts),
        )
        .await;
    assert_eq!(again.status, StatusCode::OK, "{}", again.json);
    let uploads = app
        .h
        .db
        .count("SELECT count(*) FROM evidence WHERE kind = 'upload'")
        .await
        .unwrap();
    assert_eq!(uploads, 1);

    // The capture job copies them; the staff file route serves the copy.
    app.h.settle().await.unwrap();
    let staged = app
        .h
        .db
        .count("SELECT count(*) FROM evidence_upload_bytes")
        .await
        .unwrap();
    assert_eq!(staged, 0, "staged bytes are deleted once copied");
    let read = app.send(app.get(&g("/cases/1")).by(&moderator)).await;
    let up = item(&read.json, "upload");
    assert_eq!(up["state"], "saved", "{up}");
    assert_eq!(up["files"][0]["copy_state"], "copied");
    let file = up["files"][0]["id"].as_str().unwrap();
    let served = app
        .send(
            app.get(&g(&format!("/cases/1/evidence/{file}")))
                .by(&moderator),
        )
        .await;
    assert_eq!(served.status, StatusCode::FOUND, "{}", served.json);
}

#[tokio::test]
async fn uploads_are_refused_up_front() {
    let app = App::new().await;
    case(&app).await;
    let moderator = app.login(MODERATOR, true).await;
    let png = [("files", Some("a.png"), "image/png", b"x".as_slice())];

    // No evidence channel: refused before the body counts.
    let none = upload(&app, &moderator, &png).await;
    assert_eq!(none.status, StatusCode::CONFLICT, "{}", none.json);
    assert_eq!(none.code(), Some("evidence_channel_unavailable"));
    assert!(
        none.json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("none is set")
    );
    // One that members can see is refused too.
    let guild: GuildId = common::sf(GUILD);
    let public = app.h.fake.world(|w| w.add_text_channel(guild, false));
    app.h
        .db
        .execute(&format!(
            "UPDATE guild_settings SET evidence_channel_id = {public} WHERE guild_id = {GUILD}"
        ))
        .await
        .unwrap();
    let open = upload(&app, &moderator, &png).await;
    assert_eq!(open.code(), Some("evidence_channel_unavailable"));
    assert!(
        open.json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("visible outside staff")
    );
    evidence_channel(&app).await;

    // Every file Quack won't copy is named, and nothing is saved.
    let big = vec![0u8; 10_485_761];
    let refused = upload(
        &app,
        &moderator,
        &[
            ("files", Some("clip.mp4"), "video/mp4", &big),
            ("files", Some("fine.png"), "image/png", b"ok"),
            (
                "files",
                Some("setup.exe"),
                "application/x-msdownload",
                b"MZ",
            ),
        ],
    )
    .await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{}", refused.json);
    assert_eq!(refused.code(), Some("validation_failed"));
    let fields = refused.json["error"]["fields"].as_array().unwrap();
    let codes: Vec<(&str, &str)> = fields
        .iter()
        .map(|f| (f["field"].as_str().unwrap(), f["code"].as_str().unwrap()))
        .collect();
    assert_eq!(
        codes,
        [("files", "too_large"), ("files", "unsupported_type")]
    );
    assert!(
        fields[0]["message"]
            .as_str()
            .unwrap()
            .contains("clip.mp4 (larger than 10 MB)")
    );

    let eleven: Vec<String> = (0..11).map(|i| format!("{i}.png")).collect();
    let parts: Vec<common::Part<'_>> = eleven
        .iter()
        .map(|n| ("files", Some(n.as_str()), "image/png", b"x".as_slice()))
        .collect();
    let many = upload(&app, &moderator, &parts).await;
    assert_eq!(many.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(many.code(), Some("too_many_files"));

    let long = "n".repeat(501);
    let noted = upload(
        &app,
        &moderator,
        &[png[0], ("note", None, "text/plain", long.as_bytes())],
    )
    .await;
    assert_eq!(noted.json["error"]["fields"][0]["field"], "note");
    assert_eq!(noted.json["error"]["fields"][0]["code"], "note_too_long");

    let empty = upload(&app, &moderator, &[("note", None, "text/plain", b"hi")]).await;
    assert_eq!(empty.json["error"]["fields"][0]["field"], "files");

    let stray = upload(&app, &moderator, &[("links", None, "text/plain", b"x")]).await;
    assert_eq!(stray.json["error"]["fields"][0]["field"], "links");

    let json = app
        .send(
            app.post(&g("/cases/1/evidence/uploads"))
                .by(&moderator)
                .json(json!({"files": []})),
        )
        .await;
    assert_eq!(json.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);

    // A declared body over the upload cap is refused before it is read.
    let (content_type, bytes) = common::multipart(&png);
    let huge = app
        .send(
            app.post(&g("/cases/1/evidence/uploads"))
                .by(&moderator)
                .raw(&content_type, bytes)
                .header("content-length", "104923137"),
        )
        .await;
    assert_eq!(huge.status, StatusCode::PAYLOAD_TOO_LARGE, "{}", huge.json);
    assert_eq!(huge.code(), Some("body_too_large"));
    // Other routes keep the 256 KiB cap.
    let (content_type, bytes) = common::multipart(&png);
    let other = app
        .send(
            app.post(&g("/cases/1/evidence"))
                .by(&moderator)
                .raw(&content_type, bytes)
                .header("content-length", "300000"),
        )
        .await;
    assert_eq!(other.code(), Some("body_too_large"));

    let missing = app
        .send(
            app.post(&g("/cases/9/evidence/uploads"))
                .by(&moderator)
                .multipart(&png),
        )
        .await;
    assert_eq!(missing.code(), Some("not_found"));
    assert!(
        missing.json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("#9")
    );

    let saved = app
        .h
        .db
        .count("SELECT count(*) FROM evidence")
        .await
        .unwrap();
    assert_eq!(saved, 0, "nothing refused was saved");
}

#[tokio::test]
async fn notes_visibility_and_removal() {
    let app = App::new().await;
    case(&app).await;
    evidence_channel(&app).await;
    let moderator = app.login(MODERATOR, true).await;
    let link = format!("https://discord.com/channels/{GUILD}/5/6");
    let linked = app
        .send(
            app.post(&g("/cases/1/evidence"))
                .by(&moderator)
                .json(json!({"links": [link], "note": "Reported in #general."})),
        )
        .await;
    assert_eq!(linked.status, StatusCode::OK, "{}", linked.json);
    assert_eq!(linked.json["new"], 1);
    let message = item(&linked.json["case"], "message").clone();
    assert_eq!(message["note"], "Reported in #general.");
    assert_eq!(message["visible_to_member"], true);
    let up = upload(
        &app,
        &moderator,
        &[("files", Some("a.png"), "image/png", b"x".as_slice())],
    )
    .await;
    let upload_item = item(&up.json["case"], "upload").clone();
    let (m, u) = (
        message["id"].as_str().unwrap(),
        upload_item["id"].as_str().unwrap(),
    );

    // Notes: edited against the revision read; a stale one is refused.
    let note = |id: &str| g(&format!("/cases/1/evidence/{id}/note"));
    let saved =
        app.send(put(&app, &note(u)).by(&moderator).json(
            json!({"note": "Screenshot from the member's DM.", "expected_note_revision": 0}),
        ))
        .await;
    assert_eq!(saved.status, StatusCode::OK, "{}", saved.json);
    assert_eq!(
        saved.json["message"],
        "Quack saved the note on this evidence item of case #1."
    );
    assert_eq!(
        item(&saved.json["case"], "upload")["note"],
        "Screenshot from the member's DM."
    );
    let stale = app
        .send(
            put(&app, &note(u))
                .by(&moderator)
                .json(json!({"note": "Other.", "expected_note_revision": 0})),
        )
        .await;
    assert_eq!(stale.status, StatusCode::CONFLICT);
    assert_eq!(stale.code(), Some("note_changed"));
    let cleared = app
        .send(
            put(&app, &note(u))
                .by(&moderator)
                .json(json!({"note": null, "expected_note_revision": 1})),
        )
        .await;
    assert_eq!(
        cleared.json["message"],
        "Quack removed the note from this evidence item of case #1."
    );
    assert!(item(&cleared.json["case"], "upload")["note"].is_null());
    let long = app
        .send(
            put(&app, &note(m))
                .by(&moderator)
                .json(json!({"note": "n".repeat(501), "expected_note_revision": 1})),
        )
        .await;
    assert_eq!(long.json["error"]["fields"][0]["code"], "note_too_long");

    // Visibility: uploads only.
    let visibility = |id: &str| g(&format!("/cases/1/evidence/{id}/visibility"));
    let shown = app
        .send(
            put(&app, &visibility(u))
                .by(&moderator)
                .json(json!({"visible_to_member": true})),
        )
        .await;
    assert_eq!(shown.status, StatusCode::OK, "{}", shown.json);
    assert_eq!(
        shown.json["message"],
        "The member can now see these files on their page for case #1."
    );
    assert_eq!(
        item(&shown.json["case"], "upload")["visible_to_member"],
        true
    );
    let not_upload = app
        .send(
            put(&app, &visibility(m))
                .by(&moderator)
                .json(json!({"visible_to_member": false})),
        )
        .await;
    assert_eq!(not_upload.code(), Some("evidence_not_upload"));

    // Removal hides it, lists it collapsed for staff, and is recorded.
    let remove = |id: &str| g(&format!("/cases/1/evidence/{id}/remove"));
    let removed = app
        .send(app.post(&remove(m)).by(&moderator).key("remove-key-1"))
        .await;
    assert_eq!(removed.status, StatusCode::OK, "{}", removed.json);
    assert!(
        removed.json["message"]
            .as_str()
            .unwrap()
            .starts_with("Quack removed the evidence item from case #1.")
    );
    let gone = item(&removed.json["case"], "message");
    assert_eq!(gone["removed"]["by"], MODERATOR.to_string());
    assert!(
        gone["removed"]["text"]
            .as_str()
            .unwrap()
            .starts_with("Removed by ")
    );
    let replay = app
        .send(app.post(&remove(m)).by(&moderator).key("remove-key-1"))
        .await;
    assert_eq!(replay.json["message"], removed.json["message"]);
    let twice = app.send(app.post(&remove(m)).by(&moderator)).await;
    assert_eq!(
        twice.json["message"],
        "That evidence item was already removed from case #1."
    );
    let refused = app
        .send(
            put(&app, &note(m))
                .by(&moderator)
                .json(json!({"note": "Late.", "expected_note_revision": 1})),
        )
        .await;
    assert_eq!(refused.code(), Some("evidence_removed"));
    let read = app.send(app.get(&g("/cases/1")).by(&moderator)).await;
    let kinds: Vec<&str> = read.json["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert!(kinds.contains(&"evidence_removed"), "{kinds:?}");
    assert!(kinds.contains(&"evidence_visibility_changed"), "{kinds:?}");

    // An item of another case, or no item: not found.
    let rule = app.h.rule(rules_for_tests::warning("Other")).await.unwrap();
    app.h.apply(common::MEMBER_2, rule).await.unwrap();
    for path in [
        g(&format!("/cases/2/evidence/{u}/remove")),
        g("/cases/1/evidence/0190f5b2-6c1e-7cc0-9a3e-000000000001/remove"),
        g("/cases/1/evidence/garbage/remove"),
    ] {
        let resp = app.send(app.post(&path).by(&moderator)).await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND, "{path}: {}", resp.json);
        assert_eq!(resp.code(), Some("not_found"));
    }
}

#[tokio::test]
async fn saved_messages_carry_the_messages_around_them_for_staff() {
    let app = App::new().await;
    case(&app).await;
    let guild: GuildId = common::sf(GUILD);
    let channel = app.h.fake.world(|w| w.add_text_channel(guild, false));
    let post = |text: &str| {
        app.h
            .fake
            .world(|w| w.post(guild, channel, common::sf(MEMBER), text, vec![]))
    };
    post("before");
    let target = post("the one");
    post("after");
    let moderator = app.login(MODERATOR, true).await;
    let added = app
        .send(
            app.post(&g("/cases/1/evidence"))
                .by(&moderator)
                .json(json!({
                    "links": [format!("https://discord.com/channels/{GUILD}/{channel}/{target}")]
                })),
        )
        .await;
    assert_eq!(added.status, StatusCode::OK, "{}", added.json);
    app.h.settle().await.unwrap();
    let read = app.send(app.get(&g("/cases/1")).by(&moderator)).await;
    let message = item(&read.json, "message");
    assert_eq!(message["content"], "the one");
    let around = message["surrounding"].as_array().unwrap();
    assert_eq!(around.len(), 2, "{message}");
    assert_eq!(around[0]["before"], true);
    assert_eq!(around[0]["content"], "before");
    assert_eq!(around[1]["before"], false);
    assert_eq!(around[1]["author_id"], MEMBER.to_string());
}

/// The upload route over a real connection: hyper hands the body over in
/// pieces as it arrives, an accepted upload is read whole, and a refused
/// 11 MiB file still gets the answer that names it (the rest of the body is
/// read, so the connection isn't reset under the browser).
#[tokio::test]
async fn uploads_stream_over_a_real_connection() {
    let app = App::new().await;
    case(&app).await;
    evidence_channel(&app).await;
    let moderator = app.login(MODERATOR, true).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = app.router.clone();
    let server = tokio::spawn(async move { axum::serve(listener, router).await });
    let client = reqwest::Client::new();
    let send = |parts: &[common::Part<'_>], key: &str| {
        let (content_type, body) = common::multipart(parts);
        client
            .post(format!(
                "http://{addr}/api/guilds/{GUILD}/cases/1/evidence/uploads"
            ))
            .header("content-type", content_type)
            .header("origin", common::PUBLIC)
            .header("x-csrf-token", &moderator.csrf)
            .header("idempotency-key", key)
            .header(
                "cookie",
                format!("__Host-quack_session={}", moderator.cookie),
            )
            .body(body)
            .send()
    };
    let big = vec![7u8; 9 * 1024 * 1024];
    let ok = send(
        &[("files", Some("clip.mp4"), "video/mp4", &big)],
        "real-upload-1",
    )
    .await
    .unwrap();
    assert_eq!(ok.status(), 200);
    let json: Value = ok.json().await.unwrap();
    assert_eq!(json["files"], 1, "{json}");
    let size = item(&json["case"], "upload")["files"][0]["size_bytes"].clone();
    assert_eq!(size, big.len());

    let over = vec![7u8; 11 * 1024 * 1024];
    let refused = send(
        &[
            ("files", Some("huge.mp4"), "video/mp4", &over),
            ("files", Some("after.png"), "image/png", b"x"),
        ],
        "real-upload-2",
    )
    .await
    .unwrap();
    assert_eq!(refused.status(), 400);
    let json: Value = refused.json().await.unwrap();
    assert_eq!(json["error"]["fields"][0]["code"], "too_large", "{json}");
    server.abort();
}
