//! The honeypot: turned on from `/setup` (Quack creates the channel and
//! its rule), a member's burst of posts becomes one case after a restart,
//! the posts are deleted only after the case saved them, and staff posts
//! are ignored.

use anyhow::Result;
use quack_core::ids::ChannelId;
use quack_discord::custom_id::CustomId;
use quack_testkit::{MEMBER, MODERATOR, sf};
use quack_text::actions::{SetupArea, SetupOp};
use quack_text::render::plain_msg;

use crate::common::{System, admin, click};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_burst_in_the_honeypot_is_one_case_and_the_posts_are_cleaned_up() -> Result<()> {
    let mut sys = System::boot().await?;
    sys.install().await?;

    // /setup: Quack creates #honeypot; turning it on with no rule creates
    // the "Honeypot" rule.
    let create = CustomId::Setup(SetupArea::Honeypot, SetupOp::Create).encode();
    sys.send(&click(admin(), sys.channel, &create, true)).await;
    let made = plain_msg(&sys.answer());
    assert!(made.contains("so members can see it"), "{made}");
    let on = CustomId::Setup(SetupArea::Honeypot, SetupOp::TurnOn).encode();
    sys.send(&click(admin(), sys.channel, &on, true)).await;
    let line = plain_msg(&sys.answer());
    assert!(line.contains("Quack created the rule"), "{line}");
    assert!(line.contains("The honeypot is on"), "{line}");
    sys.settle().await?;
    let trap: ChannelId = sf(u64::try_from(
        sys.db
            .count("SELECT channel_id FROM honeypot_settings")
            .await?,
    )?);
    assert!(!sys.posted(trap).is_empty(), "the warning");

    // Staff posts are ignored; a member's two quick posts are one burst.
    sys.say(trap, MODERATOR, "Testing the trap.").await;
    let first = sys.say(trap, MEMBER, "free nitro here").await;
    let second = sys.say(trap, MEMBER, "click the link").await;
    assert_eq!(sys.db.count("SELECT count(*) FROM honeypot_hits").await?, 2);
    sys.restart().await?;
    sys.settle().await?;

    let cases = sys
        .db
        .count("SELECT count(*) FROM cases WHERE source = 'honeypot'")
        .await?;
    assert_eq!(cases, 1, "one case for the burst");
    let saved = sys
        .db
        .count("SELECT count(*) FROM evidence WHERE state = 'saved'")
        .await?;
    assert!(saved >= 1, "the first post was saved as evidence");
    let gone = sys
        .fake
        .world(|w| !w.messages.contains_key(&first) && !w.messages.contains_key(&second));
    assert!(gone, "both posts deleted after the case saved them");
    let cleaned = sys
        .db
        .count("SELECT count(*) FROM honeypot_hits WHERE state = 'case_created' AND cleaned_up_at IS NOT NULL")
        .await?;
    assert_eq!(cleaned, 2);
    let warning = sys.posted(trap).join("\n");
    assert!(
        warning.contains('1'),
        "the warning counts the case: {warning}"
    );
    sys.assert_no_duplicate_effects();
    Ok(())
}
