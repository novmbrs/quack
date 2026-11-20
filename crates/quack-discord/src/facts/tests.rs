use std::sync::Arc;

use httpmock::MockServer;
use quack_core::ids::{AccountId, ChannelId, GuildId, MemberId};
use quack_core::permissions::Permissions;
use serde_json::json;

use super::*;
use crate::cache::{GuildCache, MemberCache, feed_members};
use crate::fixtures;
use crate::rest::tests::client;

const GUILD: u64 = 1;
const QUACK: u64 = 50;
const OWNER: u64 = 8;
/// View Channel, Send Messages, Kick Members.
const QUACK_ROLE_PERMS: u64 = 1024 | 2048 | 2;

fn g() -> GuildId {
    GuildId::new(GUILD).unwrap()
}

fn roles() -> Vec<serde_json::Value> {
    vec![
        fixtures::role(GUILD, 0, 1024),
        fixtures::role(20, 5, QUACK_ROLE_PERMS),
    ]
}

/// Caches fed with `READY` and `GUILD_CREATE`: trusted for the server.
fn loaded() -> (Arc<GuildCache>, Arc<MemberCache>) {
    let guilds = Arc::new(GuildCache::new());
    let members = Arc::new(MemberCache::default());
    let quack = fixtures::member(QUACK, &[20]);
    let events = [
        fixtures::dispatch("READY", &fixtures::ready(QUACK, &[GUILD])),
        fixtures::dispatch(
            "GUILD_CREATE",
            &fixtures::guild_create(
                GUILD,
                OWNER,
                &roles(),
                &[
                    fixtures::channel_with(30, GUILD, 0, None, &[(GUILD, 0, 0, 1024)]),
                    fixtures::channel_with(31, GUILD, 0, None, &[]),
                ],
                &[quack, fixtures::member(7, &[])],
            ),
        ),
    ];
    for event in &events {
        guilds.update(event);
        feed_members(&members, event);
    }
    (guilds, members)
}

fn facts(base: &str, caches: (Arc<GuildCache>, Arc<MemberCache>)) -> Facts {
    Facts::new(client(base), caches.0, caches.1)
}

#[tokio::test]
async fn a_trusted_cache_answers_without_rest() {
    let server = MockServer::start_async().await;
    let any = server
        .mock_async(|when, then| {
            when.any_request();
            then.status(500);
        })
        .await;
    let facts = facts(&server.base_url(), loaded());
    assert_eq!(
        facts
            .quack_id()
            .await
            .done()
            .map(quack_core::ids::AccountId::get),
        Some(QUACK)
    );
    let guild = facts.guild_facts(g()).await.done().unwrap();
    assert_eq!(guild.owner_id.get(), OWNER);
    assert!(guild.mfa_required);
    let quack = guild.quack.unwrap();
    assert_eq!(quack.highest_role_position, 5);
    assert_eq!(quack.permissions, Permissions(QUACK_ROLE_PERMS | 1024));
    let member = facts
        .member(g(), MemberId::new(7).unwrap(), Fresh::Cached)
        .await;
    assert!(matches!(member, Outcome::Done(Membership::Present(m)) if m.id.get() == 7));
    let staff = facts
        .channel_facts(g(), ChannelId::new(30).unwrap())
        .await
        .done()
        .flatten()
        .unwrap();
    // @everyone loses View Channel, but Quack's role grants it again only
    // at server level, so the overwrite hides the channel from Quack.
    assert_eq!(staff.quack_permissions, Permissions::NONE);
    let channels = facts.guild_channels(g()).await.done().unwrap();
    assert_eq!(channels.len(), 2);
    assert_eq!(any.calls_async().await, 0);
}

