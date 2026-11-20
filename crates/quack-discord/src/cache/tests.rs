use quack_core::ids::{ChannelId, GuildId, MemberId};
use quack_core::permissions::{Permission, Permissions};
use serde_json::json;
use time::OffsetDateTime;
use twilight_model::gateway::event::Event;

use super::*;
use crate::fixtures;

const QUACK: u64 = 50;

fn g(id: u64) -> GuildId {
    GuildId::new(id).unwrap()
}

fn now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

fn ready() -> Event {
    fixtures::dispatch("READY", &fixtures::ready(QUACK, &[1]))
}

/// Server 1: @everyone views; role 20 (position 5) may kick; channel 30
/// hides from @everyone and lets Quack's role back in; Quack has role 20.
fn guild_create() -> Event {
    fixtures::dispatch(
        "GUILD_CREATE",
        &fixtures::guild_create(
            1,
            8,
            &[fixtures::role(1, 0, 1024), fixtures::role(20, 5, 2)],
            &[
                fixtures::channel_with(30, 1, 0, None, &[(1, 0, 0, 1024), (20, 0, 1024, 0)]),
                fixtures::channel_with(31, 1, 4, None, &[]),
            ],
            &[fixtures::member(QUACK, &[20]), fixtures::member(7, &[])],
        ),
    )
}

fn feed(cache: &GuildCache, members: &MemberCache, events: &[Event]) {
    for event in events {
        cache.update(event);
        feed_members(members, event);
    }
}

fn loaded() -> (GuildCache, MemberCache) {
    let (cache, members) = (GuildCache::new(), MemberCache::default());
    feed(&cache, &members, &[ready(), guild_create()]);
    (cache, members)
}

fn role_update(id: u64, position: i64, permissions: u64) -> Event {
    fixtures::dispatch(
        "GUILD_ROLE_UPDATE",
        &json!({"guild_id": "1", "role": fixtures::role(id, position, permissions)}),
    )
}

fn quack_update(roles: &[&str]) -> Event {
    fixtures::dispatch(
        "GUILD_MEMBER_UPDATE",
        &json!({
            "guild_id": "1",
            "user": fixtures::user(QUACK, "Quack"),
            "roles": roles,
            "nick": null,
            "joined_at": "2024-01-02T03:04:05.000000+00:00",
            "premium_since": null,
            "avatar": null,
            "communication_disabled_until": null
        }),
    )
}

#[test]
fn a_lost_cache_update_waits_for_a_new_session() {
    let (cache, members) = loaded();
    assert!(cache.is_trusted(g(1)));
    cache.untrust(true);
    assert!(!cache.is_trusted(g(1)));
    cache.update(&Event::Resumed);
    assert!(!cache.is_trusted(g(1)), "a resume doesn't replay it");
    feed(&cache, &members, &[ready(), guild_create()]);
    assert!(cache.is_trusted(g(1)), "a new session rebuilt it");
    cache.untrust(false);
    cache.update(&Event::Resumed);
    assert!(
        cache.is_trusted(g(1)),
        "a broken connection only needs the resume"
    );
}

#[test]
fn trusted_only_while_connected_and_loaded() {
    let (cache, members) = (GuildCache::new(), MemberCache::default());
    assert!(!cache.is_trusted(g(1)));
    feed(&cache, &members, &[ready()]);
    assert!(cache.is_connected());
    assert_eq!(
        cache.quack_id().map(quack_core::ids::AccountId::get),
        Some(QUACK)
    );
    assert!(!cache.is_trusted(g(1)), "not loaded until GUILD_CREATE");
    assert!(cache.guild_facts(g(1), now()).is_none());
    feed(&cache, &members, &[guild_create()]);
    assert!(cache.is_trusted(g(1)));
    assert_eq!(cache.guild_count(), 1);
    cache.update(&Event::GatewayClose(None));
    assert!(!cache.is_trusted(g(1)));
    assert!(cache.guild_facts(g(1), now()).is_none());
    cache.update(&Event::Resumed);
    assert!(cache.is_trusted(g(1)), "a resume missed nothing");
    cache.update(&fixtures::dispatch(
        "GUILD_DELETE",
        &json!({"id": "1", "unavailable": true}),
    ));
    assert!(!cache.is_trusted(g(1)), "an outage");
    feed(&cache, &members, &[guild_create(), ready()]);
    assert!(
        !cache.is_trusted(g(1)),
        "a new session reloads every server"
    );
}

