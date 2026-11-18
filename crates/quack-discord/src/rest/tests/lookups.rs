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

fn long_ago() -> time::OffsetDateTime {
    time::macros::datetime!(2026-01-01 00:00 UTC)
}

/// Discord review BLOCKER 2: only private threads Quack created, with the
/// exact name and parent, are ever returned, so recovery can't adopt a
/// public thread or delete someone else's.
#[tokio::test]
async fn find_threads_returns_only_quacks_own_private_threads() {
    let server = MockServer::start_async().await;
    let name = "ticket-AAAAAAAAAAAAAAAAAAAAAQ";
    let at = "2026-11-17T12:00:00+00:00";
    server
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/guilds/1/threads/active");
            then.status(200).json_body(json!({
                "threads": [
                    // A member's public thread with the ticket's name.
                    fixtures::thread_by(10, 5, name, at, 7, 11),
                    // A member's private thread with the ticket's name.
                    fixtures::thread_by(11, 5, name, at, 7, 12),
                    // Quack's, but public.
                    fixtures::thread_by(12, 5, name, at, 42, 11),
                    // Quack's own.
                    fixtures::thread_by(13, 5, name, at, 42, 12)
                ],
                "members": []
            }));
        })
        .await;
    server
        .mock_async(|when, then| {
            when.method("GET")
                .path("/api/v10/channels/5/threads/archived/private");
            then.status(200).json_body(json!({
                "threads": [fixtures::thread_by(14, 5, name, at, 7, 12)],
                "members": [],
                "has_more": false
            }));
        })
        .await;
    let found = client(&server.base_url())
        .find_threads(
            GuildId::new(1).unwrap(),
            ChannelId::new(5).unwrap(),
            name,
            a(42),
            long_ago(),
        )
        .await
        .done()
        .unwrap();
    assert_eq!(found, vec![ChannelId::new(13).unwrap()]);
}

/// Discord review MAJOR 13: a lookup that stops at the page cap or on a
/// page without a cursor is not complete, so it never authorizes a create.
#[tokio::test]
async fn an_incomplete_archived_search_is_unknown() {
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
            then.status(200).json_body(json!({
                "threads": [fixtures::thread(20, 5, "other", "2026-11-17T12:00:00+00:00")],
                "members": [],
                "has_more": true
            }));
        })
        .await;
    let c = client(&server.base_url());
    let (g, p) = (GuildId::new(1).unwrap(), ChannelId::new(5).unwrap());
    assert_eq!(
        c.find_threads(g, p, "x", a(42), long_ago()).await,
        Outcome::Unknown(crate::rest::UnknownCause::Timeout)
    );
    // Reaching threads archived before the ticket existed completes it.
    let since = time::macros::datetime!(2026-11-17 13:00 UTC);
    assert_eq!(
        c.find_threads(g, p, "x", a(42), since).await,
        Outcome::Done(Vec::new())
    );
    // A page with more to come but no thread to continue from.
    let empty = MockServer::start_async().await;
    empty
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/guilds/1/threads/active");
            then.status(200)
                .json_body(json!({"threads": [], "members": []}));
        })
        .await;
    empty
        .mock_async(|when, then| {
            when.method("GET")
                .path("/api/v10/channels/5/threads/archived/private");
            then.status(200)
                .json_body(json!({"threads": [], "members": [], "has_more": true}));
        })
        .await;
    assert_eq!(
        client(&empty.base_url())
            .find_threads(g, p, "x", a(42), long_ago())
            .await,
        Outcome::Unknown(crate::rest::UnknownCause::Malformed)
    );
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
        .find_threads(
            GuildId::new(1).unwrap(),
            ChannelId::new(5).unwrap(),
            name,
            a(42),
            long_ago(),
        )
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
            .find_threads(
                GuildId::new(1).unwrap(),
                ChannelId::new(5).unwrap(),
                "x",
                a(42),
                long_ago()
            )
            .await
            .refusal(),
        Some(RefusalKind::MissingAccess)
    );
}

#[tokio::test]
async fn thread_members_are_read_by_member_id() {
    let server = MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/channels/31/thread-members");
            then.status(200).json_body(json!([
                {"id": "31", "user_id": "7", "join_timestamp": "2026-11-17T12:00:00+00:00", "flags": 0},
                {"id": "31", "user_id": "9", "join_timestamp": "2026-11-17T12:00:00+00:00", "flags": 0}
            ]));
        })
        .await;
    let client = client(&server.base_url());
    let members = client
        .thread_members(ChannelId::new(31).unwrap())
        .await
        .done()
        .unwrap();
    assert_eq!(
        members.iter().map(|m| m.get()).collect::<Vec<_>>(),
        vec![7, 9]
    );
}
