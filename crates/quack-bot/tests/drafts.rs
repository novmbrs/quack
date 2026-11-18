//! The multi-page `/case add` form (§11.6): the first page opens at once,
//! pages save by compare-and-set, double clicks and stale pages are
//! harmless, problems re-open prefilled, and the "Add case" commands'
//! rule picker. End to end on real PostgreSQL with the fake Discord.

#![allow(clippy::unwrap_used, clippy::panic, reason = "tests")]

mod common;

use common::payload::{self, interaction, moderator};
use common::{Sent, Step, World, snapshot};
use quack_core::ids::{CaseId, RuleId};
use quack_core::rule::{Field, FieldKey, FieldKind, RuleDefinition};
use quack_testkit::MEMBER;
use quack_testkit::rules_for_tests::warning;
use quack_text::actions::{DraftPage, InputId};
use quack_text::model::Modal;

type Result = anyhow::Result<()>;

/// A rule with `n` required short-text fields `q1`…`qn` and one number.
fn form_rule(name: &str, n: usize) -> RuleDefinition {
    let mut def = warning(name);
    def.fields = (1..=n)
        .map(|i| Field {
            key: FieldKey(format!("q{i}")),
            label: format!("Question {i}"),
            kind: FieldKind::ShortText,
            required: true,
        })
        .collect();
    def.fields.push(Field {
        key: FieldKey("count".to_owned()),
        label: "How many times?".to_owned(),
        kind: FieldKind::Number,
        required: false,
    });
    def
}

/// The draft page a modal saves.
fn page_of(modal: &Modal) -> DraftPage {
    match &modal.action {
        quack_text::ModalAction::DraftPage(p) => *p,
        other => panic!("not a form page: {other:?}"),
    }
}

/// The modal submit for a form page with these answers.
fn submit(at: DraftPage, answers: &[(&str, &str)]) -> serde_json::Value {
    let id = format!(
        "1:cd:{}:{}:{}:{}",
        at.draft.to_u22(),
        at.rule.to_u22(),
        at.version,
        at.page
    );
    let inputs: Vec<(String, &str)> = answers
        .iter()
        .map(|(k, v)| (format!("f:{k}"), *v))
        .collect();
    let refs: Vec<(&str, &str)> = inputs.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    payload::modal(&id, &refs)
}

fn continue_button(at: DraftPage) -> serde_json::Value {
    payload::component(
        &format!(
            "1:cn:{}:{}:{}:{}",
            at.draft.to_u22(),
            at.rule.to_u22(),
            at.version,
            at.page
        ),
        2,
        &[],
    )
}

fn answers(range: std::ops::RangeInclusive<usize>) -> Vec<(String, String)> {
    range
        .map(|i| (format!("q{i}"), format!("answer {i}")))
        .collect()
}

fn refs(a: &[(String, String)]) -> Vec<(&str, &str)> {
    a.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect()
}

async fn cases(w: &World) -> anyhow::Result<i64> {
    w.h.db.count("SELECT count(*) FROM cases").await
}

