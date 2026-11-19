//! Delivery (§13.3.2): the bounded queue and its counted drops, routes
//! re-read at delivery, the staff-only check, retries, losing the route
//! channel, other events, settings, shutdown, and cross-server isolation.

mod common;

use std::time::Duration;

use anyhow::{Result, bail};
use common::{world, world_with};
use quack_activity_log::{
    ActivitySettingsInput, BanChange, BanEntry, ChannelChange, Delivered, Limits, MemberJoined,
    MemberLeft,
};
use quack_core::modules::{ActivityEvent, ModuleDisabledReason, ModuleProblem};
use quack_core::permissions::ChannelProblem;
use quack_engine::Origin;
use quack_engine::settings::{FieldProblem, SettingsError, set_enabled};
use quack_testkit::fake::{Call, Script};
use quack_testkit::{ADMIN, MEMBER, sf};
use quack_text::model::ChannelRef;
use time::OffsetDateTime;

#[tokio::test]
async fn a_full_queue_drops_and_never_blocks() -> Result<()> {
    let w = world_with(Limits {
        queue: 2,
        ..Limits::default()
    })
    .await?;
    let started = std::time::Instant::now();
    for _ in 0..5 {
        w.log.on_server_changed(w.h.guild);
    }
    assert!(
        started.elapsed() < Duration::from_millis(100),
        "never waits"
    );
    assert_eq!(w.log.queued(), 2);
    assert_eq!(w.drain().await, [Delivered::Posted, Delivered::Posted]);
    Ok(())
}

#[tokio::test]
async fn routes_are_read_again_at_delivery() -> Result<()> {
    let w = world().await?;
    w.log.on_server_changed(w.h.guild);
    let admin = w.h.principal(ADMIN, Origin::Dashboard);
    let current = quack_activity_log::settings(&w.h.engine, &admin).await?;
    let mut routes = current.routes.clone();
    routes.remove(&ActivityEvent::ServerChanged);
    quack_activity_log::update_settings(
        &w.h.engine,
        &admin,
        &ActivitySettingsInput {
            expected_version: current.version,
            include_message_content: true,
            include_file_names: true,
            include_embed_types: true,
            routes,
        },
        &w.h.key("PUT"),
    )
    .await?;
    assert_eq!(w.drain().await, [Delivered::Dropped], "the route is gone");
    // Turning the module off drops queued posts and empties the cache.
    w.log.on_message_created(&w.created(1, "cached"));
    w.log.on_member_left(&MemberLeft {
        guild: w.h.guild,
        username: "duck".to_owned(),
        id: sf(MEMBER),
    });
    assert_eq!(w.log.cached(), 1);
    set_enabled(
        &w.h.engine,
        &admin,
        &quack_activity_log::hooks(),
        false,
        &w.h.key("off"),
    )
    .await?;
    w.reload().await?;
    assert_eq!(w.drain().await, [Delivered::Dropped]);
    w.log.on_server_changed(w.h.guild);
    assert_eq!(w.log.queued(), 0);
    assert_eq!(w.log.cached(), 0);
    Ok(())
}

#[tokio::test]
async fn a_destination_that_is_no_longer_staff_only_gets_nothing() -> Result<()> {
    let w = world().await?;
    w.h.fake.world(|world| {
        if let Some(c) = world
            .guilds
            .get_mut(&w.h.guild)
            .and_then(|g| g.channels.get_mut(&w.route))
        {
            c.overwrites.clear();
        }
    });
    w.log.on_server_changed(w.h.guild);
    assert_eq!(w.drain().await, [Delivered::Failed]);
    assert_eq!(w.posted().len(), 0);
    Ok(())
}

