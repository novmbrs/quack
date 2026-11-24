//! Appeals from Discord (the member's DM controls and staff review),
//! guided `/setup` for the core areas, and the core gateway events.

#![allow(clippy::unwrap_used, clippy::panic, reason = "tests")]

mod common;

use common::payload::{self, admin, interaction, member, moderator};
use common::{Sent, World, snapshot};
use quack_core::ids::{AppealId, ChannelId, GuildId};
use quack_core::permissions::{
    AccountFacts, ChannelFacts, Overwrite, OverwriteTarget, Permission, Permissions,
};
use quack_discord::gateway::Event;
use quack_discord::model::{Channel, Guild, MemberCount};
use quack_engine::{OpKey, Origin};
use quack_testkit::rules_for_tests::warning;
use quack_testkit::{MEMBER, OWNER, sf};
use serde_json::json;

type Result = anyhow::Result<()>;

async fn appeal(w: &World) -> anyhow::Result<(quack_core::ids::CaseId, AppealId)> {
    let rule = w.h.rule(warning("Spam")).await?;
    let case = w.h.apply(MEMBER, rule).await?.case.id;
    let done = quack_engine::appeals::submit(
        &w.h.engine,
        sf(MEMBER),
        Origin::Dashboard,
        case,
        "It wasn't me.",
        &OpKey::new("test", "appeal"),
    )
    .await?;
    Ok((case, done.appeal))
}

#[tokio::test]
async fn staff_review_appeals_with_a_reason_modal() -> Result {
    let w = World::new().await?;
    let (_, appeal) = appeal(&w).await?;
    w.send(&interaction(
        2,
        moderator(),
        w.channel,
        &payload::slash("appeals", &json!([])),
    ))
    .await;
    w.assert_acked_first();
    w.assert_ephemeral();
    snapshot("appeals_list", &w.answer());

    let open = payload::on_message(
        interaction(
            3,
            moderator(),
            w.channel,
            &payload::component(&payload::ids::open_appeal(appeal), 2, &[]),
        ),
        w.channel,
        true,
    );
    w.send(&open).await;
    w.assert_acked_first();
    snapshot("appeal_card", &w.answer());

    // A member clicking an appeal post's button learns nothing.
    let public = payload::on_message(
        interaction(
            3,
            member(),
            w.channel,
            &payload::component(&payload::ids::open_appeal(appeal), 2, &[]),
        ),
        w.channel,
        false,
    );
    w.send(&public).await;
    w.assert_acked_first();
    w.assert_ephemeral();
    snapshot("appeal_card_not_staff", &w.answer());

    // Reject opens a modal with the default reason; submitting decides.
    let reject = payload::component(&format!("1:aj:{}:1", appeal.to_u22()), 2, &[]);
    w.send(&payload::on_message(
        interaction(3, moderator(), w.channel, &reject),
        w.channel,
        false,
    ))
    .await;
    w.assert_acked_first();
    assert!(w.modal().is_some());
    let submit = payload::modal(
        &format!("1:ajm:{}:1", appeal.to_u22()),
        &[("reason", "We checked the logs.")],
    );
    let sent = w
        .send(&interaction(5, moderator(), w.channel, &submit))
        .await;
    w.assert_acked_first();
    assert_eq!(sent.first(), Some(&Sent::Defer { ephemeral: true }));
    snapshot("appeal_rejected", &w.answer());
    // The same stale version again.
    w.send(&interaction(5, moderator(), w.channel, &submit))
        .await;
    snapshot("appeal_changed", &w.answer());
    Ok(())
}

