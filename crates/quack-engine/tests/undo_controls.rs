//! The undo guard and provenance (§5.6, scenario 11) and staff controls
//! (§5.8, scenario 12).

mod common;

use anyhow::{Context as _, Result};
use common::{action, logged, undo_of, void};
use quack_core::DomainError;
use quack_core::action::{ActionKind, ActionSkipReason, ActionState};
use quack_core::modlog::ModLogKind;
use quack_engine::actions::{ActionError, dismiss, retry, undo};
use quack_engine::port::RefusalKind;
use quack_testkit::fake::{Call, Script};
use quack_testkit::rules_for_tests::{BAN, TIMEOUT, levels, one};
use quack_testkit::{Harness, MEMBER, sf};
use time::OffsetDateTime;

#[tokio::test]
async fn another_valid_cases_ban_keeps_the_ban() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h
        .rule(levels("Spam", &[Some(BAN), Some(BAN)], false))
        .await?;
    let first = h.apply(MEMBER, rule).await?;
    let second = h.apply(MEMBER, rule).await?;
    h.settle().await?;
    assert_eq!(h.fake.count(Call::Ban), 1);
    void(&h, first.case.id).await?;
    h.settle().await?;
    let undo = undo_of(&h, first.action.unwrap()).await?.unwrap();
    assert_eq!(undo.state, ActionState::Skipped);
    assert_eq!(
        undo.skip,
        Some((ActionSkipReason::OtherCaseApplies, Some(second.case.id)))
    );
    assert!(h.fake.world(|w| w.ban(h.guild, sf(MEMBER)).is_some()));
    assert_eq!(logged(&h, ModLogKind::ActionUndoSkipped).await?, 1);
    // Voiding the case that still applies removes the ban: it owns it now.
    void(&h, second.case.id).await?;
    h.settle().await?;
    assert!(h.fake.world(|w| w.ban(h.guild, sf(MEMBER)).is_none()));
    Ok(())
}

#[tokio::test]
async fn a_timeout_changed_by_hand_is_left_in_place() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(one("Spam", TIMEOUT, false)).await?;
    let created = h.apply(MEMBER, rule).await?;
    h.settle().await?;
    let other = OffsetDateTime::now_utc() + time::Duration::days(3);
    h.fake.world(|w| {
        if let Some(m) = w.member_mut(h.guild, sf(MEMBER)) {
            m.timeout_until = Some(other);
        }
    });
    void(&h, created.case.id).await?;
    h.settle().await?;
    let undo = undo_of(&h, created.action.unwrap()).await?.unwrap();
    assert_eq!(
        (undo.state, undo.skip),
        (
            ActionState::Skipped,
            Some((ActionSkipReason::ChangedOutsideCase, None))
        )
    );
    assert_eq!(h.fake.count(Call::RemoveTimeout), 0);
    Ok(())
}

#[tokio::test]
async fn a_timeout_quack_placed_is_removed_on_void() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(one("Spam", TIMEOUT, false)).await?;
    let created = h.apply(MEMBER, rule).await?;
    h.settle().await?;
    void(&h, created.case.id).await?;
    h.settle().await?;
    let undo = undo_of(&h, created.action.unwrap()).await?.unwrap();
    assert_eq!(undo.state, ActionState::Done);
    assert_eq!(
        h.fake
            .world(|w| w.member(h.guild, sf(MEMBER)).and_then(|m| m.timeout_until)),
        None
    );
    Ok(())
}

#[tokio::test]
async fn a_ban_without_a_marker_is_left_in_place() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(one("Spam", BAN, false)).await?;
    let created = h.apply(MEMBER, rule).await?;
    h.settle().await?;
    h.fake
        .world(|w| w.set_ban(h.guild, sf(MEMBER), Some("Banned by hand")));
    void(&h, created.case.id).await?;
    h.settle().await?;
    let undo = undo_of(&h, created.action.unwrap()).await?.unwrap();
    assert_eq!(
        undo.skip,
        Some((ActionSkipReason::ChangedOutsideCase, None))
    );
    assert_eq!(h.fake.count(Call::Unban), 0);
    Ok(())
}

