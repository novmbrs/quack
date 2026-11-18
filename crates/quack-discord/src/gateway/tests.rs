use std::sync::{Arc, Mutex};
use std::time::Duration;

use quack_core::ids::{AccountId, ChannelId, GuildId, MessageId};
use serde_json::{Value, json};
use tokio::sync::Semaphore;

use super::dispatch::Dispatcher;
use super::event::map;
use super::*;
use crate::fixtures;

fn event(kind: &str, d: &Value) -> Option<Event> {
    map(&fixtures::dispatch(kind, d), 0)
}

fn g(id: u64) -> GuildId {
    GuildId::new(id).unwrap()
}

fn member_remove(guild: u64, user: u64) -> Event {
    Event::MemberRemove {
        guild_id: g(guild),
        account: crate::convert::user(&serde_json::from_value(fixtures::user(user, "u")).unwrap())
            .unwrap(),
    }
}

#[test]
fn ready_and_resumed() {
    let raw = fixtures::dispatch("READY", &fixtures::ready(50, &[1, 2]));
    let first = map(&raw, 0).unwrap();
    assert_eq!(
        first,
        Event::Ready {
            quack_id: AccountId::new(50).unwrap(),
            guild_ids: vec![g(1), g(2)],
            missed_events: false,
        }
    );
    assert!(matches!(
        map(&raw, 1),
        Some(Event::Ready {
            missed_events: true,
            ..
        })
    ));
    assert_eq!(event("RESUMED", &Value::Null), Some(Event::Resumed));
    assert_eq!(first.guild_id(), None);
    assert_eq!(first.name(), "READY");
}

#[test]
fn guild_lifecycle() {
    let create = event(
        "GUILD_CREATE",
        &fixtures::guild_create(
            1,
            8,
            &[fixtures::role(1, 0, 1024)],
            &[fixtures::channel_with(30, 1, 0, None, &[])],
            &[fixtures::member(50, &[])],
        ),
    )
    .unwrap();
    let Event::GuildCreate {
        guild,
        channel_ids,
        thread_ids,
    } = &create
    else {
        panic!("expected GUILD_CREATE, got {create:?}");
    };
    assert_eq!((guild.id, guild.owner_id.get()), (g(1), 8));
    assert!(guild.mfa_required);
    assert_eq!(*channel_ids, vec![ChannelId::new(30).unwrap()]);
    assert_eq!(*thread_ids, Vec::<ChannelId>::new());
    assert_eq!(
        event("GUILD_CREATE", &json!({"id": "2", "unavailable": true})),
        None
    );
    let mut renamed = fixtures::guild(1, 9, &[fixtures::role(1, 0, 0)]);
    renamed["name"] = json!("Renamed");
    assert!(matches!(
        event("GUILD_UPDATE", &renamed),
        Some(Event::GuildUpdate { guild }) if guild.name == "Renamed" && guild.owner_id.get() == 9
    ));
    assert_eq!(
        event("GUILD_DELETE", &json!({"id": "1", "unavailable": true})),
        Some(Event::GuildDelete {
            guild_id: g(1),
            unavailable: true
        })
    );
    assert_eq!(
        event("GUILD_DELETE", &json!({"id": "1"})),
        Some(Event::GuildDelete {
            guild_id: g(1),
            unavailable: false
        })
    );
    // Role events only update the guild cache.
    assert_eq!(
        event(
            "GUILD_ROLE_CREATE",
            &json!({"guild_id": "1", "role": fixtures::role(5, 1, 0)})
        ),
        None
    );
}

#[test]
fn channels_threads_and_members() {
    let channel = fixtures::channel_with(30, 1, 0, None, &[(1, 0, 0, 1024)]);
    for kind in ["CHANNEL_CREATE", "CHANNEL_UPDATE", "CHANNEL_DELETE"] {
        let mapped = event(kind, &channel).unwrap();
        assert_eq!(mapped.name(), kind);
        assert_eq!(mapped.guild_id(), Some(g(1)));
    }
    assert_eq!(
        event(
            "THREAD_DELETE",
            &json!({"id": "31", "guild_id": "1", "parent_id": "30", "type": 12})
        ),
        Some(Event::ThreadDelete {
            guild_id: g(1),
            thread_id: ChannelId::new(31).unwrap(),
            parent_id: ChannelId::new(30).unwrap(),
        })
    );
    let mut added = fixtures::member(9, &[20]);
    added["guild_id"] = json!("1");
    assert!(matches!(
        event("GUILD_MEMBER_ADD", &added),
        Some(Event::MemberAdd { guild_id, member }) if guild_id == g(1) && member.role_ids.len() == 1
    ));
    let updated = json!({
        "guild_id": "1",
        "user": fixtures::user(9, "member"),
        "roles": [],
        "nick": "New",
        "joined_at": "2024-01-02T03:04:05.000000+00:00",
        "premium_since": null,
        "avatar": null,
        "communication_disabled_until": "2030-01-01T00:00:00.000000+00:00"
    });
    assert!(matches!(
        event("GUILD_MEMBER_UPDATE", &updated),
        Some(Event::MemberUpdate { member, .. })
            if member.nick.as_deref() == Some("New") && member.timeout_until.is_some()
    ));
    assert_eq!(
        event(
            "GUILD_MEMBER_REMOVE",
            &json!({"guild_id": "1", "user": fixtures::user(9, "u")})
        ),
        Some(member_remove(1, 9))
    );
}