#[tokio::test]
async fn the_member_appeals_from_their_dm() -> Result {
    let w = World::new().await?;
    let rule = w.h.rule(warning("Spam")).await?;
    let case = w.h.apply(MEMBER, rule).await?.case.id;
    let open = payload::dm_interaction(
        3,
        MEMBER,
        &payload::component(&format!("1:ap:{}", case.to_u22()), 2, &[]),
    );
    w.send(&open).await;
    w.assert_acked_first();
    assert!(w.modal().is_some());
    let submit = payload::modal(
        &format!("1:apm:{}", case.to_u22()),
        &[("statement", "I was quoting someone.")],
    );
    let sent = w.send(&payload::dm_interaction(5, MEMBER, &submit)).await;
    w.assert_acked_first();
    assert_eq!(
        sent.first(),
        Some(&Sent::Defer { ephemeral: false }),
        "no ephemeral flag in DMs"
    );
    snapshot("appeal_submitted", &w.answer());
    w.send(&payload::dm_interaction(5, MEMBER, &submit)).await;
    snapshot("appeal_submitted_again", &w.answer());
    // Someone else's case reads as not found.
    w.send(&payload::dm_interaction(
        5,
        quack_testkit::MEMBER_2,
        &submit,
    ))
    .await;
    snapshot("appeal_not_yours", &w.answer());
    Ok(())
}

fn su(w: &World, who: payload::Who, id: &str, values: &[String]) -> serde_json::Value {
    let kind = if values.is_empty() { 2 } else { 8 };
    payload::on_message(
        interaction(3, who, w.channel, &payload::component(id, kind, values)),
        w.channel,
        true,
    )
}

#[tokio::test]
async fn setup_chooses_creates_and_clears_the_log_channel() -> Result {
    let w = World::new().await?;
    w.send(&interaction(
        2,
        admin(),
        w.channel,
        &payload::slash("setup", &json!([])),
    ))
    .await;
    w.assert_acked_first();
    w.assert_ephemeral();
    snapshot("setup_home", &w.answer());

    w.send(&interaction(
        2,
        moderator(),
        w.channel,
        &payload::slash("setup", &json!([])),
    ))
    .await;
    snapshot("setup_not_allowed", &w.answer());

    let staff =
        w.h.fake
            .world(|world| world.add_text_channel(w.h.guild, true));
    w.send(&su(&w, admin(), "1:su:log:ch", &[staff.to_string()]))
        .await;
    w.assert_acked_first();
    snapshot("setup_log_saved", &w.answer());

    w.send(&su(&w, admin(), "1:su:log:ch", &[w.channel.to_string()]))
        .await;
    snapshot("setup_log_not_staff_only", &w.answer());

    w.send(&su(&w, admin(), "1:su:log:new", &[])).await;
    w.assert_acked_first();
    snapshot("setup_log_created", &w.answer());
    // A #moderation-log Quack made a moment ago (a double click or a lost
    // answer) is adopted instead of created again.
    let recent: ChannelId = sf(payload::snowflake());
    w.h.fake.world(|world| {
        let g = world.guilds.get_mut(&w.h.guild).unwrap();
        g.channels.insert(
            recent,
            ChannelFacts {
                id: recent,
                guild_id: w.h.guild,
                kind: 0,
                name: "moderation-log".to_owned(),
                parent_id: None,
                overwrites: vec![
                    Overwrite {
                        target: OverwriteTarget::Role(w.h.guild.everyone_role()),
                        allow: Permissions::NONE,
                        deny: Permission::ViewChannel.into(),
                    },
                    Overwrite {
                        target: OverwriteTarget::Member(sf(quack_testkit::QUACK)),
                        allow: Permission::ViewChannel.into(),
                        deny: Permissions::NONE,
                    },
                ],
                quack_permissions: quack_testkit::quack_permissions(),
            },
        );
    });
    w.send(&su(&w, admin(), "1:su:log:new", &[])).await;
    assert_eq!(
        w.h.fake.count(quack_testkit::fake::Call::CreateChannel),
        0,
        "nothing created"
    );
    snapshot("setup_log_adopted", &w.answer());

    w.send(&su(&w, admin(), "1:su:log:clr", &[])).await;
    snapshot("setup_log_cleared", &w.answer());

    // The rejoin invite modal is prefilled (the documented exception).
    w.send(&su(&w, admin(), "1:su:ap:inv", &[])).await;
    w.assert_single_ack();
    assert!(w.modal().is_some());
    let save = payload::modal("1:su:ap:invm", &[("invite", "https://discord.gg/quack")]);
    w.send(&payload::on_message(
        interaction(5, admin(), w.channel, &save),
        w.channel,
        true,
    ))
    .await;
    w.assert_acked_first();
    snapshot("setup_invite_saved", &w.answer());

    // Module areas show their module's settings.
    w.send(&su(&w, admin(), "1:su:tk:open", &[])).await;
    snapshot("setup_tickets_area", &w.answer());
    Ok(())
}