#[tokio::test]
async fn rest_reads_bypass_the_cache_and_absent_is_never_cached() {
    let server = MockServer::start_async().await;
    let present = server
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/guilds/1/members/7");
            then.status(200).json_body(fixtures::member(7, &[20]));
        })
        .await;
    let absent = server
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/guilds/1/members/9");
            then.status(404)
                .json_body(json!({"code": 10007, "message": "Unknown Member"}));
        })
        .await;
    let search = server
        .mock_async(|when, then| {
            when.method("GET")
                .path("/api/v10/guilds/1/members/search")
                .query_param("query", "mem");
            then.status(200)
                .json_body(json!([fixtures::member(11, &[20])]));
        })
        .await;
    let caches = loaded();
    let members = caches.1.clone();
    let facts = facts(&server.base_url(), caches);
    let fresh = facts
        .member(g(), MemberId::new(7).unwrap(), Fresh::Rest)
        .await;
    assert!(matches!(fresh, Outcome::Done(Membership::Present(m)) if m.highest_role_position == 5));
    assert_eq!(present.calls_async().await, 1);
    for _ in 0..2 {
        assert_eq!(
            facts
                .member(g(), MemberId::new(9).unwrap(), Fresh::Cached)
                .await,
            Outcome::Done(Membership::Absent)
        );
    }
    assert_eq!(absent.calls_async().await, 2);
    assert!(members.get(g(), MemberId::new(9).unwrap(), true).is_none());
    let found = facts.search_members(g(), "mem", 5).await.done().unwrap();
    assert_eq!(
        found
            .iter()
            .map(|m| (m.id.get(), m.highest_role_position))
            .collect::<Vec<_>>(),
        vec![(11, 5)]
    );
    assert!(
        members
            .get(g(), MemberId::new(11).unwrap(), false)
            .is_some()
    );
    search.assert_async().await;
}

#[tokio::test]
async fn an_untrusted_cache_reads_everything_over_rest() {
    let server = MockServer::start_async().await;
    let me = server
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/users/@me");
            then.status(200).json_body(fixtures::user(QUACK, "Quack"));
        })
        .await;
    server
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/guilds/1");
            then.status(200)
                .json_body(fixtures::guild(GUILD, OWNER, &roles()));
        })
        .await;
    server
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/guilds/1/members/50");
            then.status(200).json_body(fixtures::member(QUACK, &[20]));
        })
        .await;
    server
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/channels/40");
            then.status(200)
                .json_body(fixtures::channel_with(40, GUILD, 12, Some(31), &[]));
        })
        .await;
    let parent = server
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/channels/31");
            then.status(200).json_body(fixtures::channel_with(
                31,
                GUILD,
                0,
                None,
                &[(QUACK, 1, 1 << 38, 0)],
            ));
        })
        .await;
    server
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/channels/41");
            then.status(404)
                .json_body(json!({"code": 10003, "message": "Unknown Channel"}));
        })
        .await;
    server
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/channels/42");
            then.status(200)
                .json_body(fixtures::channel_with(42, 99, 0, None, &[]));
        })
        .await;
    let facts = facts(
        &server.base_url(),
        (
            Arc::new(GuildCache::new()),
            Arc::new(MemberCache::default()),
        ),
    );
    let guild = facts.guild_facts(g()).await.done().unwrap();
    assert_eq!(guild.quack.map(|q| q.id.get()), Some(QUACK));
    let thread = facts
        .channel_facts(g(), ChannelId::new(40).unwrap())
        .await
        .done()
        .flatten()
        .unwrap();
    // A thread takes its parent's overwrites (Quack's member overwrite
    // grants Send Messages in Threads, bit 38).
    assert!(thread.quack_permissions.0 & (1 << 38) != 0);
    assert_eq!(parent.calls_async().await, 1);
    assert_eq!(
        facts.channel_facts(g(), ChannelId::new(41).unwrap()).await,
        Outcome::Done(None)
    );
    assert_eq!(
        facts.channel_facts(g(), ChannelId::new(42).unwrap()).await,
        Outcome::Done(None),
        "another server's channel is not this server's"
    );
    // Quack's id is read once.
    assert_eq!(me.calls_async().await, 1);
}

#[tokio::test]
async fn accounts_come_from_the_cache_and_rest_answers_fill_it() {
    let server = MockServer::start_async().await;
    let user = server
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/users/9");
            then.status(200).json_body(fixtures::user(9, "nine"));
        })
        .await;
    let facts = facts(&server.base_url(), loaded());
    let ids = [AccountId::new(9).unwrap(), AccountId::new(7).unwrap()];
    for _ in 0..2 {
        let found = facts.accounts(&ids).await.done().unwrap();
        let names: Vec<_> = found.iter().map(|a| a.username.as_str()).collect();
        assert_eq!(names, ["nine", "member"], "in the order asked");
    }
    // Member 7 came from GUILD_CREATE; 9 was read once, then cached.
    assert_eq!(user.calls_async().await, 1);
}
