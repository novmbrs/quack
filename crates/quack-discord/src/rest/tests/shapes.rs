//! What each call sends: method, path, headers, and JSON body.

use httpmock::MockServer;
use quack_core::ids::{AccountId, ChannelId, GuildId, MemberId, RoleId};
use quack_core::permissions::{Overwrite, OverwriteTarget, Permissions};
use serde_json::json;
use time::macros::datetime;

use super::client;
use crate::fixtures;
use crate::model::ChannelSpec;
use crate::rest::Outcome;

fn g(id: u64) -> GuildId {
    GuildId::new(id).unwrap()
}
fn m(id: u64) -> MemberId {
    MemberId::new(id).unwrap()
}
fn c(id: u64) -> ChannelId {
    ChannelId::new(id).unwrap()
}

#[tokio::test]
async fn moderation_actions_send_the_audit_log_reason() {
    let server = MockServer::start_async().await;
    let reason = "Quack%20case%20%234%3A%20Spam%20%26%20scams";
    let kick = server
        .mock_async(|when, then| {
            when.method("DELETE")
                .path("/api/v10/guilds/1/members/2")
                .header("authorization", "Bot TOKEN")
                .header("x-audit-log-reason", reason)
                .header_prefix("user-agent", "DiscordBot (");
            then.status(204);
        })
        .await;
    let timeout = server
        .mock_async(|when, then| {
            when.method("PATCH")
                .path("/api/v10/guilds/1/members/3")
                .header("x-audit-log-reason", reason)
                .json_body(json!({"communication_disabled_until": "2026-11-17T12:00:00Z"}));
            then.status(200).json_body(fixtures::member(3, &[]));
        })
        .await;
    let untimeout = server
        .mock_async(|when, then| {
            when.method("PATCH")
                .path("/api/v10/guilds/1/members/4")
                .json_body(json!({"communication_disabled_until": null}));
            then.status(200).json_body(fixtures::member(4, &[]));
        })
        .await;
    let ban = server
        .mock_async(|when, then| {
            when.method("PUT")
                .path("/api/v10/guilds/1/bans/5")
                .header("x-audit-log-reason", reason)
                .json_body(json!({"delete_message_seconds": 604_800}));
            then.status(204);
        })
        .await;
    let unban = server
        .mock_async(|when, then| {
            when.method("DELETE")
                .path("/api/v10/guilds/1/bans/6")
                .header("x-audit-log-reason", "Quack%20case%20%234%20voided");
            then.status(204);
        })
        .await;
    let client = client(&server.base_url());
    let why = "Quack case #4: Spam & scams";
    assert_eq!(client.kick(g(1), m(2), why).await, Outcome::Done(()));
    let until = datetime!(2026-11-17 12:00 UTC);
    assert_eq!(
        client.timeout(g(1), m(3), until, why).await,
        Outcome::Done(())
    );
    assert_eq!(
        client.remove_timeout(g(1), m(4), why).await,
        Outcome::Done(())
    );
    assert_eq!(
        client.ban_member(g(1), m(5), 999_999, why).await,
        Outcome::Done(())
    );
    assert_eq!(
        client.unban(g(1), m(6), "Quack case #4 voided").await,
        Outcome::Done(())
    );
    for mock in [kick, timeout, untimeout, ban, unban] {
        mock.assert_async().await;
    }
}

#[tokio::test]
async fn reads_answer_absent_only_for_their_unknown_code() {
    let server = MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/guilds/1/members/2");
            then.status(200).json_body(fixtures::member(2, &[7]));
        })
        .await;
    server
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/guilds/1/members/3");
            then.status(404)
                .json_body(json!({"code": 10007, "message": "Unknown Member"}));
        })
        .await;
    server
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/guilds/1/members/4");
            then.status(404)
                .json_body(json!({"code": 10013, "message": "Unknown User"}));
        })
        .await;
    server
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/guilds/1/bans/2");
            then.status(200).json_body(
                json!({"reason": "Quack case #3: Spam", "user": fixtures::user(2, "x")}),
            );
        })
        .await;
    server
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/guilds/1/bans/3");
            then.status(404)
                .json_body(json!({"code": 10026, "message": "Unknown Ban"}));
        })
        .await;
    let client = client(&server.base_url());
    let Outcome::Done(Some(member)) = client.member(g(1), m(2)).await else {
        panic!("expected a member");
    };
    assert_eq!(member.role_ids, vec![RoleId::new(7).unwrap()]);
    assert_eq!(member.joined_at, Some(datetime!(2024-01-02 03:04:05 UTC)));
    assert_eq!(client.member(g(1), m(3)).await, Outcome::Done(None));
    assert_eq!(
        client.member(g(1), m(4)).await.refusal(),
        Some(crate::RefusalKind::UnknownUser)
    );
    let ban = client.ban(g(1), m(2)).await.done().flatten().unwrap();
    assert_eq!(ban.case_marker(), Some(3));
    assert_eq!(client.ban(g(1), m(3)).await, Outcome::Done(None));
}