#[tokio::test]
async fn a_two_page_form_saves_each_page_once_and_applies_at_the_end() -> Result {
    let w = World::new().await?;
    let rule = w.h.rule(form_rule("Scam", 6)).await?;

    // 1. /case add writes the draft row, then opens page 1 (the bounded
    //    prefilled-modal exception), so the page's submit always finds it,
    //    even right away or after a restart.
    w.send(&interaction(
        2,
        moderator(),
        w.channel,
        &payload::case_add(MEMBER, rule, Some(true)),
    ))
    .await;
    w.assert_single_ack();
    let steps = w.log.steps();
    let ack = steps.iter().position(|s| matches!(s, Step::Ack("modal")));
    let work = steps.iter().position(|s| matches!(s, Step::Work(_)));
    assert!(
        work < ack,
        "the draft is saved before the form shows: {steps:?}"
    );
    assert_eq!(
        w.h.db.count("SELECT count(*) FROM case_drafts").await?,
        1,
        "the form's draft"
    );
    let first = w.modal().expect("page 1");
    assert_eq!(first.title, "Scam (1/2)");
    assert_eq!(first.inputs.len(), 5);
    let at = page_of(&first);

    // 2. Page 1 saves; Continue is offered.
    let page1 = answers(1..=5);
    let p1 = interaction(5, moderator(), w.channel, &submit(at, &refs(&page1)));
    w.send(&p1).await;
    w.assert_acked_first();
    w.assert_ephemeral();
    snapshot("draft_page_saved", &w.answer());

    // 3. A double click on page 1 (a new interaction) says it's saved.
    w.send(&interaction(
        5,
        moderator(),
        w.channel,
        &submit(at, &refs(&page1)),
    ))
    .await;
    w.assert_acked_first();
    snapshot("draft_page_already_saved", &w.answer());

    // 4. Continue opens page 2 from the rules cache.
    let next = DraftPage { page: 2, ..at };
    let cont = payload::on_message(
        interaction(3, moderator(), w.channel, &continue_button(next)),
        w.channel,
        true,
    );
    w.send(&cont).await;
    w.assert_acked_first();
    let second = w.modal().expect("page 2");
    assert_eq!(second.title, "Scam (2/2)");
    assert_eq!(second.inputs.len(), 2);

    // 5. Page 2 applies the rule; the private receipt is the answer.
    let page2 = vec![
        ("q6".to_owned(), "answer 6".to_owned()),
        ("count".to_owned(), "3".to_owned()),
    ];
    let p2 = payload::on_message(
        interaction(5, moderator(), w.channel, &submit(next, &refs(&page2))),
        w.channel,
        true,
    );
    let sent = w.send(&p2).await;
    w.assert_acked_first();
    assert_eq!(sent, vec![Sent::DeferUpdate]);
    assert_eq!(cases(&w).await?, 1);
    assert_eq!(
        w.h.db.count("SELECT count(*) FROM case_drafts").await?,
        0,
        "the case used the draft"
    );
    w.h.settle().await?;
    let edits = w.h.fake.world(|world| world.interaction_edits.clone());
    snapshot("draft_private_receipt", &edits.last().expect("receipt").1);

    // 6. Submitting the last page again finds the case.
    w.send(&payload::on_message(
        interaction(5, moderator(), w.channel, &submit(next, &refs(&page2))),
        w.channel,
        true,
    ))
    .await;
    assert_eq!(cases(&w).await?, 1, "still one case");
    snapshot("draft_already_applied", &w.answer());
    Ok(())
}

#[tokio::test]
async fn a_page_with_problems_reopens_prefilled() -> Result {
    let w = World::new().await?;
    let rule = w.h.rule(form_rule("Spam", 1)).await?;
    w.send(&interaction(
        2,
        moderator(),
        w.channel,
        &payload::case_add(MEMBER, rule, None),
    ))
    .await;
    let at = page_of(&w.modal().expect("page 1"));
    w.send(&interaction(
        5,
        moderator(),
        w.channel,
        &submit(at, &[("q1", "links"), ("count", "lots")]),
    ))
    .await;
    w.assert_acked_first();
    snapshot("draft_page_invalid", &w.answer());
    assert_eq!(cases(&w).await?, 0);

    let retry = payload::component(
        &format!(
            "1:cr:{}:{}:{}:{}",
            at.draft.to_u22(),
            at.rule.to_u22(),
            at.version,
            at.page
        ),
        2,
        &[],
    );
    w.send(&payload::on_message(
        interaction(3, moderator(), w.channel, &retry),
        w.channel,
        true,
    ))
    .await;
    w.assert_single_ack();
    let modal = w.modal().expect("the page again");
    let value = |key: &str| {
        modal
            .inputs
            .iter()
            .find(|i| i.id == InputId::Field(FieldKey(key.to_owned())))
            .and_then(|i| match &i.kind {
                quack_text::model::InputKind::Text { value, .. } => value.clone(),
                quack_text::model::InputKind::YesNo { .. } => None,
            })
    };
    assert_eq!(value("q1").as_deref(), Some("links"));
    assert_eq!(value("count").as_deref(), Some("lots"));

    // A non-staff member clicking the same Try again gets no modal.
    w.send(&payload::on_message(
        interaction(3, payload::member(), w.channel, &retry),
        w.channel,
        true,
    ))
    .await;
    assert!(w.modal().is_none());
    snapshot("draft_retry_refused", &w.answer());
    Ok(())
}

#[tokio::test]
async fn a_stale_page_after_a_rule_edit_is_refused() -> Result {
    let w = World::new().await?;
    let rule = w.h.rule(form_rule("Spam", 1)).await?;
    w.send(&interaction(
        2,
        moderator(),
        w.channel,
        &payload::case_add(MEMBER, rule, None),
    ))
    .await;
    let at = page_of(&w.modal().expect("page 1"));
    let admin =
        w.h.principal(quack_testkit::ADMIN, quack_engine::Origin::Dashboard);
    quack_engine::rules::edit(
        &w.h.engine,
        &admin,
        rule,
        1,
        form_rule("Spam", 2),
        &w.h.key("edit"),
    )
    .await?;
    w.h.engine
        .rules_cache()
        .reload(w.h.engine.db(), w.h.guild)
        .await?;

    w.send(&interaction(
        5,
        moderator(),
        w.channel,
        &submit(at, &[("q1", "x")]),
    ))
    .await;
    w.assert_acked_first();
    snapshot("draft_rule_changed", &w.answer());
    assert_eq!(cases(&w).await?, 0);

    let cont = continue_button(DraftPage { page: 1, ..at });
    w.send(&payload::on_message(
        interaction(3, moderator(), w.channel, &cont),
        w.channel,
        true,
    ))
    .await;
    w.assert_acked_first();
    assert!(w.modal().is_none(), "no modal for an old version");
    Ok(())
}

