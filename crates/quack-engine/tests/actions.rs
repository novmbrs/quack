//! Running actions (§5, §20.1.1 scenarios 6–11): lost answers, crashes at
//! every boundary, the fixed timeout end, rejoined members, the budget.

mod common;

use anyhow::Result;
use common::{action, log_kinds, undo_of, void};
use quack_core::action::{ActionFailure, ActionKind, ActionState, AttemptOutcome};
use quack_core::modlog::ModLogKind;
use quack_db::jobs::JobKind;
use quack_engine::port::RefusalKind;
use quack_testkit::fake::{Call, Script};
use quack_testkit::rules_for_tests::{BAN, KICK, TIMEOUT, one};
use quack_testkit::{Harness, MEMBER, RunEnd, sf};
use time::OffsetDateTime;

async fn attempts(
    h: &Harness,
    id: quack_core::ids::ActionId,
) -> Result<Vec<Option<AttemptOutcome>>> {
    let mut conn = h.db.db.conn().await?;
    Ok(quack_db::actions::attempts(&mut conn, id)
        .await?
        .into_iter()
        .map(|a| a.outcome)
        .collect())
}

#[tokio::test]
async fn a_lost_ban_answer_is_not_banned_twice() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(one("Spam", BAN, false)).await?;
    let created = h.apply(MEMBER, rule).await?;
    h.fake.script(Call::Ban, Script::LoseResponse);
    h.settle().await?;
    let id = created.action.unwrap();
    assert_eq!(action(&h, id).await?.state, ActionState::Done);
    assert_eq!(h.fake.count(Call::Ban), 1);
    assert_eq!(
        attempts(&h, id).await?,
        vec![Some(AttemptOutcome::Unknown), Some(AttemptOutcome::Done)]
    );
    Ok(())
}

#[tokio::test]
async fn a_lost_timeout_answer_never_moves_the_end() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(one("Spam", TIMEOUT, false)).await?;
    let created = h.apply(MEMBER, rule).await?;
    h.fake.script(Call::Timeout, Script::LoseResponse);
    h.fake.script(Call::Timeout, Script::Unknown);
    h.settle().await?;
    let id = created.action.unwrap();
    let a = action(&h, id).await?;
    assert_eq!(a.state, ActionState::Done);
    assert!(a.no_change);
    // One attempt took effect; the retry read it back and made no call.
    let calls = h.fake.calls_of(Call::Timeout);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].until, a.timeout_ends_at);
    Ok(())
}

#[tokio::test]
async fn a_lost_kick_answer_is_not_kicked_twice() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(one("Spam", KICK, false)).await?;
    let created = h.apply(MEMBER, rule).await?;
    h.fake.script(Call::Kick, Script::LoseResponse);
    h.settle().await?;
    let a = action(&h, created.action.unwrap()).await?;
    assert_eq!((a.state, a.no_change), (ActionState::Done, true));
    assert_eq!(h.fake.count(Call::Kick), 1);
    Ok(())
}

#[tokio::test]
async fn a_member_who_left_and_rejoined_is_not_kicked() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(one("Spam", KICK, false)).await?;
    let created = h.apply(MEMBER, rule).await?;
    h.fake.world(|w| {
        if let Some(m) = w.member_mut(h.guild, sf(MEMBER)) {
            m.joined_at = Some(OffsetDateTime::now_utc() + time::Duration::minutes(1));
        }
    });
    h.settle().await?;
    let a = action(&h, created.action.unwrap()).await?;
    assert_eq!((a.state, a.no_change), (ActionState::Done, true));
    assert_eq!(h.fake.count(Call::Kick), 0);
    Ok(())
}

#[tokio::test]
async fn a_crash_before_the_call_settles_interrupted_and_retries() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(one("Spam", BAN, false)).await?;
    let created = h.apply(MEMBER, rule).await?;
    let gate = h.fake.hold(Call::GetBan);
    h.crash_at(JobKind::RunAction, &gate).await?;
    let id = created.action.unwrap();
    assert_eq!(action(&h, id).await?.state, ActionState::Running);
    h.settle().await?;
    assert_eq!(action(&h, id).await?.state, ActionState::Done);
    assert_eq!(h.fake.count(Call::Ban), 1);
    assert_eq!(
        attempts(&h, id).await?,
        vec![
            Some(AttemptOutcome::Interrupted),
            Some(AttemptOutcome::Done)
        ]
    );
    Ok(())
}

#[tokio::test]
async fn a_crash_after_the_call_reads_the_ban_back() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(one("Spam", BAN, false)).await?;
    let created = h.apply(MEMBER, rule).await?;
    let gate = h.fake.hold_after(Call::Ban);
    h.crash_at(JobKind::RunAction, &gate).await?;
    h.settle().await?;
    let id = created.action.unwrap();
    assert_eq!(action(&h, id).await?.state, ActionState::Done);
    assert_eq!(h.fake.count(Call::Ban), 1);
    assert_eq!(
        attempts(&h, id).await?,
        vec![
            Some(AttemptOutcome::Interrupted),
            Some(AttemptOutcome::Done)
        ]
    );
    Ok(())
}