#[tokio::test]
async fn guild_facts_compute_quacks_permissions() {
    let server = MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/guilds/1");
            then.status(200).json_body(fixtures::guild(
                1,
                8,
                &[fixtures::role(1, 0, 1024), fixtures::role(20, 5, 4)],
            ));
        })
        .await;
    server
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/guilds/1/members/50");
            then.status(200).json_body(fixtures::member(50, &[20]));
        })
        .await;
    let client = client(&server.base_url());
    let now = datetime!(2026-11-17 0:00 UTC);
    let facts = client
        .guild_facts(g(1), AccountId::new(50).unwrap(), now)
        .await
        .done()
        .unwrap();
    assert!(facts.mfa_required);
    assert_eq!(facts.owner_id, AccountId::new(8).unwrap());
    let quack = facts.quack.unwrap();
    assert_eq!(quack.highest_role_position, 5);
    assert_eq!(quack.permissions, Permissions(1024 | 4));
}

#[tokio::test]
async fn channels_and_threads() {
    let server = MockServer::start_async().await;
    let create = server
        .mock_async(|when, then| {
            when.method("POST")
                .path("/api/v10/guilds/1/channels")
                .json_body(json!({
                    "name": "moderation-log",
                    "type": 0,
                    "topic": "Quack posts here.",
                    "permission_overwrites": [
                        {"id": "1", "type": 0, "allow": "0", "deny": "1024"},
                        {"id": "50", "type": 1, "allow": "117760", "deny": "0"}
                    ]
                }));
            then.status(201).json_body(fixtures::channel(30, 1, 0));
        })
        .await;
    let thread = server
        .mock_async(|when, then| {
            when.method("POST")
                .path("/api/v10/channels/30/threads")
                .json_body(json!({
                    "name": "ticket-abc",
                    "type": 12,
                    "invitable": false,
                    "auto_archive_duration": 1440
                }));
            then.status(201).json_body(fixtures::channel(31, 1, 12));
        })
        .await;
    server
        .mock_async(|when, then| {
            when.method("GET")
                .path("/api/v10/channels/31/thread-members/7");
            then.status(404)
                .json_body(json!({"code": 10007, "message": "Unknown Member"}));
        })
        .await;
    let freeze = server
        .mock_async(|when, then| {
            when.method("PATCH")
                .path("/api/v10/channels/31")
                .json_body(json!({"archived": true, "locked": true}));
            then.status(200).json_body(fixtures::channel(31, 1, 12));
        })
        .await;
    server
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/channels/30");
            then.status(200).json_body(fixtures::channel(30, 1, 0));
        })
        .await;
    let client = client(&server.base_url());
    let spec = ChannelSpec {
        name: "moderation-log".into(),
        topic: Some("Quack posts here.".into()),
        parent_id: None,
        category: false,
        overwrites: vec![
            Overwrite {
                target: OverwriteTarget::Role(RoleId::new(1).unwrap()),
                allow: Permissions::NONE,
                deny: Permissions(1024),
            },
            Overwrite {
                target: OverwriteTarget::Member(m(50)),
                allow: Permissions(1024 | 2048 | 65536 | 16384 | 32768),
                deny: Permissions::NONE,
            },
        ],
    };
    assert_eq!(
        client.create_channel(g(1), &spec).await,
        Outcome::Done(c(30))
    );
    assert_eq!(
        client.create_private_thread(c(30), "ticket-abc").await,
        Outcome::Done(c(31))
    );
    assert_eq!(
        client
            .is_thread_member(c(31), AccountId::new(7).unwrap())
            .await,
        Outcome::Done(false)
    );
    assert!(client.set_thread_state(c(31), true, true).await.is_done());
    let fetched = client.channel(c(30)).await.done().unwrap();
    assert_eq!(fetched.overwrites.len(), 2);
    for mock in [create, thread, freeze] {
        mock.assert_async().await;
    }
}

#[tokio::test]
async fn dms_and_member_search() {
    let server = MockServer::start_async().await;
    let dm = server
        .mock_async(|when, then| {
            when.method("POST")
                .path("/api/v10/users/@me/channels")
                .json_body(json!({"recipient_id": "9"}));
            then.status(200)
                .json_body(json!({"id": "40", "type": 1, "recipients": [fixtures::user(9, "x")]}));
        })
        .await;
    let search = server
        .mock_async(|when, then| {
            when.method("GET")
                .path("/api/v10/guilds/1/members/search")
                .query_param("query", "a b")
                .query_param("limit", "5");
            then.status(200)
                .json_body(json!([fixtures::member(2, &[])]));
        })
        .await;
    let client = client(&server.base_url());
    assert_eq!(
        client.open_dm(AccountId::new(9).unwrap()).await,
        Outcome::Done(c(40))
    );
    assert_eq!(
        client
            .search_members(g(1), "a b", 5)
            .await
            .done()
            .map(|v| v.len()),
        Some(1)
    );
    dm.assert_async().await;
    search.assert_async().await;
}
