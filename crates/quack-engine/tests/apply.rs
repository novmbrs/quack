//! Applying a rule (§4.2, §20.1.1 scenarios 1–4).

use anyhow::Result;
use quack_core::DomainError;
use quack_core::action::{ActionKind, ActionState};
use quack_core::modlog::ModLogKind;
use quack_core::permissions::{MemberRefusal, Permission, Permissions};
use quack_core::rule::LevelAction;
use quack_db::modlog::{self, ModLogFilter};
use quack_engine::cases::{Actor, Applied, ApplyError, apply_rule};
use quack_engine::{OpKey, Origin};
use quack_testkit::rules_for_tests::{BAN, TIMEOUT, levels, one, warning};
use quack_testkit::{GUILD, Harness, MEMBER, MODERATOR, MODERATOR_2, OWNER, sf};

async fn log_kinds(h: &Harness) -> Result<Vec<ModLogKind>> {
    let mut conn = h.db.db.conn().await?;
    let entries = modlog::list(&mut conn, h.guild, &ModLogFilter::default(), None, 100).await?;
    Ok(entries.into_iter().rev().map(|e| e.entry.kind()).collect())
}

#[tokio::test]
async fn a_warning_is_a_case_with_no_action() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(warning("Spam")).await?;
    let created = h.apply(MEMBER, rule).await?;
    assert_eq!(created.case.number, 1);
    assert!(created.action.is_none());
    assert!(created.dm.is_some());
    let d = created.case.decision.unwrap();
    assert_eq!((d.case_count, d.level_index), (1, 1));
    assert_eq!(log_kinds(&h).await?, vec![ModLogKind::CaseCreated]);
    Ok(())
}

#[tokio::test]
async fn a_timeout_runs_once_with_the_fixed_end() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(one("Spam", TIMEOUT, false)).await?;
    let created = h.apply(MEMBER, rule).await?;
    h.settle().await?;
    let timeouts = h.fake.calls_of(quack_testkit::fake::Call::Timeout);
    assert_eq!(timeouts.len(), 1);
    let expected = quack_core::action::timeout_ends_at(created.case.created_at, 3600);
    assert_eq!(timeouts[0].until, Some(expected));
    assert_eq!(timeouts[0].reason.as_deref(), Some("Quack case #1: Spam"));
    let mut conn = h.db.db.conn().await?;
    let action = quack_db::actions::get(&mut conn, h.guild, created.action.unwrap())
        .await?
        .unwrap();
    assert_eq!(action.state, ActionState::Done);
    assert_eq!(
        h.fake
            .world(|w| w.member(h.guild, sf(MEMBER)).and_then(|m| m.timeout_until)),
        Some(expected)
    );
    Ok(())
}

#[tokio::test]
async fn concurrent_creations_get_consecutive_numbers_and_counts() -> Result<()> {
    let h = std::sync::Arc::new(Harness::with_connections(60).await?);
    let rule = h
        .rule(levels(
            "Spam",
            &[None, None, Some(TIMEOUT), None, Some(BAN)],
            false,
        ))
        .await?;
    let mut tasks = Vec::new();
    for _ in 0..50 {
        let h = std::sync::Arc::clone(&h);
        tasks.push(tokio::spawn(async move {
            apply_rule(&h.engine, h.request(MEMBER, rule)).await
        }));
    }
    let mut decided = Vec::new();
    for task in tasks {
        match task.await?? {
            Applied::Created(c) => decided.push((c.case.number, c.case.decision.unwrap())),
            Applied::Replayed(_) => anyhow::bail!("unexpected replay"),
        }
    }
    decided.sort_by_key(|(n, _)| *n);
    for (i, (number, d)) in decided.iter().enumerate() {
        let n = u64::try_from(i + 1)?;
        assert_eq!(*number, n);
        // Case numbers and case counts advance together: one counter row
        // serializes creation, so each case counts every earlier one.
        assert_eq!(u64::from(d.case_count), n);
        let expected_level = u8::try_from(n.min(5))?;
        assert_eq!(d.level_index, expected_level);
    }
    Ok(())
}

