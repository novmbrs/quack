//! Message edits and deletes (§13.3.2): cached vs uncached, partial
//! updates, filtering, redaction, privacy flags, batches, and bulk deletes.

mod common;

use anyhow::Result;
use common::world;
use quack_activity_log::{AuthorKind, Delivered, FileFacts, Limits};
use quack_testkit::{MEMBER, QUACK};

#[tokio::test]
async fn an_edit_of_a_cached_message_shows_before_and_after_redacted() -> Result<()> {
    let w = world().await?;
    w.log.on_message_created(&w.created(1, "hello there"));
    w.log
        .on_message_updated(&w.edit(1, Some("my password=hunter2 oops")));
    assert_eq!(w.drain().await, [Delivered::Posted]);
    let posts = w.posted();
    assert_eq!(posts.len(), 1);
    assert!(posts[0].contains("was edited"), "{}", posts[0]);
    assert!(posts[0].contains("hello there"));
    assert!(posts[0].contains("[redacted]"));
    assert!(!posts[0].contains("hunter2"), "secrets never reach a post");
    Ok(())
}

#[tokio::test]
async fn an_edit_of_an_uncached_message_says_quack_had_not_seen_it() -> Result<()> {
    let w = world().await?;
    let mut edit = w.edit(2, Some("new words"));
    edit.author = Some((w.person(MEMBER), AuthorKind::Member));
    w.log.on_message_updated(&edit);
    w.drain().await;
    let posts = w.posted();
    assert_eq!(posts.len(), 1);
    assert!(
        posts[0].contains("hadn't seen the earlier text"),
        "{}",
        posts[0]
    );
    // With no author and no cached copy, there is nothing to say.
    w.log
        .on_message_updated(&w.edit(3, Some("who wrote this?")));
    assert_eq!(w.log.queued(), 0);
    Ok(())
}

#[tokio::test]
async fn partial_and_embed_only_updates_post_nothing_wrong() -> Result<()> {
    let w = world().await?;
    w.log.on_message_created(&w.created(1, "same"));
    // An embed unfurl: no edited timestamp.
    let mut unfurl = w.edit(1, None);
    unfurl.edited = false;
    unfurl.embeds = Some(vec!["article".to_owned()]);
    w.log.on_message_updated(&unfurl);
    // Edited, but neither text nor files changed.
    w.log.on_message_updated(&w.edit(1, Some("same")));
    // Text missing from the update is unknown, not empty.
    let mut files = w.edit(1, None);
    files.files = Some(vec![FileFacts {
        name: "cat.png".to_owned(),
        url: "https://cdn.discordapp.com/attachments/1/2/cat.png".to_owned(),
        size: 10,
    }]);
    w.log.on_message_updated(&files);
    w.drain().await;
    let posts = w.posted();
    assert_eq!(posts.len(), 1, "only the file change posts");
    assert!(
        posts[0].contains("Discord didn't include the new text"),
        "{}",
        posts[0]
    );
    assert!(posts[0].contains("cat.png"));
    // An empty text is known-empty.
    w.log.on_message_updated(&w.edit(1, Some("")));
    w.drain().await;
    assert!(w.posted()[1].contains("The message has no text"));
    Ok(())
}

#[tokio::test]
async fn deletes_of_cached_and_uncached_messages() -> Result<()> {
    let w = world().await?;
    w.log.on_message_created(&w.created(1, "goodbye"));
    w.log.on_message_deleted(w.h.guild, w.general, w.msg(1));
    w.drain().await;
    assert!(w.posted()[0].contains("goodbye"));
    // Uncached deletes wait in their channel's batch.
    w.log.on_message_deleted(w.h.guild, w.general, w.msg(7));
    assert_eq!(w.log.queued(), 0);
    w.log.flush_batches(true);
    w.drain().await;
    assert!(
        w.posted()[1].contains("Quack hadn't seen it"),
        "{}",
        w.posted()[1]
    );
    for id in 10..13 {
        w.log.on_message_deleted(w.h.guild, w.general, w.msg(id));
    }
    w.log.flush_batches(true);
    w.drain().await;
    assert!(
        w.posted()[2].contains("3 messages were deleted"),
        "{}",
        w.posted()[2]
    );
    // 50 uncached deletes post at once without waiting for the window.
    for id in 100..150 {
        w.log.on_message_deleted(w.h.guild, w.general, w.msg(id));
    }
    assert_eq!(w.log.queued(), 1);
    Ok(())
}

#[tokio::test]
async fn bulk_deletes_list_what_quack_had_seen() -> Result<()> {
    let w = world().await?;
    w.log.on_message_created(&w.created(1, "first"));
    w.log.on_message_created(&w.created(2, "second"));
    w.log
        .on_messages_bulk_deleted(w.h.guild, w.general, &[w.msg(1), w.msg(2), w.msg(3)]);
    w.drain().await;
    let post = &w.posted()[0];
    assert!(post.contains("3 messages were deleted"), "{post}");
    assert!(post.contains("first") && post.contains("second"));
    assert_eq!(w.log.cached(), 0, "deleted messages leave the cache");
    Ok(())
}

