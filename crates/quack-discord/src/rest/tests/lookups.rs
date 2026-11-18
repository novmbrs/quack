//! Reads that combine several calls: accounts and thread lookup.

use httpmock::MockServer;
use quack_core::ids::{AccountId, ChannelId, GuildId};
use serde_json::json;

use super::client;
use crate::fixtures;
use crate::rest::{Outcome, RefusalKind};

fn a(id: u64) -> AccountId {
    AccountId::new(id).unwrap()
}

#[tokio::test]
async fn accounts_leave_out_unknown_users_and_stop_at_other_failures() {
    let server = MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/users/1");
            then.status(200).json_body(fixtures::user(1, "one"));
        })
        .await;
    server
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/users/2");
            then.status(404)
                .json_body(json!({"code": 10013, "message": "Unknown User"}));
        })
        .await;
    server
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/users/3");
            then.status(403)
                .json_body(json!({"code": 50001, "message": "Missing Access"}));
        })
        .await;
    let client = client(&server.base_url());
    let found = client.accounts(&[a(1), a(2)]).await.done().unwrap();
    assert_eq!(found.iter().map(|f| f.id).collect::<Vec<_>>(), vec![a(1)]);
    assert_eq!(found[0].username, "one");
    assert_eq!(
        client.accounts(&[a(1), a(3), a(2)]).await.refusal(),
        Some(RefusalKind::MissingAccess)
    );
    assert_eq!(client.accounts(&[]).await, Outcome::Done(Vec::new()));
}

#[tokio::test]
async fn find_threads_reads_active_and_every_archived_page() {
    let server = MockServer::start_async().await;
    let name = "ticket-AAAAAAAAAAAAAAAAAAAAAQ";
    server
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/guilds/1/threads/active");
            then.status(200).json_body(json!({
                "threads": [
                    fixtures::thread(30, 5, name, "2026-11-17T12:00:00+00:00"),
                    fixtures::thread(31, 6, name, "2026-11-17T12:00:00+00:00"),
                    fixtures::thread(32, 5, "ticket-other", "2026-11-17T12:00:00+00:00")
                ],
                "members": []
            }));
        })
        .await;
    let first = server
        .mock_async(|when, then| {
            when.method("GET")
                .path("/api/v10/channels/5/threads/archived/private")
                .query_param("limit", "100")
                .query_param_missing("before");
            then.status(200).json_body(json!({
                "threads": [fixtures::thread(20, 5, name, "2026-11-16T10:00:00+00:00")],
                "members": [],
                "has_more": true
            }));
        })
        .await;
    let second = server
        .mock_async(|when, then| {
            when.method("GET")
                .path("/api/v10/channels/5/threads/archived/private")
                .query_param("before", "2026-11-16T10:00:00Z");
            then.status(200).json_body(json!({
                "threads": [
                    fixtures::thread(10, 5, name, "2026-11-15T10:00:00+00:00"),
                    fixtures::thread(30, 5, name, "2026-11-15T09:00:00+00:00")
                ],
                "members": [],
                "has_more": false
            }));
        })
        .await;
    let client = client(&server.base_url());
    let found = client
        .find_threads(GuildId::new(1).unwrap(), ChannelId::new(5).unwrap(), name)
        .await
        .done()
        .unwrap();
    let ids: Vec<u64> = found.iter().map(|c| c.get()).collect();
    assert_eq!(ids, vec![10, 20, 30]);
    first.assert_async().await;
    second.assert_async().await;
}

#[tokio::test]
async fn find_threads_stops_at_the_first_failure() {
    let server = MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/guilds/1/threads/active");
            then.status(200)
                .json_body(json!({"threads": [], "members": []}));
        })
        .await;
    server
        .mock_async(|when, then| {
            when.method("GET")
                .path("/api/v10/channels/5/threads/archived/private");
            then.status(403)
                .json_body(json!({"code": 50001, "message": "Missing Access"}));
        })
        .await;
    let client = client(&server.base_url());
    assert_eq!(
        client
            .find_threads(GuildId::new(1).unwrap(), ChannelId::new(5).unwrap(), "x")
            .await
            .refusal(),
        Some(RefusalKind::MissingAccess)
    );
}
