//! Showing and changing cases from Discord: `/case view|list|history|void`,
//! Open case, the case select, Void case, Edit context, Add evidence, View
//! evidence, and Retry / Dismiss / Remove timeout. Every click is
//! re-authorized: a non-staff member clicking a public receipt's Open case
//! sees nothing.

#![allow(clippy::unwrap_used, clippy::panic, reason = "tests")]

mod common;

use common::payload::{self, admin, interaction, member, moderator};
use common::{Sent, World, snapshot};
use quack_core::case::MessageLink;
use quack_core::ids::{CaseId, GuildId};
use quack_core::rule::{Field, FieldKey, FieldKind};
use quack_engine::port::RefusalKind;
use quack_testkit::fake::{Call, Script};
use quack_testkit::rules_for_tests::{BAN, TIMEOUT, one, warning};
use quack_testkit::{MEMBER, MEMBER_2};
use serde_json::json;

type Result = anyhow::Result<()>;

fn number(n: u64) -> serde_json::Value {
    json!([{"name": "number", "type": 4, "value": n}])
}

fn button(w: &World, who: payload::Who, id: &str, ephemeral: bool) -> serde_json::Value {
    payload::on_message(
        interaction(3, who, w.channel, &payload::component(id, 2, &[])),
        w.channel,
        ephemeral,
    )
}

#[tokio::test]
async fn case_view_shows_the_card_privately() -> Result {
    let w = World::new().await?;
    let rule = w.h.rule(one("Timeouts", TIMEOUT, true)).await?;
    w.h.apply(MEMBER, rule).await?;
    w.h.settle().await?;
    let sent = w
        .send(&interaction(
            2,
            moderator(),
            w.channel,
            &payload::case("view", &number(1), None),
        ))
        .await;
    w.assert_acked_first();
    w.assert_ephemeral();
    assert_eq!(sent.first(), Some(&Sent::Defer { ephemeral: true }));
    snapshot("case_view", &w.answer());

    w.send(&interaction(
        2,
        moderator(),
        w.channel,
        &payload::case("view", &number(9), None),
    ))
    .await;
    snapshot("case_view_not_found", &w.answer());

    w.send(&interaction(
        2,
        member(),
        w.channel,
        &payload::case("view", &number(1), None),
    ))
    .await;
    w.assert_acked_first();
    snapshot("case_view_not_staff", &w.answer());
    Ok(())
}

#[tokio::test]
async fn open_case_on_a_public_receipt_answers_privately_and_reauthorizes() -> Result {
    let w = World::new().await?;
    let rule = w.h.rule(warning("Spam")).await?;
    let case = w.h.apply(MEMBER, rule).await?.case.id;
    let id = payload::ids::open_case(case);
    let sent = w.send(&button(&w, moderator(), &id, false)).await;
    w.assert_acked_first();
    w.assert_ephemeral();
    assert_eq!(
        sent.first(),
        Some(&Sent::Defer { ephemeral: true }),
        "a new private message"
    );
    assert!(w.answer().plain().contains("Case #1"));

    w.send(&button(&w, member(), &id, false)).await;
    w.assert_acked_first();
    w.assert_ephemeral();
    assert!(
        !w.answer().plain().contains("Case #1"),
        "no staff data for a member"
    );
    snapshot("open_case_not_staff", &w.answer());

    // On an ephemeral list, the card replaces the list.
    let sent = w.send(&button(&w, moderator(), &id, true)).await;
    assert_eq!(sent.first(), Some(&Sent::DeferUpdate));

    // A case of another server names nothing here.
    let other = CaseId::from_uuid(uuid::Uuid::now_v7());
    w.send(&button(
        &w,
        moderator(),
        &payload::ids::open_case(other),
        true,
    ))
    .await;
    snapshot("open_case_other_server", &w.answer());
    Ok(())
}

