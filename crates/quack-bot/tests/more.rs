//! The remaining controls: `/rules` and its pager, the appeals and
//! history pagers, the rule picker's pager, Try again after a save
//! (`cs`), a Go-era history button, and a draft clicked by another
//! moderator.

#![allow(clippy::unwrap_used, clippy::panic, reason = "tests")]

mod common;

use common::payload::{self, Who, interaction, member, moderator};
use common::{MOD_ROLE, Sent, World, snapshot};
use quack_core::ids::CaseId;
use quack_testkit::rules_for_tests::warning;
use quack_testkit::{MEMBER, MODERATOR_2};
use serde_json::json;

type Result = anyhow::Result<()>;

fn click(w: &World, who: Who, id: &str) -> serde_json::Value {
    payload::on_message(
        interaction(3, who, w.channel, &payload::component(id, 2, &[])),
        w.channel,
        true,
    )
}

#[tokio::test]
async fn rules_lists_active_rules_for_staff_only() -> Result {
    let w = World::new().await?;
    for name in ["Spam", "Insults", "Raids"] {
        w.h.rule(warning(name)).await?;
    }
    w.send(&interaction(
        2,
        moderator(),
        w.channel,
        &payload::slash("rules", &json!([])),
    ))
    .await;
    w.assert_acked_first();
    w.assert_ephemeral();
    snapshot("rules", &w.answer());
    w.send(&click(&w, moderator(), "1:rl:2")).await;
    w.assert_acked_first();
    assert!(w.answer().plain().contains("Rules"));
    w.send(&interaction(
        2,
        member(),
        w.channel,
        &payload::slash("rules", &json!([])),
    ))
    .await;
    snapshot("rules_not_staff", &w.answer());
    Ok(())
}

#[tokio::test]
async fn pagers_reauthorize_and_page() -> Result {
    let w = World::new().await?;
    let rule = w.h.rule(warning("Spam")).await?;
    w.h.apply(MEMBER, rule).await?;
    for id in ["1:al:1".to_owned(), format!("1:hi:{MEMBER}:1")] {
        let sent = w.send(&click(&w, moderator(), &id)).await;
        w.assert_acked_first();
        assert_eq!(sent.first(), Some(&Sent::DeferUpdate), "{id}");
        w.send(&click(&w, member(), &id)).await;
        assert!(w.answer().plain().contains("Only moderators"), "{id}");
    }
    // The Go-era "member details" button opens the history.
    w.send(&click(
        &w,
        moderator(),
        &format!("case:user_detail:v1:{MEMBER}"),
    ))
    .await;
    w.assert_acked_first();
    snapshot("legacy_user_detail", &w.answer());
    Ok(())
}

#[tokio::test]
async fn the_picker_pages_and_try_again_finds_the_saved_case() -> Result {
    let w = World::new().await?;
    for i in 0..30 {
        w.h.rule(warning(&format!("Rule {i:02}"))).await?;
    }
    w.send(&interaction(
        2,
        moderator(),
        w.channel,
        &payload::add_case_for(MEMBER),
    ))
    .await;
    let draft: CaseId = match &w.answer().rows[0] {
        quack_text::model::Row::Select(s) => match s.action {
            quack_text::SelectAction::PickRule(d) => d,
            other => panic!("{other:?}"),
        },
        other @ quack_text::model::Row::Buttons(_) => panic!("{other:?}"),
    };
    w.send(&click(
        &w,
        moderator(),
        &format!("1:rpp:{}:2", draft.to_u22()),
    ))
    .await;
    w.assert_acked_first();
    snapshot("picker_page_2", &w.answer());

    // Another moderator can't use this moderator's draft.
    let other = Who {
        id: MODERATOR_2,
        role: Some(MOD_ROLE),
        permissions: moderator().permissions,
    };
    w.send(&click(&w, other, &format!("1:rpp:{}:1", draft.to_u22())))
        .await;
    snapshot("picker_other_moderator", &w.answer());

    // Pick, then Try again (`cs`): the operation key finds the case.
    let rule = w.h.engine.rules_cache().cached(w.h.guild).unwrap().active[0].id;
    let pick = payload::component(
        &payload::ids::rule_picker(draft),
        3,
        &[payload::ids::rule_value(rule)],
    );
    w.send(&payload::on_message(
        interaction(3, moderator(), w.channel, &pick),
        w.channel,
        true,
    ))
    .await;
    assert_eq!(w.h.db.count("SELECT count(*) FROM cases").await?, 1);
    w.send(&click(&w, moderator(), &format!("1:cs:{}", draft.to_u22())))
        .await;
    w.assert_acked_first();
    assert_eq!(
        w.h.db.count("SELECT count(*) FROM cases").await?,
        1,
        "still one case"
    );
    snapshot("save_again_after_success", &w.answer());
    Ok(())
}
