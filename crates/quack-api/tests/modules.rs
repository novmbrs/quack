//! Module routes 39–50 with the modules connected as `serve` connects
//! them: each module's page and settings, turning one on, and tickets read
//! and closed by their member and by staff.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::too_many_lines,
    reason = "test scenarios read top to bottom and fail loudly"
)]

mod common;

use axum::http::{Method, StatusCode};
use common::{ADMIN, App, GUILD, MEMBER, MODERATOR, OUTSIDER};
use quack_core::ids::{ChannelId, GuildId, MemberId, TicketId};
use quack_testkit::sf;
use serde_json::json;

fn g(path: &str) -> String {
    format!("/api/guilds/{GUILD}{path}")
}

#[tokio::test]
async fn module_pages_settings_and_switches() {
    let app = App::new().await.with_modules();
    let admin = app.login(ADMIN, true).await;
    let moderator = app.login(MODERATOR, true).await;

    let tickets = app.send(app.get(&g("/modules/tickets")).by(&admin)).await;
    assert_eq!(tickets.status, StatusCode::OK, "{:?}", tickets.json);
    assert_eq!(tickets.json["enabled"], false);
    assert_eq!(tickets.json["problems"], json!([]));
    assert_eq!(tickets.json["settings"]["entry_channel_id"], json!(null));
    assert_eq!(tickets.json["open_tickets"], 0);

    // Nothing is set up: turning tickets on lists what to fix.
    let on = app
        .send(app.post(&g("/modules/tickets/enable")).by(&admin))
        .await;
    assert_eq!(on.status, StatusCode::CONFLICT, "{:?}", on.json);
    assert_eq!(on.code(), Some("module_not_ready"));

    // A setting without channels saves; the retention is validated.
    let version = tickets.json["version"].clone();
    let saved = app
        .send(
            app.req(Method::PUT, &g("/modules/tickets/settings"))
                .by(&admin)
                .json(
                    json!({"expected_version": version, "entry_channel_id": null,
                             "queue_channel_id": null, "transcript_retention_days": 14}),
                ),
        )
        .await;
    assert_eq!(saved.status, StatusCode::OK, "{:?}", saved.json);
    assert_eq!(saved.json["settings"]["transcript_retention_days"], 14);
    assert_ne!(saved.json["version"], version);
    // A page read before that save can't overwrite it (dashboard review C9).
    let stale = app
        .send(
            app.req(Method::PUT, &g("/modules/tickets/settings"))
                .by(&admin)
                .json(
                    json!({"expected_version": version, "entry_channel_id": null,
                             "queue_channel_id": null, "transcript_retention_days": 30}),
                ),
        )
        .await;
    assert_eq!(stale.code(), Some("settings_changed"), "{:?}", stale.json);
    let bad = app
        .send(
            app.req(Method::PUT, &g("/modules/tickets/settings"))
                .by(&admin)
                .json(
                    json!({"expected_version": saved.json["version"], "entry_channel_id": null,
                             "queue_channel_id": null, "transcript_retention_days": 0}),
                ),
        )
        .await;
    assert_eq!(bad.code(), Some("validation_failed"), "{:?}", bad.json);

    let honeypot = app.send(app.get(&g("/modules/honeypot")).by(&admin)).await;
    assert_eq!(honeypot.status, StatusCode::OK, "{:?}", honeypot.json);
    assert_eq!(honeypot.json["cases_created"], 0);
    assert_eq!(honeypot.json["pending_hits"], 0);
    assert!(honeypot.json["eligible_rules"].is_array());
    // Dashboard review C8: each eligible rule carries its levels, so the
    // warning preview needs no rule read.
    let eligible = honeypot.json["eligible_rules"].as_array().unwrap();
    assert!(
        eligible.iter().all(|r| r["levels"].is_array()),
        "{eligible:?}"
    );
    let warned = app
        .send(
            app.req(Method::PUT, &g("/modules/honeypot/settings"))
                .by(&admin)
                .json(json!({"expected_version": honeypot.json["version"],
                             "channel_id": null, "rule_id": null,
                             "warning_text": "Please don't post here."})),
        )
        .await;
    assert_eq!(warned.status, StatusCode::OK, "{:?}", warned.json);
    assert_eq!(
        warned.json["settings"]["warning_text"],
        "Please don't post here."
    );

    let activity = app
        .send(app.get(&g("/modules/activity-log")).by(&admin))
        .await;
    assert_eq!(activity.status, StatusCode::OK, "{:?}", activity.json);
    assert_eq!(activity.json["posted_since_start"], 0);
    assert_eq!(activity.json["settings"]["routes"], json!({}));
    let off = app
        .send(app.post(&g("/modules/activity-log/disable")).by(&admin))
        .await;
    assert_eq!(off.status, StatusCode::OK, "{:?}", off.json);
    assert_eq!(off.json["enabled"], false);

    // Module settings are for administrators.
    let refused = app
        .send(app.get(&g("/modules/tickets")).by(&moderator))
        .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
}