#[test]
fn facts_follow_role_and_member_updates() {
    let (cache, members) = loaded();
    let facts = cache.guild_facts(g(1), now()).unwrap();
    assert_eq!(facts.owner_id.get(), 8);
    let quack = facts.quack.unwrap();
    assert_eq!(quack.highest_role_position, 5);
    assert_eq!(quack.permissions, Permissions(1024 | 2));
    feed(&cache, &members, &[role_update(20, 6, 8)]);
    let quack = cache.guild_facts(g(1), now()).unwrap().quack.unwrap();
    assert_eq!(quack.highest_role_position, 6);
    assert!(quack.is_administrator());
    feed(&cache, &members, &[quack_update(&[])]);
    let quack = cache.guild_facts(g(1), now()).unwrap().quack.unwrap();
    assert_eq!(
        (quack.highest_role_position, quack.permissions),
        (0, Permissions(1024))
    );
    // A deleted role no longer counts.
    feed(
        &cache,
        &members,
        &[
            quack_update(&["20"]),
            fixtures::dispatch(
                "GUILD_ROLE_DELETE",
                &json!({"guild_id": "1", "role_id": "20"}),
            ),
        ],
    );
    let quack = cache.guild_facts(g(1), now()).unwrap().quack.unwrap();
    assert_eq!(quack.role_ids, Vec::<quack_core::ids::RoleId>::new());
}

#[test]
fn channels_and_threads() {
    let (cache, members) = loaded();
    let staff = cache
        .channel_facts(g(1), ChannelId::new(30).unwrap(), now())
        .unwrap()
        .unwrap();
    assert!(staff.quack_permissions.allows(Permission::ViewChannel));
    assert_eq!(staff.overwrites.len(), 2);
    let thread = json!({
        "id": "40", "guild_id": "1", "type": 12, "name": "ticket-x", "parent_id": "30",
        "thread_metadata": {
            "archived": false, "auto_archive_duration": 1440,
            "archive_timestamp": "2026-11-17T12:00:00+00:00", "locked": false
        }
    });
    feed(
        &cache,
        &members,
        &[fixtures::dispatch("THREAD_CREATE", &thread)],
    );
    let in_thread = cache
        .channel_facts(g(1), ChannelId::new(40).unwrap(), now())
        .unwrap()
        .unwrap();
    assert_eq!(in_thread.parent_id, Some(ChannelId::new(30).unwrap()));
    assert_eq!(in_thread.quack_permissions, staff.quack_permissions);
    let channels = cache.guild_channels(g(1), now()).unwrap();
    assert_eq!(
        channels
            .iter()
            .map(|c| c.id.get())
            .collect::<Vec<_>>()
            .len(),
        2,
        "threads are not listed"
    );
    feed(
        &cache,
        &members,
        &[fixtures::dispatch(
            "CHANNEL_DELETE",
            &fixtures::channel_with(30, 1, 0, None, &[]),
        )],
    );
    assert_eq!(
        cache.channel_facts(g(1), ChannelId::new(30).unwrap(), now()),
        Some(None)
    );
    assert_eq!(
        cache.channel_facts(g(2), ChannelId::new(31).unwrap(), now()),
        None,
        "another server is not loaded"
    );
}

#[test]
fn the_member_cache_follows_events() {
    let (cache, members) = loaded();
    let seven = MemberId::new(7).unwrap();
    assert!(members.get(g(1), seven, cache.is_trusted(g(1))).is_some());
    let mut added = fixtures::member(9, &[20]);
    added["guild_id"] = json!("1");
    feed(
        &cache,
        &members,
        &[fixtures::dispatch("GUILD_MEMBER_ADD", &added)],
    );
    assert_eq!(
        members
            .get(g(1), MemberId::new(9).unwrap(), true)
            .map(|m| m.role_ids.len()),
        Some(1)
    );
    feed(
        &cache,
        &members,
        &[fixtures::dispatch(
            "GUILD_MEMBER_REMOVE",
            &json!({"guild_id": "1", "user": fixtures::user(7, "u")}),
        )],
    );
    assert!(members.get(g(1), seven, true).is_none());
    feed(
        &cache,
        &members,
        &[fixtures::dispatch("GUILD_DELETE", &json!({"id": "1"}))],
    );
    assert!(members.get(g(1), MemberId::new(9).unwrap(), true).is_none());
    feed(&cache, &members, &[guild_create(), ready()]);
    assert!(members.is_empty(), "READY starts from scratch");
}

#[test]
fn members_interaction_authors_and_message_authors_refresh_accounts() {
    let (_, members) = loaded();
    let accounts = members.accounts();
    let account = |id| accounts.get(quack_core::ids::AccountId::new(id).unwrap());
    assert_eq!(account(7).map(|a| a.username).as_deref(), Some("member"));
    let mut message = fixtures::message(1, 30);
    message["guild_id"] = json!("1");
    let mut webhook = fixtures::message(2, 30);
    webhook["author"] = fixtures::user(11, "hook");
    webhook["webhook_id"] = json!("11");
    let interaction = fixtures::interaction(2, &json!({"id": "3", "name": "case", "type": 1}));
    let events = [
        fixtures::dispatch("MESSAGE_CREATE", &message),
        fixtures::dispatch("MESSAGE_CREATE", &webhook),
        fixtures::dispatch("INTERACTION_CREATE", &interaction),
    ];
    for event in &events {
        feed_members(&members, event);
    }
    assert_eq!(account(9).map(|a| a.username).as_deref(), Some("author"));
    assert_eq!(account(11), None, "a webhook's author isn't an account");
    assert_eq!(account(7).map(|a| a.username).as_deref(), Some("moderator"));
}