#[tokio::test]
async fn nothing_in_place_is_no_change() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(one("Spam", BAN, false)).await?;
    let created = h.apply(MEMBER, rule).await?;
    h.settle().await?;
    h.fake.world(|w| {
        if let Some(g) = w.guilds.get_mut(&h.guild) {
            g.bans.clear();
        }
    });
    void(&h, created.case.id).await?;
    h.settle().await?;
    let undo = undo_of(&h, created.action.unwrap()).await?.unwrap();
    assert_eq!((undo.state, undo.no_change), (ActionState::Done, true));
    assert_eq!(h.fake.count(Call::Unban), 0);
    Ok(())
}

async fn failed_ban(h: &Harness) -> Result<quack_engine::cases::CaseCreated> {
    let rule = h.rule(one("Spam", BAN, false)).await?;
    failed_ban_under(h, rule).await
}

async fn failed_ban_under(
    h: &Harness,
    rule: quack_core::ids::RuleId,
) -> Result<quack_engine::cases::CaseCreated> {
    let created = h.apply(MEMBER, rule).await?;
    h.fake
        .script(Call::Ban, Script::refuse(RefusalKind::MissingPermissions));
    h.settle().await?;
    assert_eq!(
        action(h, created.action.context("an action")?).await?.state,
        ActionState::Failed
    );
    Ok(created)
}

#[tokio::test]
async fn retrying_a_failed_action_starts_a_new_round() -> Result<()> {
    let h = Harness::new().await?;
    let created = failed_ban(&h).await?;
    let id = created.action.unwrap();
    let done = retry(&h.engine, &h.moderator(), id, &h.key("ar")).await?;
    assert_eq!(done.action, id);
    let a = action(&h, id).await?;
    assert_eq!(
        (a.state, a.attempts, a.failure),
        (ActionState::Pending, 0, None)
    );
    h.settle().await?;
    assert_eq!(action(&h, id).await?.state, ActionState::Done);
    assert_eq!(h.fake.count(Call::Ban), 2);
    assert_eq!(logged(&h, ModLogKind::ActionRetryRequested).await?, 1);
    Ok(())
}

#[tokio::test]
async fn retrying_an_original_on_a_voided_case_is_refused() -> Result<()> {
    let h = Harness::new().await?;
    let created = failed_ban(&h).await?;
    void(&h, created.case.id).await?;
    let err = retry(
        &h.engine,
        &h.moderator(),
        created.action.unwrap(),
        &h.key("ar"),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, ActionError::Refused(DomainError::CaseVoided)));
    Ok(())
}

#[tokio::test]
async fn retrying_a_failed_undo_works() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(one("Spam", BAN, false)).await?;
    let created = h.apply(MEMBER, rule).await?;
    h.settle().await?;
    void(&h, created.case.id).await?;
    h.fake
        .script(Call::Unban, Script::refuse(RefusalKind::MissingPermissions));
    h.settle().await?;
    let undo = undo_of(&h, created.action.unwrap()).await?.unwrap();
    assert_eq!(undo.state, ActionState::Failed);
    retry(&h.engine, &h.moderator(), undo.id, &h.key("ar")).await?;
    h.settle().await?;
    assert_eq!(action(&h, undo.id).await?.state, ActionState::Done);
    assert!(h.fake.world(|w| w.ban(h.guild, sf(MEMBER)).is_none()));
    Ok(())
}

async fn failed(h: &Harness, dismissed: bool) -> Result<usize> {
    let mut conn = h.db.db.conn().await?;
    Ok(
        quack_db::actions::list(&mut conn, h.guild, ActionState::Failed, dismissed, None, 50)
            .await?
            .len(),
    )
}

#[tokio::test]
async fn dismiss_hides_a_failure_from_the_failed_list() -> Result<()> {
    let h = Harness::new().await?;
    let created = failed_ban(&h).await?;
    let id = created.action.unwrap();
    assert_eq!(failed(&h, false).await?, 1);
    dismiss(&h.engine, &h.moderator(), id, &h.key("ad")).await?;
    assert_eq!((failed(&h, false).await?, failed(&h, true).await?), (0, 1));
    let again = dismiss(&h.engine, &h.moderator(), id, &h.key("ad")).await;
    assert!(matches!(
        again,
        Err(ActionError::Refused(DomainError::ActionNotDismissable))
    ));
    Ok(())
}

