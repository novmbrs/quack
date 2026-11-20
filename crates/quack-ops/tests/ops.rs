//! Ops in Discord (§21) over PostgreSQL and the fake Discord: channels,
//! durable posts, the run before, alerts, the board, the error feed, and
//! the `/ops` answers.

#![allow(clippy::unwrap_used, reason = "tests")]

use anyhow::Result;
use quack_core::discord::RefusalKind;
use quack_core::ids::{AccountId, ChannelId, GuildId, JobId};
use quack_core::permissions::{Overwrite, OverwriteTarget, Permission, Permissions};
use quack_ops::{Ops, OpsConfig, StopReason};
use quack_telemetry::errors::{ErrorEvent, push};
use quack_testkit::fake::{Call, Recorded, Script};
use quack_testkit::rules_for_tests::{BAN, one};
use quack_testkit::{Harness, MEMBER, OWNER, sf};

fn new_ops(h: &Harness) -> Ops {
    let ops = Ops::new(
        h.engine.clone(),
        OpsConfig {
            guild: h.guild,
            operators: vec![sf(OWNER)],
            trace_url: Some("https://grafana.test/trace/{trace_id}".to_owned()),
            version: "9.9.9".to_owned(),
            commit: None,
        },
    );
    h.register(ops.jobs());
    ops
}

/// The plain text of every message sent to `channel`.
fn sent_to(h: &Harness, channel: Option<ChannelId>) -> Vec<String> {
    h.fake
        .calls_of(Call::Send)
        .into_iter()
        .filter(|r| r.channel == channel)
        .filter_map(|r: Recorded| r.msg.map(|m| m.plain()))
        .collect()
}

#[tokio::test]
async fn channels_are_created_private_and_adopted_next_time() -> Result<()> {
    let h = Harness::new().await?;
    let ops = new_ops(&h);
    let channels = ops.prepare_channels().await.unwrap();
    assert_eq!(
        h.fake.count(Call::CreateChannel),
        6,
        "a category and five channels"
    );
    let (category, everyone_hidden) = h.fake.world(|w| {
        let g = &w.guilds[&h.guild];
        let alerts = &g.channels[&channels.alerts.unwrap()];
        let category = alerts.parent_id.unwrap();
        let hidden = alerts.overwrites.iter().any(|o| {
            o.target == OverwriteTarget::Role(sf(h.guild.get()))
                && o.deny.contains(Permission::ViewChannel.into())
        });
        (g.channels[&category].clone(), hidden)
    });
    assert_eq!(category.kind, 4);
    assert_eq!(category.name, "Ops");
    assert!(everyone_hidden);

    let again = new_ops(&h).prepare_channels().await.unwrap();
    assert_eq!(again, channels, "the same channels are adopted");
    assert_eq!(
        h.fake.count(Call::CreateChannel),
        6,
        "nothing new is created"
    );
    Ok(())
}

#[tokio::test]
async fn an_adopted_public_channel_is_never_posted_to() -> Result<()> {
    let h = Harness::new().await?;
    h.fake.world(|w| {
        let category = w.add_text_channel(h.guild, false);
        let public = w.add_text_channel(h.guild, false);
        let g = w.guilds.get_mut(&h.guild).unwrap();
        let c = g.channels.get_mut(&category).unwrap();
        c.kind = 4;
        c.name = "ops".to_owned();
        let p = g.channels.get_mut(&public).unwrap();
        p.name = "ops-alerts".to_owned();
        p.parent_id = Some(category);
        p.overwrites = vec![Overwrite {
            target: OverwriteTarget::Role(sf(h.guild.get())),
            allow: Permission::ViewChannel.into(),
            deny: Permissions::NONE,
        }];
    });
    let channels = new_ops(&h).prepare_channels().await.unwrap();
    assert_eq!(channels.alerts, None);
    assert!(channels.deploys.is_some());
    assert_eq!(
        h.fake.count(Call::CreateChannel),
        4,
        "only the missing four"
    );
    Ok(())
}

#[tokio::test]
async fn each_start_posts_once_and_says_how_the_run_before_ended() -> Result<()> {
    let h = Harness::new().await?;
    let ops = new_ops(&h);
    let channels = ops.prepare_channels().await.unwrap();

    ops.record_start(1, std::time::Duration::from_secs(4))
        .await?;
    h.settle().await?;
    ops.record_stop(StopReason::Signal).await;
    ops.record_start(0, std::time::Duration::from_secs(4))
        .await?;
    h.settle().await?;
    // No stop recorded: a crash.
    ops.record_start(0, std::time::Duration::from_secs(4))
        .await?;
    h.settle().await?;

    let posts = sent_to(&h, channels.deploys);
    assert_eq!(posts.len(), 3, "{posts:#?}");
    assert!(
        posts[0].contains("first run ops knows about"),
        "{}",
        posts[0]
    );
    assert!(posts[1].contains("stopped cleanly"), "{}", posts[1]);
    assert!(posts[2].contains("ended without stopping"), "{}", posts[2]);
    assert_eq!(
        h.db.count("SELECT count(*) FROM ops_posts").await?,
        0,
        "posted rows are deleted"
    );
    Ok(())
}

