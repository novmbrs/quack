//! The server activity log: turned on from `/setup`, an edit and a delete
//! of a cached message are posted with their text, and after a restart
//! (the in-memory cache is gone) a delete is batched as unknown and
//! posted when Quack shuts down.

use std::time::Duration;

use anyhow::Result;
use quack_core::ids::ChannelId;
use quack_discord::custom_id::CustomId;
use quack_testkit::{MEMBER, sf};
use quack_text::actions::{SetupArea, SetupOp};
use quack_text::render::plain_msg;
use tokio_util::sync::CancellationToken;

use crate::common::{System, admin, click};

/// Waits until `channel` holds `n` posts (the senders run in tasks).
async fn posts(sys: &System, channel: ChannelId, n: usize) -> Vec<String> {
    for _ in 0..100 {
        let posted = sys.posted(channel);
        if posted.len() >= n {
            return posted;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    sys.posted(channel)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
pub(crate) async fn edits_and_deletes_are_posted_and_unknown_deletes_flush_at_shutdown()
-> Result<()> {
    let mut sys = System::boot().await?;
    sys.install().await?;
    let create = CustomId::Setup(SetupArea::ActivityLog, SetupOp::Create).encode();
    sys.send(&click(admin(), sys.channel, &create, true)).await;
    let on = CustomId::Setup(SetupArea::ActivityLog, SetupOp::TurnOn).encode();
    sys.send(&click(admin(), sys.channel, &on, true)).await;
    let line = plain_msg(&sys.answer());
    assert!(line.contains("server activity log is on"), "{line}");
    sys.settle().await?;
    let routes: ChannelId = sf(u64::try_from(
        sys.db
            .count("SELECT channel_id FROM activity_log_routes LIMIT 1")
            .await?,
    )?);
    // `serve` starts the activity log's senders at startup.
    let stop = CancellationToken::new();
    let tasks = sys.app.modules.start(&sys.app.engine, &stop).await?;
    let before = sys.posted(routes).len();

    let first = sys.say(sys.channel, MEMBER, "see you at 5").await;
    sys.edit(sys.channel, first, MEMBER, "see you at 6").await;
    sys.delete(sys.channel, first).await;
    let posted = posts(&sys, routes, before + 2).await;
    let text = posted[before..].join("\n");
    assert!(text.contains("was edited"), "{text}");
    assert!(
        text.contains("see you at 5") && text.contains("see you at 6"),
        "{text}"
    );
    assert!(text.contains("was deleted"), "{text}");

    // A restart empties the message cache: a later delete is unknown and
    // waits in its batch until shutdown flushes it.
    let second = sys
        .say(sys.channel, MEMBER, "a message before the restart")
        .await;
    stop.cancel();
    for t in tasks {
        t.abort();
    }
    sys.app.modules.shutdown(Duration::from_secs(5)).await;
    sys.restart().await?;
    let stop = CancellationToken::new();
    let _tasks = sys.app.modules.start(&sys.app.engine, &stop).await?;
    let count = sys.posted(routes).len();
    sys.delete(sys.channel, second).await;
    sys.app.modules.shutdown(Duration::from_secs(5)).await;
    let after = posts(&sys, routes, count + 1).await;
    let flushed = after[count..].join("\n");
    assert!(flushed.contains("A message was deleted"), "{flushed}");
    assert!(
        !flushed.contains("before the restart"),
        "the text isn't known: {flushed}"
    );
    stop.cancel();
    sys.assert_no_duplicate_effects();
    Ok(())
}