#[tokio::test]
async fn eight_unknown_attempts_end_unconfirmed() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(one("Spam", BAN, false)).await?;
    let created = h.apply(MEMBER, rule).await?;
    h.fake.script_n(Call::Ban, 8, &Script::Unknown);
    h.settle().await?;
    let a = action(&h, created.action.unwrap()).await?;
    assert_eq!(a.state, ActionState::Failed);
    assert_eq!(
        a.failure.map(|f| f.reason),
        Some(ActionFailure::Unconfirmed)
    );
    assert_eq!(h.fake.count(Call::Ban), 8);
    assert!(log_kinds(&h).await?.contains(&ModLogKind::ActionFailed));
    Ok(())
}

#[tokio::test]
async fn reads_that_never_answer_end_retries_exhausted() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(one("Spam", BAN, false)).await?;
    let created = h.apply(MEMBER, rule).await?;
    h.fake.script_n(Call::GetBan, 8, &Script::NotSent);
    h.settle().await?;
    let a = action(&h, created.action.unwrap()).await?;
    assert_eq!(
        a.failure.map(|f| f.reason),
        Some(ActionFailure::RetriesExhausted)
    );
    assert_eq!(h.fake.count(Call::Ban), 0);
    Ok(())
}

#[tokio::test]
async fn a_claim_beyond_the_budget_is_finalized_without_calling_discord() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(one("Spam", BAN, false)).await?;
    let created = h.apply(MEMBER, rule).await?;
    // Every run crashed before settling; the budget is spent.
    h.db.execute("UPDATE jobs SET attempts = 8 WHERE kind = 'run_action'")
        .await?;
    h.fake.clear_calls();
    let ran = h.run_one(JobKind::RunAction).await?.unwrap();
    assert_eq!(ran.end, RunEnd::Settled(quack_db::jobs::JobResult::Done));
    assert!(h.fake.calls().is_empty());
    let a = action(&h, created.action.unwrap()).await?;
    assert_eq!(
        a.failure.map(|f| f.reason),
        Some(ActionFailure::RetriesExhausted)
    );
    Ok(())
}

#[tokio::test]
async fn a_longer_timeout_is_left_alone() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(one("Spam", TIMEOUT, false)).await?;
    let later = OffsetDateTime::now_utc() + time::Duration::days(2);
    h.fake.world(|w| {
        if let Some(m) = w.member_mut(h.guild, sf(MEMBER)) {
            m.timeout_until = Some(later);
        }
    });
    let created = h.apply(MEMBER, rule).await?;
    h.settle().await?;
    let a = action(&h, created.action.unwrap()).await?;
    assert_eq!((a.state, a.no_change), (ActionState::Done, true));
    assert_eq!(h.fake.count(Call::Timeout), 0);
    Ok(())
}

#[tokio::test]
async fn a_timeout_whose_end_passed_fails() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(one("Spam", TIMEOUT, false)).await?;
    let created = h.apply(MEMBER, rule).await?;
    let id = created.action.unwrap();
    h.db.execute(&format!(
        "UPDATE actions SET timeout_ends_at = now() - interval '1 second' WHERE id = '{}'",
        id.get()
    ))
    .await?;
    h.settle().await?;
    let a = action(&h, id).await?;
    assert_eq!(
        a.failure.map(|f| f.reason),
        Some(ActionFailure::TimeoutWindowPassed)
    );
    assert_eq!(h.fake.count(Call::Timeout), 0);
    Ok(())
}

#[tokio::test]
async fn a_definite_refusal_fails_with_discords_code() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(one("Spam", BAN, false)).await?;
    let created = h.apply(MEMBER, rule).await?;
    h.fake
        .script(Call::Ban, Script::refuse(RefusalKind::MissingPermissions));
    h.settle().await?;
    let a = action(&h, created.action.unwrap()).await?;
    let failure = a.failure.unwrap();
    assert_eq!(failure.reason, ActionFailure::DiscordRefused);
    assert_eq!(
        (failure.http_status, failure.discord_code),
        (Some(403), Some(50_013))
    );
    Ok(())
}

#[tokio::test]
async fn void_first_cancels_without_calling_discord() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(one("Spam", BAN, false)).await?;
    let created = h.apply(MEMBER, rule).await?;
    void(&h, created.case.id).await?;
    h.settle().await?;
    let id = created.action.unwrap();
    assert_eq!(action(&h, id).await?.state, ActionState::Cancelled);
    assert!(undo_of(&h, id).await?.is_none());
    assert_eq!(h.fake.count(Call::GetBan) + h.fake.count(Call::Ban), 0);
    Ok(())
}