#[tokio::test]
async fn an_expired_token_never_posts_a_private_receipt_publicly() -> Result {
    let w = World::new().await?;
    let rule = w.h.rule(warning("Spam")).await?;
    let old = payload::interaction_with(
        2,
        moderator(),
        w.channel,
        &payload::case_add(MEMBER, rule, Some(true)),
        payload::old_snowflake(),
    );
    w.send(&old).await;
    w.assert_acked_first();
    assert_eq!(cases(&w).await?, 1);
    w.h.settle().await?;
    let (posted, edits) = w.h.fake.world(|world| {
        (
            world.sent_to(w.channel).len(),
            world.interaction_edits.len(),
        )
    });
    assert_eq!(posted, 0, "never posted publicly");
    assert_eq!(edits, 0, "the expired token is not used");
    Ok(())
}

#[tokio::test]
async fn the_member_command_picks_a_rule_and_posts_the_receipt() -> Result {
    let w = World::new().await?;
    w.h.rule(warning("Spam")).await?;
    let insults = w.h.rule(warning("Insults")).await?;
    w.send(&interaction(
        2,
        moderator(),
        w.channel,
        &payload::add_case_for(MEMBER),
    ))
    .await;
    w.assert_acked_first();
    w.assert_ephemeral();
    let picker = w.answer();
    snapshot("picker", &picker);
    let draft: CaseId = match &picker.rows[0] {
        quack_text::model::Row::Select(s) => match s.action {
            quack_text::SelectAction::PickRule(d) => d,
            other => panic!("{other:?}"),
        },
        other @ quack_text::model::Row::Buttons(_) => panic!("{other:?}"),
    };
    let pick = payload::component(
        &payload::ids::rule_picker(draft),
        3,
        &[payload::ids::rule_value(insults)],
    );
    let sent = w
        .send(&payload::on_message(
            interaction(3, moderator(), w.channel, &pick),
            w.channel,
            true,
        ))
        .await;
    w.assert_acked_first();
    assert_eq!(sent, vec![Sent::DeferUpdate]);
    assert_eq!(cases(&w).await?, 1);
    w.h.settle().await?;
    let posted = w.h.fake.world(|world| world.sent_to(w.channel).len());
    assert_eq!(posted, 1, "the public receipt");
    let edits = w.h.fake.world(|world| world.interaction_edits.clone());
    snapshot(
        "picker_receipt_posted",
        &edits.last().expect("confirmation").1,
    );

    // A non-staff member picking on the same picker is refused.
    let sent = w
        .send(&payload::on_message(
            interaction(3, payload::member(), w.channel, &pick),
            w.channel,
            true,
        ))
        .await;
    assert_eq!(sent.first(), Some(&Sent::DeferUpdate));
    snapshot("picker_not_staff", &w.answer());
    let _: Option<RuleId> = None;
    Ok(())
}

#[tokio::test]
async fn the_message_command_applies_the_only_rule_privately_with_evidence() -> Result {
    let w = World::new().await?;
    w.h.rule(warning("Spam")).await?;
    let message = w.h.fake.world(|world| {
        world.post(
            w.h.guild,
            w.channel,
            quack_testkit::sf(MEMBER),
            "spam spam spam",
            Vec::new(),
        )
    });
    let sent = w
        .send(&interaction(
            2,
            moderator(),
            w.channel,
            &payload::add_case_on(message.get(), w.channel, MEMBER),
        ))
        .await;
    w.assert_acked_first();
    assert_eq!(sent, vec![Sent::Defer { ephemeral: true }]);
    assert_eq!(cases(&w).await?, 1);
    assert_eq!(
        w.h.db.count("SELECT count(*) FROM evidence").await?,
        1,
        "the message is evidence"
    );
    w.h.settle().await?;
    let posted = w.h.fake.world(|world| {
        world
            .sent_to(w.channel)
            .iter()
            .filter(|m| m.msg.as_ref().is_some_and(|m| !m.rows.is_empty()))
            .count()
    });
    assert_eq!(posted, 0, "the receipt is private");
    Ok(())
}
