//! `/help`, autocomplete, unknown and legacy custom ids, and `/case add`
//! applied at once, end to end on real PostgreSQL with the fake Discord.

#![allow(clippy::unwrap_used, clippy::panic, reason = "tests")]

mod common;

use common::payload::{self, Who, interaction, member, moderator};
use common::{Sent, World, snapshot};
use quack_core::ids::CaseId;
use quack_testkit::rules_for_tests::{TIMEOUT, one, warning};
use quack_testkit::{MEMBER, MODERATOR, OWNER};
use serde_json::json;

type Result = anyhow::Result<()>;

#[tokio::test]
async fn help_answers_at_once_with_static_text() -> Result {
    let w = World::new().await?;
    let sent = w
        .send(&interaction(
            2,
            member(),
            w.channel,
            &payload::slash("help", &json!([])),
        ))
        .await;
    w.assert_acked_first();
    w.assert_ephemeral();
    assert!(matches!(
        sent.as_slice(),
        [Sent::Reply {
            ephemeral: true,
            ..
        }]
    ));
    assert!(
        w.log
            .steps()
            .iter()
            .all(|s| !matches!(s, common::Step::Work(_))),
        "no database work"
    );
    snapshot("help", &w.answer());
    Ok(())
}

#[tokio::test]
async fn unknown_expired_and_legacy_ids_get_their_answers() -> Result {
    let w = World::new().await?;
    let cases = [
        ("expired", "1:zz:nothing"),
        ("legacy_unknown", "case:explode:v1:01HZ"),
        (
            "legacy_unmapped_case",
            "case:view:v1:01ARZ3NDEKTSV4RRFFQ69G5FAV",
        ),
        (
            "legacy_unmapped_action",
            "case:retry:v1:01ARZ3NDEKTSV4RRFFQ69G5FAV",
        ),
    ];
    for (name, id) in cases {
        let p = payload::on_message(
            interaction(3, moderator(), w.channel, &payload::component(id, 2, &[])),
            w.channel,
            false,
        );
        w.send(&p).await;
        w.assert_ephemeral();
        snapshot(&format!("button_{name}"), &w.answer());
    }
    Ok(())
}

#[tokio::test]
async fn an_unknown_command_is_answered() -> Result {
    let w = World::new().await?;
    w.send(&interaction(
        2,
        moderator(),
        w.channel,
        &payload::slash("template", &json!([])),
    ))
    .await;
    w.assert_acked_first();
    w.assert_ephemeral();
    snapshot("unknown_command", &w.answer());
    Ok(())
}

#[tokio::test]
async fn autocomplete_answers_from_memory_for_moderators_only() -> Result {
    let w = World::new().await?;
    w.h.rule(warning("Spam")).await?;
    w.h.rule(warning("Spoilers")).await?;
    w.h.rule(warning("Harassment")).await?;
    let sent = w
        .send(&interaction(
            4,
            moderator(),
            w.channel,
            &payload::rule_autocomplete("add", "sp"),
        ))
        .await;
    w.assert_acked_first();
    assert!(
        w.log
            .steps()
            .iter()
            .all(|s| !matches!(s, common::Step::Work(_))),
        "memory only"
    );
    let [Sent::Choices(choices)] = sent.as_slice() else {
        panic!("{sent:?}")
    };
    let mut names: Vec<&str> = choices.iter().map(|c| c.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["Spam", "Spoilers"]);
    assert!(
        choices
            .iter()
            .all(|c| uuid::Uuid::try_parse(&c.value).is_ok())
    );
    let sent = w
        .send(&interaction(
            4,
            member(),
            w.channel,
            &payload::rule_autocomplete("add", "sp"),
        ))
        .await;
    assert_eq!(
        sent,
        vec![Sent::Choices(Vec::new())],
        "no choices without moderate"
    );
    Ok(())
}

async fn cases(w: &World) -> anyhow::Result<i64> {
    w.h.db.count("SELECT count(*) FROM cases").await
}

#[tokio::test]
async fn case_add_private_defers_applies_and_the_receipt_job_answers() -> Result {
    let w = World::new().await?;
    let rule = w.h.rule(warning("Spam")).await?;
    let sent = w
        .send(&interaction(
            2,
            moderator(),
            w.channel,
            &payload::case_add(MEMBER, rule, Some(true)),
        ))
        .await;
    w.assert_acked_first();
    w.assert_ephemeral();
    assert_eq!(
        sent,
        vec![Sent::Defer { ephemeral: true }],
        "the receipt's sync owns the answer"
    );
    assert_eq!(cases(&w).await?, 1);
    w.h.settle().await?;
    let edits = w.h.fake.world(|world| world.interaction_edits.clone());
    let (_, card) = edits.last().expect("the private receipt");
    snapshot("case_add_private_receipt", card);
    let public = w.h.fake.world(|world| world.sent_to(w.channel).len());
    assert_eq!(public, 0, "a private receipt is never posted");
    Ok(())
}