#[tokio::test]
async fn posts_wait_for_the_channels() -> Result<()> {
    let h = Harness::new().await?;
    let ops = new_ops(&h);
    ops.server_joined(h.guild, false).await;
    h.run_jobs().await?;
    assert_eq!(h.fake.count(Call::Send), 0);
    assert_eq!(
        h.db.count("SELECT count(*) FROM ops_posts").await?,
        1,
        "still waiting"
    );

    let channels = ops.prepare_channels().await.unwrap();
    h.fast_forward().await?;
    h.settle().await?;
    let posts = sent_to(&h, channels.installs);
    assert_eq!(posts.len(), 1);
    assert!(posts[0].contains("Quack joined a server"), "{}", posts[0]);
    Ok(())
}

async fn dead_job(h: &Harness) -> Result<JobId> {
    h.db.execute(&format!(
        "INSERT INTO jobs (kind, key, guild_id, payload, max_attempts, attempts, dead_at, last_error)
         VALUES ('send_dm', 'send_dm:test', {}, '{{}}', 3, 3, now(), 'discord_unavailable')",
        h.guild.get()
    ))
    .await?;
    let id: uuid::Uuid = sqlx::query_scalar("SELECT id FROM jobs WHERE key = 'send_dm:test'")
        .fetch_one(h.engine.db().pool())
        .await?;
    Ok(JobId::from_uuid(id))
}

#[tokio::test]
async fn a_dead_job_is_posted_once_and_can_be_requeued() -> Result<()> {
    let h = Harness::new().await?;
    let ops = new_ops(&h);
    let channels = ops.prepare_channels().await.unwrap();
    let job = dead_job(&h).await?;

    ops.check_alerts().await?;
    ops.check_alerts().await?;
    h.settle().await?;
    let posts = sent_to(&h, channels.alerts);
    assert_eq!(posts.len(), 1, "{posts:#?}");
    assert!(posts[0].contains("send_dm job died"), "{}", posts[0]);

    let by: AccountId = sf(OWNER);
    let done = ops.requeue(job, by).await?.unwrap();
    assert!(done.plain().contains("requeued it"));
    let dead =
        h.db.count("SELECT count(*) FROM jobs WHERE key = 'send_dm:test' AND dead_at IS NOT NULL")
            .await?;
    assert_eq!(dead, 0);
    assert!(
        ops.requeue(job, by).await?.is_err(),
        "it isn't dead any more"
    );
    assert!(ops.dismiss(job, by).await?.is_err());
    Ok(())
}

#[tokio::test]
async fn a_dead_job_can_be_dismissed_and_listed() -> Result<()> {
    let h = Harness::new().await?;
    let ops = new_ops(&h);
    let job = dead_job(&h).await?;
    let list = ops.dead_jobs(1).await?.plain();
    assert!(list.contains("1 dead job"), "{list}");
    assert!(list.contains("send_dm:test"), "{list}");
    assert!(ops.dismiss(job, sf(OWNER)).await?.is_ok());
    assert_eq!(
        h.db.count("SELECT count(*) FROM jobs WHERE key = 'send_dm:test'")
            .await?,
        0
    );
    assert!(ops.dead_jobs(1).await?.plain().contains("No dead jobs"));
    Ok(())
}

#[tokio::test]
async fn failed_actions_are_summarized_once() -> Result<()> {
    let h = Harness::new().await?;
    let ops = new_ops(&h);
    let channels = ops.prepare_channels().await.unwrap();
    let rule = h.rule(one("Spam", BAN, false)).await?;
    h.apply(MEMBER, rule).await?;
    h.fake
        .script(Call::Ban, Script::refuse(RefusalKind::MissingPermissions));
    h.settle().await?;
    let failed =
        h.db.count("SELECT count(*) FROM actions WHERE state = 'failed'")
            .await?;
    assert_eq!(failed, 1);

    ops.check_alerts().await?;
    ops.check_alerts().await?;
    h.settle().await?;
    let posts = sent_to(&h, channels.alerts);
    assert_eq!(posts.len(), 1, "{posts:#?}");
    assert!(posts[0].contains("1 action failed"), "{}", posts[0]);
    assert!(posts[0].contains("discord_refused"), "{}", posts[0]);
    Ok(())
}