#[tokio::test]
async fn a_destination_opened_after_a_post_gets_nothing_more() -> Result<()> {
    let w = world().await?;
    w.log.on_server_changed(w.h.guild);
    assert_eq!(w.drain().await, [Delivered::Posted]);
    w.h.fake.world(|world| {
        if let Some(c) = world
            .guilds
            .get_mut(&w.h.guild)
            .and_then(|g| g.channels.get_mut(&w.route))
        {
            c.overwrites.clear();
        }
    });
    w.log.on_server_changed(w.h.guild);
    assert_eq!(
        w.drain().await,
        [Delivered::Failed],
        "checked at every send"
    );
    assert_eq!(w.posted().len(), 1);
    Ok(())
}

#[tokio::test]
async fn lost_answers_retry_without_duplicates() -> Result<()> {
    let w = world().await?;
    w.h.fake.script(Call::Send, Script::LoseResponse);
    w.h.fake
        .script(Call::Send, Script::RetryAfter(Duration::from_millis(10)));
    w.log.on_server_changed(w.h.guild);
    assert_eq!(w.drain().await, [Delivered::Posted]);
    assert_eq!(w.posted().len(), 1, "the nonce returned the first post");
    // Out of tries.
    w.h.fake.script_n(Call::Send, 3, &Script::Unknown);
    w.log.on_server_changed(w.h.guild);
    assert_eq!(w.drain().await, [Delivered::Failed]);
    Ok(())
}

#[tokio::test]
async fn losing_the_only_route_channel_turns_the_module_off() -> Result<()> {
    let w = world().await?;
    w.h.fake
        .world(|world| world.delete_channel(w.h.guild, w.route));
    w.log.on_server_changed(w.h.guild);
    assert_eq!(w.drain().await, [Delivered::Failed]);
    let admin = w.h.principal(ADMIN, Origin::Dashboard);
    let s = quack_activity_log::settings(&w.h.engine, &admin).await?;
    assert!(!s.enabled);
    assert_eq!(
        s.disabled_reason,
        Some(ModuleDisabledReason::ChannelDeleted)
    );
    assert_eq!(s.routes.len(), 0);
    Ok(())
}

#[tokio::test]
async fn members_bans_and_channels() -> Result<()> {
    let w = world().await?;
    w.log.on_member_joined(&MemberJoined {
        guild: w.h.guild,
        member: w.person(MEMBER),
        account_created: OffsetDateTime::now_utc(),
    });
    let ban = BanEntry {
        guild: w.h.guild,
        change: BanChange::Banned,
        member: w.person(MEMBER),
        executor: w.person(quack_testkit::MODERATOR),
        by_quack: false,
        reason: Some("token=abc123 spam".to_owned()),
    };
    w.log.on_ban(&ban);
    w.log.on_ban(&BanEntry {
        by_quack: true,
        ..ban.clone()
    });
    w.log.on_channel(
        w.h.guild,
        &ChannelChange::Created(ChannelRef {
            id: w.general,
            name: Some("general".to_owned()),
        }),
    );
    w.log.on_channel(
        w.h.guild,
        &ChannelChange::Deleted(w.general, "old".to_owned()),
    );
    assert_eq!(w.drain().await.len(), 4, "Quack's own ban is not posted");
    let posts = w.posted().join("\n");
    assert!(posts.contains("joined the server"));
    assert!(posts.contains("was banned by"));
    assert!(!posts.contains("abc123"));
    assert!(posts.contains("was created"));
    assert!(posts.contains("was deleted"));
    Ok(())
}