#[test]
fn messages() {
    let mut message = fixtures::message(10, 5);
    message["guild_id"] = json!("1");
    let created = event("MESSAGE_CREATE", &message).unwrap();
    let Event::MessageCreate(created) = created else {
        panic!("expected MESSAGE_CREATE");
    };
    assert_eq!(created.guild_id, Some(g(1)));
    assert_eq!(created.content, "hello");
    assert_eq!(created.attachments.len(), 1);
    assert!(matches!(
        event("MESSAGE_UPDATE", &message),
        Some(Event::MessageUpdate(m)) if m.id == MessageId::new(10).unwrap()
    ));
    assert_eq!(
        event(
            "MESSAGE_DELETE",
            &json!({"id": "10", "channel_id": "5", "guild_id": "1"})
        ),
        Some(Event::MessageDelete {
            guild_id: Some(g(1)),
            channel_id: ChannelId::new(5).unwrap(),
            message_id: MessageId::new(10).unwrap(),
        })
    );
    assert_eq!(
        event(
            "MESSAGE_DELETE_BULK",
            &json!({"ids": ["10", "11"], "channel_id": "5"})
        ),
        Some(Event::MessageDeleteBulk {
            guild_id: None,
            channel_id: ChannelId::new(5).unwrap(),
            message_ids: vec![MessageId::new(10).unwrap(), MessageId::new(11).unwrap()],
        })
    );
}

#[test]
fn bans_from_the_audit_log_and_interactions() {
    let entry = |action: u16| {
        json!({
            "id": "900",
            "guild_id": "1",
            "action_type": action,
            "target_id": "9",
            "user_id": "4",
            "reason": "spam"
        })
    };
    assert_eq!(
        event("GUILD_AUDIT_LOG_ENTRY_CREATE", &entry(22)),
        Some(Event::Ban {
            guild_id: g(1),
            change: BanChange::Banned,
            target: AccountId::new(9).unwrap(),
            executor: Some(AccountId::new(4).unwrap()),
            reason: Some("spam".into()),
        })
    );
    assert!(matches!(
        event("GUILD_AUDIT_LOG_ENTRY_CREATE", &entry(23)),
        Some(Event::Ban {
            change: BanChange::Unbanned,
            ..
        })
    ));
    assert_eq!(event("GUILD_AUDIT_LOG_ENTRY_CREATE", &entry(20)), None);
    let interaction = fixtures::interaction(2, &json!({"id": "101", "name": "rules", "type": 1}));
    let mapped = event("INTERACTION_CREATE", &interaction).unwrap();
    assert_eq!(mapped.guild_id(), Some(g(1)));
    assert!(
        matches!(mapped, Event::Interaction(i) if i.request == crate::interaction::Request::Rules)
    );
}

#[test]
fn an_unreadable_interaction_frame_is_recovered() {
    let mut payload = fixtures::interaction(2, &json!({"id": "101", "name": "rules", "type": 1}));
    payload
        .as_object_mut()
        .unwrap()
        .remove("attachment_size_limit");
    let frame = json!({"op": 0, "s": 2, "t": "INTERACTION_CREATE", "d": payload}).to_string();
    let error = twilight_gateway::parse(frame, wanted()).unwrap_err();
    assert!(matches!(recover(&error), Some(Event::Interaction(_))));
    let other = json!({"op": 0, "s": 2, "t": "MESSAGE_CREATE", "d": {"id": "x"}}).to_string();
    let error = twilight_gateway::parse(other, wanted()).unwrap_err();
    assert!(recover(&error).is_none());
}