#[tokio::test]
async fn case_add_public_posts_the_receipt_and_confirms_privately() -> Result {
    let w = World::new().await?;
    let rule = w.h.rule(one("Timeouts", TIMEOUT, true)).await?;
    w.send(&interaction(
        2,
        moderator(),
        w.channel,
        &payload::case_add(MEMBER, rule, None),
    ))
    .await;
    w.assert_acked_first();
    w.assert_ephemeral();
    w.h.settle().await?;
    let posted = w.h.fake.world(|world| {
        world
            .sent_to(w.channel)
            .iter()
            .filter_map(|m| m.msg.clone())
            .collect::<Vec<_>>()
    });
    snapshot(
        "case_add_public_receipt",
        posted.last().expect("a public receipt"),
    );
    let edits = w.h.fake.world(|world| world.interaction_edits.clone());
    snapshot(
        "case_add_public_confirmation",
        &edits.last().expect("a confirmation").1,
    );
    Ok(())
}

#[tokio::test]
async fn a_repeated_delivery_is_dropped_and_a_replay_finds_the_same_case() -> Result {
    let w = World::new().await?;
    let rule = w.h.rule(warning("Spam")).await?;
    let id = payload::snowflake();
    let p = payload::interaction_with(
        2,
        moderator(),
        w.channel,
        &payload::case_add(MEMBER, rule, None),
        id,
    );
    w.send(&p).await;
    let again = w.send(&p).await;
    assert!(
        again.is_empty(),
        "a duplicate delivery is dropped before the ack"
    );
    // A restarted process has an empty dedupe set: the operation key
    // returns the same case, and nothing happens twice.
    let fresh = quack_bot::Bot::new(w.h.engine.clone(), w.recorder.clone(), Vec::new());
    w.h.fake.clear_calls();
    fresh
        .on_interaction(quack_discord::interaction::decode_json(&p.to_string()).unwrap())
        .await;
    assert_eq!(cases(&w).await?, 1, "one case");
    snapshot("case_add_replayed", &w.answer());
    Ok(())
}

async fn refused(w: &World, who: Who, member_id: u64, name: &str) -> Result {
    let rule = quack_core::ids::RuleId::from_uuid(
        w.h.engine.rules_cache().cached(w.h.guild).unwrap().active[0]
            .id
            .get(),
    );
    w.send(&interaction(
        2,
        who,
        w.channel,
        &payload::case_add(member_id, rule, None),
    ))
    .await;
    w.assert_acked_first();
    w.assert_ephemeral();
    snapshot(&format!("case_add_refused_{name}"), &w.answer());
    Ok(())
}

#[tokio::test]
async fn case_add_refusals() -> Result {
    let w = World::new().await?;
    w.h.rule(warning("Spam")).await?;
    refused(&w, member(), MEMBER, "missing_capability").await?;
    refused(&w, moderator(), MODERATOR, "self").await?;
    refused(&w, moderator(), OWNER, "owner").await?;
    refused(&w, moderator(), quack_testkit::QUACK, "bot").await?;
    refused(&w, moderator(), quack_testkit::ADMIN, "hierarchy").await?;
    assert_eq!(cases(&w).await?, 0);
    Ok(())
}

#[tokio::test]
async fn case_add_requires_2fa_when_the_server_does() -> Result {
    let w = World::new().await?;
    w.h.rule(warning("Spam")).await?;
    w.h.fake.world(|world| {
        world.guilds.get_mut(&w.h.guild).unwrap().facts.mfa_required = true;
    });
    refused(&w, moderator(), MEMBER, "mfa").await?;
    assert_eq!(cases(&w).await?, 0);
    Ok(())
}

#[tokio::test]
async fn case_add_with_an_archived_rule_is_refused() -> Result {
    let w = World::new().await?;
    let rule = w.h.rule(warning("Old")).await?;
    w.h.db
        .execute(&format!(
            "UPDATE rules SET archived_at = now() WHERE id = '{}'",
            rule.get()
        ))
        .await?;
    w.h.engine
        .rules_cache()
        .reload(w.h.engine.db(), w.h.guild)
        .await?;
    w.send(&interaction(
        2,
        moderator(),
        w.channel,
        &payload::case_add(MEMBER, rule, None),
    ))
    .await;
    w.assert_acked_first();
    snapshot("case_add_refused_archived", &w.answer());
    assert_eq!(cases(&w).await?, 0);
    let _ = CaseId::from_uuid(uuid::Uuid::nil());
    Ok(())
}