#[tokio::test]
async fn quack_webhooks_and_route_channels_are_never_posted() -> Result<()> {
    let w = world().await?;
    w.log
        .on_message_created(&w.created_by(1, QUACK, AuthorKind::Quack, "a receipt"));
    w.log
        .on_message_created(&w.created_by(2, 777, AuthorKind::Webhook, "hooked"));
    let mut in_route = w.created(3, "staff chatter");
    in_route.channel = w.route;
    w.log.on_message_created(&in_route);
    w.log
        .on_message_created(&w.created_by(4, 778, AuthorKind::Bot, "another bot"));
    w.log.on_message_updated(&w.edit(1, Some("edited receipt")));
    w.log.on_message_deleted(w.h.guild, w.general, w.msg(2));
    w.log.on_message_deleted(w.h.guild, w.route, w.msg(3));
    w.log.on_message_deleted(w.h.guild, w.route, w.msg(99));
    w.log.flush_batches(true);
    assert_eq!(w.log.queued(), 0, "tombstones are recognized, not unknown");
    w.log.on_message_deleted(w.h.guild, w.general, w.msg(4));
    assert_eq!(
        w.drain().await,
        [Delivered::Posted],
        "other bots are logged"
    );
    Ok(())
}

#[tokio::test]
async fn hidden_text_is_neither_cached_nor_posted() -> Result<()> {
    let w = world().await?;
    let admin =
        w.h.principal(quack_testkit::ADMIN, quack_engine::Origin::Dashboard);
    let current = quack_activity_log::settings(&w.h.engine, &admin).await?;
    quack_activity_log::update_settings(
        &w.h.engine,
        &admin,
        &quack_activity_log::ActivitySettingsInput {
            expected_version: current.version,
            include_message_content: false,
            include_file_names: false,
            include_embed_types: false,
            routes: current.routes.clone(),
        },
        &w.h.key("PUT"),
    )
    .await?;
    w.log.on_message_created(&w.created(1, "a private thought"));
    w.log.on_message_deleted(w.h.guild, w.general, w.msg(1));
    w.drain().await;
    let post = &w.posted()[0];
    assert!(post.contains("hides message text"), "{post}");
    assert!(!post.contains("a private thought"));
    Ok(())
}

#[tokio::test]
async fn a_queued_post_follows_privacy_flags_turned_off_while_it_waited() -> Result<()> {
    let w = world().await?;
    let mut created = w.created(1, "a private thought");
    created.files = vec![FileFacts {
        name: "secret-plan.png".to_owned(),
        url: "https://cdn.discordapp.com/attachments/1/2/secret-plan.png".to_owned(),
        size: 10,
    }];
    w.log.on_message_created(&created);
    w.log.on_message_deleted(w.h.guild, w.general, w.msg(1));
    assert_eq!(w.log.queued(), 1);
    let admin =
        w.h.principal(quack_testkit::ADMIN, quack_engine::Origin::Dashboard);
    let current = quack_activity_log::settings(&w.h.engine, &admin).await?;
    quack_activity_log::update_settings(
        &w.h.engine,
        &admin,
        &quack_activity_log::ActivitySettingsInput {
            expected_version: current.version,
            include_message_content: false,
            include_file_names: false,
            include_embed_types: false,
            routes: current.routes.clone(),
        },
        &w.h.key("PUT"),
    )
    .await?;
    w.reload().await?;
    assert_eq!(w.drain().await, [Delivered::Posted]);
    let post = &w.posted()[0];
    assert!(post.contains("hides message text"), "{post}");
    assert!(!post.contains("a private thought"), "{post}");
    assert!(!post.contains("secret-plan"), "{post}");
    Ok(())
}

#[tokio::test]
async fn the_cache_bound_turns_old_messages_into_unknown_ones() -> Result<()> {
    let w = common::world_with(Limits {
        cache_entries: 2,
        ..Limits::default()
    })
    .await?;
    for id in 1..=3 {
        w.log
            .on_message_created(&w.created(id, &format!("message {id}")));
    }
    assert_eq!(w.log.cached(), 2);
    w.log.on_message_deleted(w.h.guild, w.general, w.msg(1));
    w.log.on_message_deleted(w.h.guild, w.general, w.msg(3));
    w.log.flush_batches(true);
    w.drain().await;
    let posts = w.posted();
    assert!(posts.iter().any(|p| p.contains("message 3")));
    assert!(posts.iter().any(|p| p.contains("Quack hadn't seen it")));
    assert!(!posts.iter().any(|p| p.contains("message 1")));
    Ok(())
}