#[test]
fn the_status_reports_connection_and_latency() {
    let status = GatewayStatus::default();
    assert!(!status.is_connected());
    assert_eq!(status.heartbeat_latency(), None);
    status.set_connected(true);
    assert!(status.is_connected());
}

#[tokio::test]
async fn one_servers_events_stay_in_order() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let dispatcher = Dispatcher::start(Arc::new(move |event: Event| {
        let log = log.clone();
        async move {
            if let Event::MemberRemove { account, .. } = event {
                tokio::time::sleep(Duration::from_millis(account.id.get() % 3)).await;
                log.lock().unwrap().push(account.id.get());
            }
        }
    }));
    for user in 1..=20 {
        assert!(dispatcher.dispatch(member_remove(7, user)).is_none());
    }
    let all_seen = async {
        while seen.lock().unwrap().len() < 20 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    };
    assert!(
        tokio::time::timeout(Duration::from_secs(5), all_seen)
            .await
            .is_ok()
    );
    dispatcher.stop().await;
    assert_eq!(*seen.lock().unwrap(), (1..=20).collect::<Vec<u64>>());
}

#[tokio::test]
async fn a_full_queue_drops_without_waiting() {
    let gate = Arc::new(Semaphore::new(0));
    let held = gate.clone();
    let dispatcher = Dispatcher::start(Arc::new(move |_event: Event| {
        let held = held.clone();
        async move {
            let _ = held.acquire().await;
        }
    }));
    let mut dropped = 0;
    for user in 1..=(EVENT_QUEUE_LEN as u64 + 10) {
        if dispatcher.dispatch(member_remove(4, user)).is_some() {
            dropped += 1;
        }
    }
    // At most one event left the queue for the (blocked) handler.
    assert!((9..=10).contains(&dropped), "dropped {dropped}");
    // Another server's queue is unaffected.
    assert!(dispatcher.dispatch(member_remove(5, 1)).is_none());
    gate.close();
    dispatcher.stop().await;
}

#[tokio::test]
async fn slow_handlers_never_hold_back_a_later_interaction_past_64() {
    // 100 handlers still busy after their acknowledgement: a later one
    // still starts (and so acknowledges) at once.
    let running = Arc::new(Semaphore::new(0));
    let gate = Arc::new(Semaphore::new(0));
    let (started, held) = (running.clone(), gate.clone());
    let dispatcher = Dispatcher::start(Arc::new(move |_event: Event| {
        let (started, held) = (started.clone(), held.clone());
        async move {
            started.add_permits(1);
            let _ = held.acquire().await;
        }
    }));
    let interaction = crate::interaction::decode_json(
        &fixtures::interaction(2, &json!({"id": "101", "name": "help", "type": 1})).to_string(),
    )
    .unwrap();
    for _ in 0..101 {
        assert!(
            dispatcher
                .dispatch(Event::Interaction(Box::new(interaction.clone())))
                .is_none()
        );
    }
    let reached = tokio::time::timeout(Duration::from_secs(5), running.acquire_many(101)).await;
    assert!(
        reached.is_ok(),
        "an interaction waited behind slow handlers"
    );
    gate.close();
    dispatcher.stop().await;
}