#[tokio::test]
async fn the_board_is_edited_in_place_and_posted_again_when_gone() -> Result<()> {
    let h = Harness::new().await?;
    let ops = new_ops(&h);
    let channels = ops.prepare_channels().await.unwrap();
    ops.update_board().await?;
    ops.update_board().await?;
    assert_eq!(sent_to(&h, channels.status).len(), 1);
    assert_eq!(h.fake.count(Call::Edit), 1);
    let board = sent_to(&h, channels.status).remove(0);
    assert!(board.contains("Quack status"), "{board}");
    assert!(board.contains("9.9.9"), "{board}");

    h.fake.world(|w| w.messages.clear());
    ops.update_board().await?;
    assert_eq!(sent_to(&h, channels.status).len(), 2, "posted again");
    Ok(())
}

#[tokio::test]
async fn errors_are_grouped_into_one_post() -> Result<()> {
    let h = Harness::new().await?;
    let ops = new_ops(&h);
    let channels = ops.prepare_channels().await.unwrap();
    for _ in 0..3 {
        push(ErrorEvent {
            target: "quack_engine::dms".to_owned(),
            message: "couldn't send".to_owned(),
            trace_id: Some("abc123".to_owned()),
        });
    }
    ops.flush_errors().await;
    ops.flush_errors().await;
    let posts: Vec<_> = h
        .fake
        .calls_of(Call::Send)
        .into_iter()
        .filter(|r| r.channel == channels.errors)
        .collect();
    assert_eq!(posts.len(), 1, "nothing to post the second time");
    let msg = posts[0].msg.clone().unwrap();
    let plain = msg.plain();
    assert!(plain.contains("3 errors in the last minute"), "{plain}");
    let discord = format!("{msg:?}");
    assert!(
        discord.contains("https://grafana.test/trace/abc123"),
        "{discord}"
    );
    Ok(())
}

#[tokio::test]
async fn server_lookups_show_counts_or_say_unknown() -> Result<()> {
    let h = Harness::new().await?;
    let ops = new_ops(&h);
    let shown = ops
        .server(&h.guild.get().to_string())
        .await?
        .unwrap()
        .plain();
    assert!(shown.contains("Quack is in this server"), "{shown}");
    assert!(shown.contains("Cases: 0 (0 voided)"), "{shown}");
    assert!(shown.contains("Missing permissions: none"), "{shown}");
    assert!(ops.server("12345").await?.is_err());
    assert!(ops.server("not a number").await?.is_err());
    Ok(())
}

#[tokio::test]
async fn only_operators_in_the_ops_server_are_operators() -> Result<()> {
    let h = Harness::new().await?;
    let ops = new_ops(&h);
    let other: GuildId = sf(999);
    assert!(ops.is_operator(Some(h.guild), sf(OWNER)));
    assert!(!ops.is_operator(Some(h.guild), sf(MEMBER)));
    assert!(!ops.is_operator(Some(other), sf(OWNER)));
    assert!(!ops.is_operator(None, sf(OWNER)));
    Ok(())
}

#[tokio::test]
async fn a_death_that_commits_after_a_poll_is_still_alerted() -> Result<()> {
    let h = Harness::new().await?;
    let ops = new_ops(&h);
    let channels = ops.prepare_channels().await.unwrap();
    ops.check_alerts().await?;
    // A worker stamped `dead_at` before that poll but committed after it.
    h.db.execute(&format!(
        "INSERT INTO jobs (kind, key, guild_id, payload, max_attempts, attempts, dead_at, last_error)
         VALUES ('send_dm', 'send_dm:late', {}, '{{}}', 3, 3, now() - interval '5 minutes', 'x')",
        h.guild.get()
    ))
    .await?;
    ops.check_alerts().await?;
    h.settle().await?;
    assert_eq!(sent_to(&h, channels.alerts).len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_job_that_dies_again_after_a_requeue_is_alerted_again() -> Result<()> {
    let h = Harness::new().await?;
    let ops = new_ops(&h);
    let channels = ops.prepare_channels().await.unwrap();
    let job = dead_job(&h).await?;
    ops.check_alerts().await?;
    ops.requeue(job, sf(OWNER)).await?.unwrap();
    h.db.execute(
        "UPDATE jobs SET dead_at = now() + interval '1 second', attempts = 3 WHERE key = 'send_dm:test'",
    )
    .await?;
    ops.check_alerts().await?;
    h.run_jobs().await?;
    assert_eq!(sent_to(&h, channels.alerts).len(), 2);
    Ok(())
}