#[tokio::test]
async fn lists_and_history_page_and_offer_the_case_select() -> Result {
    let w = World::new().await?;
    let spam = w.h.rule(warning("Spam")).await?;
    for _ in 0..11 {
        w.h.apply(MEMBER, spam).await?;
    }
    w.h.apply(MEMBER_2, spam).await?;
    w.send(&interaction(
        2,
        moderator(),
        w.channel,
        &payload::case("list", &json!([]), None),
    ))
    .await;
    w.assert_acked_first();
    w.assert_ephemeral();
    snapshot("case_list", &w.answer());

    w.send(&button(
        &w,
        moderator(),
        &format!("1:cl:2:{MEMBER}:0"),
        true,
    ))
    .await;
    w.assert_acked_first();
    snapshot("case_list_page_2_member", &w.answer());

    let history = json!([{"name": "member", "type": 6, "value": MEMBER.to_string()}]);
    w.send(&interaction(
        2,
        moderator(),
        w.channel,
        &payload::case("history", &history, Some(MEMBER)),
    ))
    .await;
    w.assert_acked_first();
    snapshot("case_history", &w.answer());

    let pick = payload::component("1:cvs", 3, &[CaseId::from_uuid(uuid::Uuid::nil()).to_u22()]);
    w.send(&payload::on_message(
        interaction(3, moderator(), w.channel, &pick),
        w.channel,
        true,
    ))
    .await;
    w.assert_acked_first();

    w.send(&interaction(
        2,
        member(),
        w.channel,
        &payload::case("list", &json!([]), None),
    ))
    .await;
    snapshot("case_list_not_staff", &w.answer());
    Ok(())
}

#[tokio::test]
async fn void_from_the_command_and_the_modal() -> Result {
    let w = World::new().await?;
    let rule = w.h.rule(one("Bans", BAN, true)).await?;
    let first = w.h.apply(MEMBER, rule).await?.case.id;
    w.h.settle().await?;
    let reason = json!([{"name": "number", "type": 4, "value": 1}, {"name": "reason", "type": 3, "value": "Wrong member"}]);
    w.send(&interaction(
        2,
        moderator(),
        w.channel,
        &payload::case("void", &reason, None),
    ))
    .await;
    w.assert_acked_first();
    w.assert_ephemeral();
    snapshot("case_void_done", &w.answer());
    w.send(&interaction(
        2,
        moderator(),
        w.channel,
        &payload::case("void", &reason, None),
    ))
    .await;
    snapshot("case_void_again", &w.answer());

    // Void case opens the modal; a member gets a refusal instead.
    let second = w.h.apply(MEMBER_2, rule).await?.case.id;
    w.send(&button(&w, moderator(), &payload::ids::void(second), true))
        .await;
    w.assert_acked_first();
    assert!(w.modal().is_some());
    w.send(&button(&w, member(), &payload::ids::void(second), true))
        .await;
    assert!(w.modal().is_none());
    let submit = payload::modal(
        &payload::ids::void_modal(second),
        &[("reason", "Duplicate")],
    );
    let p = payload::on_message(
        interaction(5, moderator(), w.channel, &submit),
        w.channel,
        true,
    );
    let sent = w.send(&p).await;
    w.assert_acked_first();
    assert_eq!(sent.first(), Some(&Sent::DeferUpdate));
    snapshot("case_void_modal", &w.answer());
    let voided =
        w.h.db
            .count("SELECT count(*) FROM cases WHERE voided_at IS NOT NULL")
            .await?;
    assert_eq!(voided, 2);
    let _ = first;
    Ok(())
}