#[tokio::test]
async fn a_message_flood_never_drops_a_server_leaving() {
    let gate = Arc::new(Semaphore::new(0));
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (held, log) = (gate.clone(), seen.clone());
    let dispatcher = Dispatcher::start(Arc::new(move |event: Event| {
        let (held, log) = (held.clone(), log.clone());
        async move {
            if matches!(event, Event::MemberRemove { .. }) {
                let _ = held.acquire().await;
            }
            log.lock().unwrap().push(event.name());
        }
    }));
    // Server 5's queue is full of held events.
    let mut dropped = 0;
    for user in 0..(EVENT_QUEUE_LEN as u64 + 10) {
        dropped += usize::from(dispatcher.dispatch(member_remove(5, user + 1)).is_some());
    }
    assert!(dropped > 0, "the queue filled");
    let leave = event("GUILD_DELETE", &json!({"id": "5", "unavailable": false})).unwrap();
    assert!(dispatcher.dispatch(leave).is_none(), "never dropped");
    tokio::time::timeout(Duration::from_secs(5), async {
        while !seen.lock().unwrap().contains(&"GUILD_DELETE") {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("handled while the flood waits");
    gate.close();
    dispatcher.stop().await;
}

#[tokio::test]
async fn an_interaction_waiting_for_a_slot_never_starts_after_shutdown() {
    let started = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let gate = Arc::new(Semaphore::new(0));
    let (count, held) = (started.clone(), gate.clone());
    let dispatcher = Dispatcher::start(Arc::new(move |_event: Event| {
        let (count, held) = (count.clone(), held.clone());
        async move {
            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let _ = held.acquire().await;
        }
    }));
    let interaction = crate::interaction::decode_json(
        &fixtures::interaction(2, &json!({"id": "101", "name": "help", "type": 1})).to_string(),
    )
    .unwrap();
    for _ in 0..=MAX_INTERACTION_HANDLERS {
        let mut event = Event::Interaction(Box::new(interaction.clone()));
        while let Some(back) = dispatcher.dispatch(event) {
            event = back;
            tokio::task::yield_now().await;
        }
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        while started.load(std::sync::atomic::Ordering::SeqCst) < MAX_INTERACTION_HANDLERS {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    // One more waits for a slot; shutdown begins, then the slots free up.
    let stopping = tokio::spawn(dispatcher.stop());
    tokio::time::sleep(Duration::from_millis(50)).await;
    gate.add_permits(10_000);
    stopping.await.unwrap();
    assert_eq!(
        started.load(std::sync::atomic::Ordering::SeqCst),
        MAX_INTERACTION_HANDLERS
    );
}

#[tokio::test]
async fn interactions_run_concurrently_up_to_the_limit() {
    let running = Arc::new(Semaphore::new(0));
    let gate = Arc::new(Semaphore::new(0));
    let (started, held) = (running.clone(), gate.clone());
    let dispatcher = Dispatcher::start(Arc::new(move |_event: Event| {
        let (started, held) = (started.clone(), held.clone());
        async move {
            started.add_permits(1);
            let _ = held.acquire().await;
        }
    }));
    let interaction = crate::interaction::decode_json(
        &fixtures::interaction(2, &json!({"id": "101", "name": "rules", "type": 1})).to_string(),
    )
    .unwrap();
    for _ in 0..(MAX_INTERACTION_HANDLERS + 5) {
        let mut event = Event::Interaction(Box::new(interaction.clone()));
        // The queue holds 256: let the pool take some when it is full.
        while let Some(back) = dispatcher.dispatch(event) {
            event = back;
            tokio::task::yield_now().await;
        }
    }
    let all = u32::try_from(MAX_INTERACTION_HANDLERS).unwrap();
    let reached = tokio::time::timeout(Duration::from_secs(5), running.acquire_many(all)).await;
    assert!(reached.is_ok(), "the handlers didn't all start");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        running.available_permits(),
        0,
        "more than the limit started"
    );
    gate.close();
    dispatcher.stop().await;
}

#[tokio::test]
async fn more_than_one_recommended_shard_stops_startup() {
    let server = httpmock::MockServer::start_async().await;
    let answer = server
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/gateway/bot");
            then.status(200).json_body(json!({
                "url": "wss://gateway.discord.gg",
                "shards": 1,
                "session_start_limit": {
                    "total": 1000, "remaining": 999, "reset_after": 1000, "max_concurrency": 1
                }
            }));
        })
        .await;
    let client = crate::rest::tests::client(&server.base_url());
    assert!(check_shards(&client).await.is_ok());
    answer.delete_async().await;
    server
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/gateway/bot");
            then.status(200).json_body(json!({
                "url": "wss://gateway.discord.gg",
                "shards": 3,
                "session_start_limit": {
                    "total": 1000, "remaining": 999, "reset_after": 1000, "max_concurrency": 1
                }
            }));
        })
        .await;
    assert!(matches!(
        check_shards(&client).await,
        Err(GatewayError::TooManyShards(3))
    ));
}

/// Discord review BLOCKER 1: an unreadable frame is described by a closed
/// kind and its event name only, never its token or text.
#[test]
fn an_unreadable_frame_is_described_without_its_content() {
    let frame = r#"{"t":"INTERACTION_CREATE","s":3,"op":0,"d":{"token":"sentinel-interaction-token","data":{"components":[{"value":"private member text"}]}}}"#;
    let kind = twilight_gateway::error::ReceiveMessageErrorType::Deserializing {
        event: frame.to_owned(),
    };
    let (what, event) = describe(&kind);
    let logged = format!("{what} {event:?}");
    assert_eq!(logged, "deserializing Some(\"INTERACTION_CREATE\")");
    assert!(!logged.contains("sentinel") && !logged.contains("private"));
    // A frame whose `t` isn't a plain event name logs no name at all.
    assert_eq!(event_name(r#"{"t":"secret token here"}"#), None);
    assert_eq!(event_name("not json sentinel"), None);
}