fn guild(id: u64) -> Guild {
    Guild {
        id: sf(id),
        name: "New server".to_owned(),
        icon_hash: None,
        owner_id: sf(OWNER),
        mfa_required: false,
        roles: Vec::new(),
    }
}

/// The member count follows the counts the gateway sends, newest first:
/// one read earlier than the stored one never moves it back (§21.6).
#[tokio::test]
async fn member_counts_keep_the_newest() -> Result {
    let w = World::new().await?;
    let new: GuildId = sf(2002);
    let t0 = time::OffsetDateTime::now_utc();
    let at = |secs: i64| t0 + time::Duration::seconds(secs);
    w.bot
        .on_event(Event::GuildCreate {
            guild: guild(2002),
            channel_ids: Vec::new(),
            thread_ids: Vec::new(),
            members: Some(MemberCount {
                members: 41,
                at: at(1),
            }),
        })
        .await;
    let leaver = AccountFacts {
        id: sf(9_001),
        username: "leaver".to_owned(),
        global_name: None,
        avatar_hash: None,
        bot: false,
    };
    w.bot
        .on_event(Event::MemberRemove {
            guild_id: new,
            account: leaver.clone(),
            members: Some(MemberCount {
                members: 40,
                at: at(3),
            }),
        })
        .await;
    // A count read before the stored one, handled late.
    w.bot
        .on_event(Event::MemberRemove {
            guild_id: new,
            account: leaver,
            members: Some(MemberCount {
                members: 99,
                at: at(2),
            }),
        })
        .await;
    assert_eq!(
        w.h.db
            .count("SELECT member_count::bigint FROM guilds WHERE guild_id = 2002")
            .await?,
        40
    );
    Ok(())
}

#[tokio::test]
async fn gateway_bootstraps_repairs_and_leaves_servers() -> Result {
    let w = World::new().await?;
    let new: GuildId = sf(2002);
    w.bot
        .on_event(Event::GuildCreate {
            guild: guild(2002),
            channel_ids: Vec::new(),
            thread_ids: Vec::new(),
            members: None,
        })
        .await;

    let rules =
        w.h.db
            .count("SELECT count(*) FROM rules WHERE guild_id = 2002")
            .await?;
    assert_eq!(rules, 1, "the starter rule");
    assert!(
        w.h.engine.config_cache().cached(new).is_some(),
        "the config cache is warm"
    );

    // A deleted log channel is cleared.
    let staff =
        w.h.fake
            .world(|world| world.add_text_channel(w.h.guild, true));
    w.send(&su(&w, admin(), "1:su:log:ch", &[staff.to_string()]))
        .await;
    let channel = Channel {
        id: staff,
        guild_id: Some(w.h.guild),
        kind: 0,
        name: "log".to_owned(),
        parent_id: None,
        overwrites: Vec::new(),
        thread: None,
        owner_id: None,
    };
    w.bot.on_event(Event::ChannelDelete(channel)).await;
    let set =
        w.h.db
            .count("SELECT count(*) FROM guild_settings WHERE log_channel_id IS NOT NULL")
            .await?;
    assert_eq!(set, 0);

    // Quack leaves: once by GUILD_DELETE, once by missing from READY.
    w.bot
        .on_event(Event::GuildDelete {
            guild_id: new,
            unavailable: true,
        })
        .await;
    assert_eq!(
        w.h.db
            .count("SELECT count(*) FROM guilds WHERE installed")
            .await?,
        2,
        "an outage is not leaving"
    );
    w.bot
        .on_event(Event::GuildDelete {
            guild_id: new,
            unavailable: false,
        })
        .await;
    assert_eq!(
        w.h.db
            .count("SELECT count(*) FROM guilds WHERE installed")
            .await?,
        1
    );
    w.bot
        .on_event(Event::Ready {
            quack_id: sf(quack_testkit::QUACK),
            guild_ids: Vec::new(),
            missed_events: false,
        })
        .await;
    assert_eq!(
        w.h.db
            .count("SELECT count(*) FROM guilds WHERE installed")
            .await?,
        0
    );
    let _: Option<ChannelId> = None;
    Ok(())
}