/// An open ticket of `member`, inserted the way the open job leaves it.
async fn open_ticket(app: &App, member: u64) -> TicketId {
    let guild: GuildId = sf(GUILD);
    let ticket = TicketId::from_uuid(uuid::Uuid::now_v7());
    let mut tx = app.h.db.db.begin().await.unwrap();
    let member: MemberId = sf(member);
    let entry: ChannelId = sf(4_444);
    quack_db::tickets::insert(&mut tx, ticket, guild, member, entry)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    app.h
        .db
        .execute(&format!(
            "UPDATE tickets SET state = 'open', thread_id = 5555, opened_at = now() WHERE id = '{}'",
            ticket.get()
        ))
        .await
        .unwrap();
    ticket
}

#[tokio::test]
async fn tickets_for_their_member_and_for_staff() {
    let app = App::new().await.with_modules();
    let member = app.login(MEMBER, true).await;
    let moderator = app.login(MODERATOR, true).await;
    let outsider = app.login(OUTSIDER, true).await;
    let ticket = open_ticket(&app, MEMBER).await;
    let path = g(&format!("/tickets/{}", ticket.get()));

    let list = app.send(app.get(&g("/tickets")).by(&moderator)).await;
    assert_eq!(list.status, StatusCode::OK, "{:?}", list.json);
    assert_eq!(list.json["tickets"].as_array().unwrap().len(), 1);
    assert_eq!(list.json["next_cursor"], json!(null), "no empty last page");
    let by_member = app.send(app.get(&g("/tickets")).by(&member)).await;
    assert_eq!(by_member.status, StatusCode::FORBIDDEN);
    let bad_state = app
        .send(app.get(&g("/tickets?state=archived")).by(&moderator))
        .await;
    assert_eq!(bad_state.code(), Some("validation_failed"));
    // Dashboard review C2: a tab asks for several states at once.
    let live = app
        .send(
            app.get(&g("/tickets?state=opening,open,closing"))
                .by(&moderator),
        )
        .await;
    assert_eq!(live.status, StatusCode::OK, "{:?}", live.json);
    assert_eq!(live.json["tickets"].as_array().unwrap().len(), 1);
    let done = app
        .send(app.get(&g("/tickets?state=closed")).by(&moderator))
        .await;
    assert_eq!(done.json["tickets"].as_array().unwrap().len(), 0);

    // The member's projection has no staff ids.
    let mine = app.send(app.get(&path).by(&member)).await;
    assert_eq!(mine.status, StatusCode::OK, "{:?}", mine.json);
    assert_eq!(mine.json["state"], "open");
    assert!(mine.json.get("closed_by").is_none());
    assert_eq!(mine.json["closed_by_you"], false);
    let staff = app.send(app.get(&path).by(&moderator)).await;
    assert!(staff.json.get("closed_by_you").is_none());

    // Someone else's ticket, or one in another server, is not found.
    let hidden = app.send(app.get(&path).by(&outsider)).await;
    assert_eq!(hidden.status, StatusCode::NOT_FOUND);
    let none = app
        .send(
            app.get(&g(&format!("/tickets/{}/transcript", ticket.get())))
                .by(&member),
        )
        .await;
    assert_eq!(none.code(), Some("file_unavailable"), "{:?}", none.json);

    // The member closes it; a replay of the same request answers the same.
    let close = app
        .post(&format!("{path}/close"))
        .by(&member)
        .json(json!({}));
    let closed = app.send(close.clone()).await;
    assert_eq!(closed.status, StatusCode::OK, "{:?}", closed.json);
    assert_eq!(closed.json["state"], "closing");
    assert_eq!(closed.json["closed_by_you"], true);
    let again = app.send(close).await;
    assert_eq!(again.status, StatusCode::OK, "{:?}", again.json);
    // A new request for a ticket that is no longer open is refused.
    let late = app
        .send(app.post(&format!("{path}/close")).by(&moderator))
        .await;
    assert_eq!(late.status, StatusCode::CONFLICT, "{:?}", late.json);
    let jobs = app
        .h
        .db
        .count("SELECT count(*) FROM jobs WHERE kind = 'close_ticket'")
        .await
        .unwrap();
    assert_eq!(jobs, 1, "one close job");
}