#[tokio::test]
async fn concurrent_retry_and_dismiss_exactly_one_wins() -> Result<()> {
    let h = std::sync::Arc::new(Harness::new().await?);
    let rule = h.rule(one("Spam", BAN, false)).await?;
    for _ in 0..5 {
        let created = failed_ban_under(&h, rule).await?;
        let id = created.action.unwrap();
        let (a, b) = {
            let (h1, h2) = (std::sync::Arc::clone(&h), std::sync::Arc::clone(&h));
            tokio::join!(
                tokio::spawn(
                    async move { retry(&h1.engine, &h1.moderator(), id, &h1.key("ar")).await }
                ),
                tokio::spawn(async move {
                    dismiss(&h2.engine, &h2.moderator(), id, &h2.key("ad")).await
                }),
            )
        };
        let wins = usize::from(a?.is_ok()) + usize::from(b?.is_ok());
        assert_eq!(wins, 1);
        h.settle().await?;
        h.fake.world(|w| {
            if let Some(g) = w.guilds.get_mut(&h.guild) {
                g.bans.clear();
            }
            w.add_member(
                h.guild,
                &quack_testkit::fake::MemberSetup::new(sf(MEMBER), 5),
            );
        });
    }
    Ok(())
}

#[tokio::test]
async fn staff_undo_queues_one_undo() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(one("Spam", BAN, false)).await?;
    let created = h.apply(MEMBER, rule).await?;
    h.settle().await?;
    let id = created.action.unwrap();
    let done = undo(&h.engine, &h.moderator(), id, &h.key("auc")).await?;
    let u = action(&h, done.action).await?;
    assert_eq!((u.kind, u.undo_of), (ActionKind::Unban, Some(id)));
    let again = undo(&h.engine, &h.moderator(), id, &h.key("auc")).await;
    assert!(matches!(
        again,
        Err(ActionError::Refused(DomainError::ActionNotUndoable))
    ));
    h.settle().await?;
    // The case is still valid, but the ban carries its own marker: the
    // undo staff asked for removes it.
    assert_eq!(action(&h, done.action).await?.state, ActionState::Done);
    assert!(h.fake.world(|w| w.ban(h.guild, sf(MEMBER)).is_none()));
    Ok(())
}

#[tokio::test]
async fn undo_is_not_offered_while_another_case_applies() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h
        .rule(levels("Spam", &[Some(BAN), Some(BAN)], false))
        .await?;
    let first = h.apply(MEMBER, rule).await?;
    h.apply(MEMBER, rule).await?;
    h.settle().await?;
    let refused = undo(
        &h.engine,
        &h.moderator(),
        first.action.unwrap(),
        &h.key("auo"),
    )
    .await;
    assert!(
        matches!(
            refused,
            Err(ActionError::Refused(DomainError::ActionNotUndoable))
        ),
        "{refused:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_remove_timeout_request_runs_the_role_check_only() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(one("Spam", TIMEOUT, false)).await?;
    let created = h.apply(MEMBER, rule).await?;
    h.settle().await?;
    let moderator = h.moderator();
    let position = moderator
        .member
        .map(|m| m.highest_role_position)
        .context("the moderator is in the server")?;
    h.fake.world(|w| {
        if let Some(m) = w.member_mut(h.guild, sf(MEMBER)) {
            m.highest_role_position = position;
        }
    });
    let refused = undo(
        &h.engine,
        &moderator,
        created.action.unwrap(),
        &h.key("art"),
    )
    .await;
    assert!(
        matches!(
            refused,
            Err(ActionError::Refused(DomainError::MemberRefused(
                quack_core::permissions::MemberRefusal::MemberRoleTooHigh
            )))
        ),
        "{refused:?}"
    );
    Ok(())
}