#[tokio::test]
async fn routes_must_be_staff_only_and_on_needs_a_route() -> Result<()> {
    let w = world().await?;
    let admin = w.h.principal(ADMIN, Origin::Dashboard);
    let current = quack_activity_log::settings(&w.h.engine, &admin).await?;
    let mut routes = current.routes.clone();
    routes.insert(ActivityEvent::MemberJoined, w.general);
    let refused = quack_activity_log::update_settings(
        &w.h.engine,
        &admin,
        &ActivitySettingsInput {
            expected_version: current.version,
            include_message_content: true,
            include_file_names: true,
            include_embed_types: true,
            routes,
        },
        &w.h.key("PUT"),
    )
    .await;
    let Err(SettingsError::Invalid(problems)) = refused else {
        bail!("expected problems, got {refused:?}");
    };
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].0, "member_joined");
    assert!(
        matches!(&problems[0].1, FieldProblem::Channel(p) if p.contains(&ChannelProblem::EveryoneCanView))
    );
    let emptied = quack_activity_log::update_settings(
        &w.h.engine,
        &admin,
        &ActivitySettingsInput {
            expected_version: current.version,
            include_message_content: true,
            include_file_names: true,
            include_embed_types: true,
            routes: std::collections::BTreeMap::new(),
        },
        &w.h.key("PUT"),
    )
    .await;
    assert!(
        matches!(&emptied, Err(SettingsError::ModuleNotReady(p)) if p == &[ModuleProblem::ActivityNoRoute])
    );
    // A server with no routes can't turn it on.
    let other = quack_testkit::Harness::new().await?;
    let refused = set_enabled(
        &other.engine,
        &other.principal(ADMIN, Origin::Dashboard),
        &quack_activity_log::hooks(),
        true,
        &other.key("on"),
    )
    .await;
    assert!(
        matches!(&refused, Err(SettingsError::ModuleNotReady(p)) if p == &[ModuleProblem::ActivityNoRoute])
    );
    Ok(())
}

#[tokio::test]
async fn shutdown_flushes_batches_and_drains_the_queue() -> Result<()> {
    let w = world().await?;
    let tasks = w.log.spawn();
    w.log.on_message_deleted(w.h.guild, w.general, w.msg(5));
    w.log.on_server_changed(w.h.guild);
    w.log.shutdown(Duration::from_secs(5)).await;
    assert_eq!(w.log.queued(), 0);
    for task in tasks {
        task.abort();
    }
    let posts = w.posted();
    assert_eq!(posts.len(), 2, "{posts:?}");
    Ok(())
}

#[tokio::test]
async fn other_servers_are_separate() -> Result<()> {
    let w = world().await?;
    let other = sf(2002);
    w.log.on_message_created(&w.created(1, "ours"));
    // The same channel and message ids in a server without the module.
    let mut theirs = w.created(1, "theirs");
    theirs.guild = other;
    w.log.on_message_created(&theirs);
    w.log.on_message_deleted(other, w.general, w.msg(1));
    w.log.on_server_changed(other);
    assert_eq!(w.log.queued(), 0);
    assert_eq!(w.log.cached(), 1);
    w.log.on_message_deleted(w.h.guild, w.general, w.msg(1));
    w.drain().await;
    assert!(w.posted()[0].contains("ours"));
    w.log.on_message_created(&w.created(2, "kept"));
    w.log.forget_guild(other);
    assert_eq!(w.log.cached(), 1);
    w.log.forget_guild(w.h.guild);
    assert_eq!(w.log.cached(), 0);
    Ok(())
}

/// Shutdown waits for a post a sender has taken off the queue and is still
/// delivering, not only for the queue to empty (the drain dropped it).
#[tokio::test]
async fn shutdown_waits_for_a_post_in_flight() -> Result<()> {
    let w = world().await?;
    let gate = w.h.fake.hold(quack_testkit::fake::Call::Send);
    let tasks = w.log.spawn();
    w.log.on_server_changed(w.h.guild);
    gate.reached().await;
    assert_eq!(w.log.queued(), 0, "the post is in flight");
    let log = w.log.clone();
    let drain = tokio::spawn(async move { log.shutdown(Duration::from_secs(5)).await });
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !drain.is_finished(),
        "shutdown waits for the post in flight"
    );
    gate.release();
    drain.await?;
    for task in tasks {
        task.abort();
    }
    assert_eq!(w.posted().len(), 1);
    Ok(())
}