#[tokio::test]
async fn the_same_operation_key_creates_one_case() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(warning("Spam")).await?;
    let request = h.request(MEMBER, rule);
    let first = apply_rule(&h.engine, request.clone()).await?;
    let again = apply_rule(&h.engine, request.clone()).await?;
    assert!(matches!(first, Applied::Created(_)));
    assert_eq!(again, Applied::Replayed(first.case_id()));
    // Concurrent repeats all replay the one case.
    let mut tasks = Vec::new();
    let h = std::sync::Arc::new(h);
    for _ in 0..8 {
        let h = std::sync::Arc::clone(&h);
        let request = request.clone();
        tasks.push(tokio::spawn(
            async move { apply_rule(&h.engine, request).await },
        ));
    }
    for task in tasks {
        assert_eq!(task.await??.case_id(), first.case_id());
    }
    assert_eq!(h.db.count("SELECT count(*) FROM cases").await?, 1);
    // A different request with the key is refused.
    let mut other = request.clone();
    other.member = sf(OWNER);
    assert!(matches!(
        apply_rule(&h.engine, other).await,
        Err(ApplyError::OperationKeyReused)
    ));
    Ok(())
}

#[tokio::test]
async fn concurrent_requests_with_one_key_create_one_case() -> Result<()> {
    let h = std::sync::Arc::new(Harness::with_connections(30).await?);
    let rule = h.rule(one("Spam", BAN, true)).await?;
    let request = h.request(MEMBER, rule);
    let mut tasks = Vec::new();
    for _ in 0..12 {
        let h = std::sync::Arc::clone(&h);
        let request = request.clone();
        tasks.push(tokio::spawn(
            async move { apply_rule(&h.engine, request).await },
        ));
    }
    let mut ids = Vec::new();
    for task in tasks {
        ids.push(task.await??.case_id());
    }
    ids.dedup();
    assert_eq!(ids.len(), 1);
    assert_eq!(h.db.count("SELECT count(*) FROM cases").await?, 1);
    assert_eq!(h.db.count("SELECT count(*) FROM actions").await?, 1);
    assert_eq!(h.db.count("SELECT count(*) FROM dms").await?, 1);
    h.settle().await?;
    assert_eq!(h.fake.count(quack_testkit::fake::Call::Ban), 1);
    assert_eq!(h.fake.world(|w| w.dms_to(sf(MEMBER)).len()), 1);
    Ok(())
}

#[tokio::test]
async fn two_moderators_with_the_same_key_create_two_cases() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(warning("Spam")).await?;
    let key = OpKey::new("case_add", "123456789");
    let mut a = h.request(MEMBER, rule);
    a.op_key = key.clone();
    let mut b = h.request(MEMBER, rule);
    b.op_key = key;
    b.actor = Actor::Staff(h.principal(MODERATOR_2, Origin::Dashboard));
    let first = apply_rule(&h.engine, a).await?;
    let second = apply_rule(&h.engine, b).await?;
    assert_ne!(first.case_id(), second.case_id());
    Ok(())
}

#[tokio::test]
async fn a_replay_after_losing_moderate_is_refused() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(warning("Spam")).await?;
    let request = h.request(MEMBER, rule);
    apply_rule(&h.engine, request.clone()).await?;
    h.fake.world(|w| {
        if let Some(m) = w.member_mut(sf(GUILD), sf(MODERATOR)) {
            m.permissions = Permissions::NONE;
        }
    });
    let mut again = request;
    again.actor = Actor::Staff(h.principal(MODERATOR, Origin::Dashboard));
    assert!(matches!(
        apply_rule(&h.engine, again).await,
        Err(ApplyError::Refused(DomainError::MissingCapability(_)))
    ));
    Ok(())
}

#[tokio::test]
async fn quack_caused_refusals_are_logged_and_others_are_not() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(one("Spam", BAN, false)).await?;
    // The owner: refused, not logged.
    let err = apply_rule(&h.engine, h.request(OWNER, rule))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        ApplyError::Refused(DomainError::MemberRefused(MemberRefusal::MemberIsOwner))
    ));
    // Self: refused, not logged.
    let err = apply_rule(&h.engine, h.request(MODERATOR, rule))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        ApplyError::Refused(DomainError::MemberRefused(MemberRefusal::MemberIsSelf))
    ));
    assert_eq!(log_kinds(&h).await?, Vec::new());
    // Quack lacks Ban Members: refused after the level is chosen, logged.
    h.fake.world(|w| {
        if let Some(g) = w.guilds.get_mut(&sf(GUILD))
            && let Some(q) = g.facts.quack.as_mut()
        {
            q.permissions = Permission::ViewChannel.into();
        }
    });
    let err = apply_rule(&h.engine, h.request(MEMBER, rule))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        ApplyError::Refused(DomainError::MemberRefused(
            MemberRefusal::QuackMissingPermission(_)
        ))
    ));
    assert_eq!(log_kinds(&h).await?, vec![ModLogKind::CaseCreateRefused]);
    assert_eq!(h.db.count("SELECT count(*) FROM cases").await?, 0);
    let _ = (ActionKind::Ban, LevelAction::Kick);
    Ok(())
}