#[tokio::test]
async fn void_after_start_queues_an_undo_that_waits_for_the_attempt() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(one("Spam", BAN, false)).await?;
    let created = h.apply(MEMBER, rule).await?;
    let id = created.action.unwrap();
    let gate = h.fake.hold(Call::GetBan);
    let task = h.spawn_one(JobKind::RunAction).await?;
    gate.reached().await;
    void(&h, created.case.id).await?;
    let undo = undo_of(&h, id).await?.expect("the undo is queued");
    assert_eq!(undo.kind, ActionKind::Unban);
    // The undo can't start while the original's attempt is in flight.
    let ran = h.run_one(JobKind::RunAction).await?.unwrap();
    assert!(matches!(
        ran.end,
        RunEnd::Settled(quack_db::jobs::JobResult::Defer { .. })
    ));
    gate.release();
    task.await?;
    h.settle().await?;
    assert_eq!(action(&h, id).await?.state, ActionState::Done);
    assert_eq!(action(&h, undo.id).await?.state, ActionState::Done);
    assert_eq!((h.fake.count(Call::Ban), h.fake.count(Call::Unban)), (1, 1));
    assert!(h.fake.world(|w| w.ban(h.guild, sf(MEMBER)).is_none()));
    Ok(())
}

#[tokio::test]
async fn a_lost_ban_then_void_unbans_by_the_marker() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(one("Spam", BAN, false)).await?;
    let created = h.apply(MEMBER, rule).await?;
    h.fake.script(Call::Ban, Script::LoseResponse);
    h.run_jobs().await?;
    let id = created.action.unwrap();
    // The ban is on Discord, but Quack only knows the attempt is unknown.
    assert_eq!(action(&h, id).await?.state, ActionState::Pending);
    void(&h, created.case.id).await?;
    h.settle().await?;
    let undo = undo_of(&h, id)
        .await?
        .expect("undo queued after an unknown attempt");
    assert_eq!(action(&h, undo.id).await?.state, ActionState::Done);
    assert_eq!(action(&h, id).await?.state, ActionState::Cancelled);
    assert!(h.fake.world(|w| w.ban(h.guild, sf(MEMBER)).is_none()));
    assert_eq!(h.fake.count(Call::Ban), 1);
    Ok(())
}

#[tokio::test]
async fn a_void_during_the_call_never_leaves_a_ban_on_a_voided_case() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(one("Spam", BAN, false)).await?;
    let created = h.apply(MEMBER, rule).await?;
    let gate = h.fake.hold_after(Call::Ban);
    let task = h.spawn_one(JobKind::RunAction).await?;
    gate.reached().await;
    void(&h, created.case.id).await?;
    gate.release();
    task.await?;
    h.settle().await?;
    assert!(h.fake.world(|w| w.ban(h.guild, sf(MEMBER)).is_none()));
    Ok(())
}

/// Engine review MAJOR 13: Discord bans and deletes messages but the answer
/// is lost; someone lifts the ban outside Quack before the retry. The
/// retry bans again (the case still applies) but never deletes messages a
/// second time.
#[tokio::test]
async fn a_ban_retry_after_a_lost_answer_deletes_no_messages() -> Result<()> {
    let h = Harness::new().await?;
    let ban = quack_core::rule::LevelAction::Ban {
        delete_message_seconds: 3600,
    };
    let rule = h
        .rule(quack_testkit::rules_for_tests::one("Spam", ban, false))
        .await?;
    h.apply(MEMBER, rule).await?;
    h.fake.script(Call::Ban, Script::LoseResponse);
    h.run_one(JobKind::RunAction).await?;
    h.fake.world(|w| {
        if let Some(g) = w.guilds.get_mut(&h.guild) {
            g.bans.remove(&sf(MEMBER));
        }
    });
    h.settle().await?;
    let reasons: Vec<String> = h
        .fake
        .calls_of(Call::Ban)
        .into_iter()
        .filter_map(|c| c.reason)
        .collect();
    assert_eq!(reasons.len(), 2, "{reasons:?}");
    assert!(reasons[0].ends_with("(delete 3600s)"), "{reasons:?}");
    assert!(reasons[1].ends_with("(delete 0s)"), "{reasons:?}");
    Ok(())
}

/// Engine review MAJOR 16: a run without room for its calls before the
/// claim's deadline opens no attempt and calls nothing; a new claim does.
#[tokio::test]
async fn a_run_short_of_time_starts_no_attempt() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(one("Spam", BAN, false)).await?;
    let created = h.apply(MEMBER, rule).await?;
    let id = created.action.expect("an action");
    let mut job = h
        .claim(JobKind::RunAction)
        .await?
        .expect("the action's job");
    job.deadline = std::time::Instant::now() + std::time::Duration::from_secs(12);
    let ran = quack_testkit::run_claimed(&h.engine, job).await;
    assert!(
        matches!(
            ran.end,
            RunEnd::Settled(quack_db::jobs::JobResult::Continue)
        ),
        "{:?}",
        ran.end
    );
    assert_eq!(h.fake.count(Call::Ban), 0);
    let opened = h.db.count("SELECT count(*) FROM action_attempts").await?;
    assert_eq!(opened, 0);
    h.settle().await?;
    assert_eq!(action(&h, id).await?.state, ActionState::Done);
    Ok(())
}