#[tokio::test]
async fn edit_context_and_evidence_from_the_card() -> Result {
    let w = World::new().await?;
    let mut def = warning("Spam");
    def.fields = vec![Field {
        key: FieldKey("where".to_owned()),
        label: "Where did it happen?".to_owned(),
        kind: FieldKind::ShortText,
        required: false,
    }];
    let rule = w.h.rule(def).await?;
    let case = w.h.apply(MEMBER, rule).await?.case.id;

    // Edit context: the prefilled modal (the documented exception).
    w.send(&button(
        &w,
        moderator(),
        &format!("1:ce:{}", case.to_u22()),
        true,
    ))
    .await;
    w.assert_single_ack();
    let modal = w.modal().expect("the context modal");
    let revision = match modal.action {
        quack_text::ModalAction::EditContext { revision, .. } => revision,
        ref other => panic!("{other:?}"),
    };
    let submit = payload::modal(
        &format!("1:cem:{}:{revision}", case.to_u22()),
        &[("f:where", "general")],
    );
    w.send(&payload::on_message(
        interaction(5, moderator(), w.channel, &submit),
        w.channel,
        true,
    ))
    .await;
    w.assert_acked_first();
    snapshot("context_saved", &w.answer());
    // The same old revision again: someone else changed it.
    w.send(&payload::on_message(
        interaction(5, moderator(), w.channel, &submit),
        w.channel,
        true,
    ))
    .await;
    snapshot("context_changed", &w.answer());
    // A member gets no modal.
    w.send(&button(
        &w,
        member(),
        &format!("1:ce:{}", case.to_u22()),
        true,
    ))
    .await;
    assert!(w.modal().is_none());

    // Add evidence, then view it.
    let message = w.h.fake.world(|world| {
        world.post(
            w.h.guild,
            w.channel,
            quack_testkit::sf(MEMBER),
            "proof",
            Vec::new(),
        )
    });
    let link = MessageLink {
        guild_id: w.h.guild,
        channel_id: w.channel,
        message_id: message,
    }
    .to_url();
    w.send(&button(
        &w,
        moderator(),
        &format!("1:ea:{}", case.to_u22()),
        true,
    ))
    .await;
    assert!(w.modal().is_some_and(|m| m.title.contains("#1")));
    let add = payload::modal(&format!("1:eam:{}", case.to_u22()), &[("links", &link)]);
    w.send(&payload::on_message(
        interaction(5, moderator(), w.channel, &add),
        w.channel,
        true,
    ))
    .await;
    w.assert_acked_first();
    snapshot("evidence_added", &w.answer());
    w.h.settle().await?;
    w.send(&button(
        &w,
        moderator(),
        &format!("1:ev:{}:1", case.to_u22()),
        true,
    ))
    .await;
    w.assert_acked_first();
    snapshot("evidence_view", &w.answer());
    Ok(())
}

#[tokio::test]
async fn retry_dismiss_and_undo_from_the_card() -> Result {
    let w = World::new().await?;
    let rule = w.h.rule(one("Timeouts", TIMEOUT, false)).await?;
    w.h.fake.script(
        Call::Timeout,
        Script::refuse(RefusalKind::MissingPermissions),
    );
    let created = w.h.apply(MEMBER, rule).await?;
    w.h.settle().await?;
    let action = created.action.unwrap();
    w.send(&interaction(
        2,
        moderator(),
        w.channel,
        &payload::case("view", &number(1), None),
    ))
    .await;
    snapshot("card_failed_action", &w.answer());

    w.send(&button(&w, member(), &payload::ids::retry(action), true))
        .await;
    w.assert_acked_first();
    snapshot("retry_not_staff", &w.answer());

    w.send(&button(&w, moderator(), &payload::ids::retry(action), true))
        .await;
    w.assert_acked_first();
    snapshot("retry_done", &w.answer());
    w.h.settle().await?;

    w.send(&button(
        &w,
        moderator(),
        &payload::ids::dismiss(action),
        true,
    ))
    .await;
    snapshot("dismiss_after_success", &w.answer());

    w.send(&button(&w, moderator(), &payload::ids::undo(action), true))
        .await;
    w.assert_acked_first();
    snapshot("undo_confirm", &w.answer());
    w.send(&button(
        &w,
        admin(),
        &payload::ids::confirm_undo(action),
        true,
    ))
    .await;
    w.assert_acked_first();
    snapshot("undo_queued", &w.answer());
    let _: Option<GuildId> = None;
    Ok(())
}
