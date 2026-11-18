//! DMs (§6, §20.1.1 scenarios 5 and 13).

mod common;

use std::time::Duration;

use anyhow::{Context as _, Result};
use common::{action, dm, log_kinds, void};
use quack_core::action::ActionState;
use quack_core::dm::{DmFailure, DmState};
use quack_core::modlog::ModLogKind;
use quack_db::jobs::JobKind;
use quack_engine::port::{Refusal, RefusalKind};
use quack_testkit::fake::{Call, Script};
use quack_testkit::rules_for_tests::{BAN, KICK, TIMEOUT, one, warning};
use quack_testkit::{Harness, MEMBER, sf};

fn dms_to_member(h: &Harness) -> usize {
    h.fake.world(|w| w.dms_to(sf(MEMBER)).len())
}

async fn warned(h: &Harness) -> Result<quack_core::ids::DmId> {
    let rule = h.rule(warning("Spam")).await?;
    h.apply(MEMBER, rule).await?.dm.context("a DM")
}

#[tokio::test]
async fn a_case_dm_is_sent_once() -> Result<()> {
    let h = Harness::new().await?;
    let id = warned(&h).await?;
    h.settle().await?;
    let d = dm(&h, id).await?;
    assert_eq!(d.state, DmState::Sent);
    assert_eq!(dms_to_member(&h), 1);
    assert!(log_kinds(&h).await?.contains(&ModLogKind::DmSent));
    Ok(())
}

#[tokio::test]
async fn each_refusal_reason_is_recorded() -> Result<()> {
    for (refusal, failure) in [
        (Refusal::of(RefusalKind::CannotDm), DmFailure::DmsClosed),
        (
            Refusal::of(RefusalKind::NoMutualGuilds),
            DmFailure::NotInSharedServer,
        ),
        (Refusal::of(RefusalKind::Other), DmFailure::DiscordRefused),
    ] {
        let h = Harness::new().await?;
        let id = warned(&h).await?;
        h.fake.script(Call::Send, Script::Refuse(refusal));
        h.settle().await?;
        let d = dm(&h, id).await?;
        assert_eq!((d.state, d.failure), (DmState::Failed, Some(failure)));
        assert_eq!(d.failure_discord_code, refusal.code);
        assert_eq!(dms_to_member(&h), 0);
        assert!(log_kinds(&h).await?.contains(&ModLogKind::DmFailed));
    }
    Ok(())
}

#[tokio::test]
async fn a_dm_channel_that_cannot_be_opened() -> Result<()> {
    let h = Harness::new().await?;
    let id = warned(&h).await?;
    h.fake
        .script(Call::OpenDm, Script::refuse(RefusalKind::CannotDm));
    h.settle().await?;
    assert_eq!(dm(&h, id).await?.failure, Some(DmFailure::DmsClosed));
    // An open that never answers spends the budget: discord_unavailable.
    let h = Harness::new().await?;
    let id = warned(&h).await?;
    h.fake.script_n(Call::OpenDm, 3, &Script::Unknown);
    h.settle().await?;
    let d = dm(&h, id).await?;
    assert_eq!(
        (d.state, d.failure),
        (DmState::Failed, Some(DmFailure::DiscordUnavailable))
    );
    assert_eq!(h.fake.count(Call::Send), 0);
    Ok(())
}

#[tokio::test]
async fn a_crash_while_sending_resumes_with_the_same_nonce() -> Result<()> {
    let h = Harness::new().await?;
    let id = warned(&h).await?;
    let gate = h.fake.hold_after(Call::Send);
    h.crash_at(JobKind::SendDm, &gate).await?;
    assert_eq!(dm(&h, id).await?.state, DmState::Sending);
    h.settle().await?;
    assert_eq!(dm(&h, id).await?.state, DmState::Sent);
    let sends = h.fake.calls_of(Call::Send);
    assert_eq!(sends.len(), 2);
    assert_eq!(sends[0].nonce, sends[1].nonce);
    // Discord's nonce returned the first message: the member got one DM.
    assert_eq!(dms_to_member(&h), 1);
    Ok(())
}

#[tokio::test]
async fn a_dm_found_sending_after_the_window_is_unknown_and_never_resent() -> Result<()> {
    let h = Harness::new().await?;
    let id = warned(&h).await?;
    let gate = h.fake.hold_after(Call::Send);
    h.crash_at(JobKind::SendDm, &gate).await?;
    h.db.execute("UPDATE dms SET first_attempt_at = first_attempt_at - interval '61 seconds'")
        .await?;
    h.settle().await?;
    assert_eq!(dm(&h, id).await?.state, DmState::Unknown);
    assert_eq!(h.fake.count(Call::Send), 1);
    assert_eq!(dms_to_member(&h), 1);
    Ok(())
}

#[tokio::test]
async fn a_resumed_dm_rate_limited_past_the_window_is_unknown() -> Result<()> {
    let h = Harness::new().await?;
    let id = warned(&h).await?;
    // The first create reaches Discord, then the worker crashes.
    let gate = h.fake.hold_after(Call::Send);
    h.crash_at(JobKind::SendDm, &gate).await?;
    let first = dm(&h, id).await?.first_attempt_at;
    // The resumed run is rate limited beyond the window: going back to
    // `pending` would lose track of the possible send and allow a second.
    h.fake
        .script(Call::Send, Script::RetryAfter(Duration::from_secs(120)));
    h.settle().await?;
    let d = dm(&h, id).await?;
    assert_eq!((d.state, d.failure), (DmState::Unknown, None));
    assert_eq!(d.first_attempt_at, first, "the window's start is kept");
    assert_eq!(dms_to_member(&h), 1);
    assert_eq!(h.fake.count(Call::Send), 2);
    Ok(())
}

#[tokio::test]
async fn a_resumed_dm_refused_is_unknown_not_failed() -> Result<()> {
    let h = Harness::new().await?;
    let id = warned(&h).await?;
    let gate = h.fake.hold_after(Call::Send);
    h.crash_at(JobKind::SendDm, &gate).await?;
    h.fake
        .script(Call::Send, Script::refuse(RefusalKind::CannotDm));
    h.settle().await?;
    let d = dm(&h, id).await?;
    assert_eq!((d.state, d.failure), (DmState::Unknown, None));
    Ok(())
}

#[tokio::test]
async fn not_sent_creates_never_make_a_dm_unknown() -> Result<()> {
    let h = Harness::new().await?;
    let id = warned(&h).await?;
    // Nothing reaches Discord, then Discord refuses: that is a definite
    // failure, not "couldn't confirm".
    h.fake.script(Call::Send, Script::NotSent);
    h.fake
        .script(Call::Send, Script::refuse(RefusalKind::CannotDm));
    h.settle().await?;
    let d = dm(&h, id).await?;
    assert_eq!(
        (d.state, d.failure),
        (DmState::Failed, Some(DmFailure::DmsClosed))
    );
    Ok(())
}

#[tokio::test]
async fn a_refusal_after_an_unknown_create_is_unknown() -> Result<()> {
    let h = Harness::new().await?;
    let id = warned(&h).await?;
    h.fake.script(Call::Send, Script::Unknown);
    h.fake
        .script(Call::Send, Script::refuse(RefusalKind::CannotDm));
    h.settle().await?;
    let d = dm(&h, id).await?;
    assert_eq!((d.state, d.failure), (DmState::Unknown, None));
    assert_eq!(h.fake.count(Call::Send), 2);
    Ok(())
}

#[tokio::test]
async fn a_rate_limit_beyond_the_window_goes_back_to_pending() -> Result<()> {
    let h = Harness::new().await?;
    let id = warned(&h).await?;
    h.fake
        .script(Call::Send, Script::RetryAfter(Duration::from_secs(120)));
    h.run_jobs().await?;
    let d = dm(&h, id).await?;
    assert_eq!((d.state, d.first_attempt_at), (DmState::Pending, None));
    h.settle().await?;
    assert_eq!(dm(&h, id).await?.state, DmState::Sent);
    assert_eq!(dms_to_member(&h), 1);
    Ok(())
}

/// Engine review MAJOR 12: on the budget's last run, a rate limit past
/// the window settles the DM at once instead of after Discord's delay.
#[tokio::test]
async fn a_rate_limit_on_the_last_run_fails_the_dm_without_waiting() -> Result<()> {
    let h = Harness::new().await?;
    let id = warned(&h).await?;
    h.db.execute("UPDATE jobs SET attempts = max_attempts - 1 WHERE kind = 'send_dm'")
        .await?;
    h.fake
        .script(Call::Send, Script::RetryAfter(Duration::from_secs(600)));
    h.run_jobs().await?;
    let d = dm(&h, id).await?;
    assert_eq!(
        (d.state, d.failure),
        (DmState::Failed, Some(DmFailure::DiscordUnavailable))
    );
    assert_eq!(dms_to_member(&h), 0);
    Ok(())
}

#[tokio::test]
async fn void_before_the_start_means_not_needed() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(warning("Spam")).await?;
    let created = h.apply(MEMBER, rule).await?;
    void(&h, created.case.id).await?;
    h.settle().await?;
    assert_eq!(
        dm(&h, created.dm.context("dm")?).await?.state,
        DmState::NotNeeded
    );
    assert_eq!(h.fake.count(Call::Send), 0);
    Ok(())
}

#[tokio::test]
async fn void_after_the_start_lets_the_dm_finish() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(warning("Spam")).await?;
    let created = h.apply(MEMBER, rule).await?;
    let gate = h.fake.hold(Call::Send);
    let task = h.spawn_one(JobKind::SendDm).await?;
    gate.reached().await;
    void(&h, created.case.id).await?;
    gate.release();
    task.await?;
    assert_eq!(
        dm(&h, created.dm.context("dm")?).await?.state,
        DmState::Sent
    );
    Ok(())
}

#[tokio::test]
async fn a_kick_waits_for_its_dm_and_the_dm_goes_first() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(one("Spam", KICK, true)).await?;
    let created = h.apply(MEMBER, rule).await?;
    // The kick's job is not due yet: it waits up to 120 s for the DM.
    assert!(h.run_one(JobKind::RunAction).await?.is_none());
    h.run_jobs().await?;
    assert_eq!(
        dm(&h, created.dm.context("dm")?).await?.state,
        DmState::Sent
    );
    // The DM's settle moved the kick's job to now.
    assert_eq!(
        action(&h, created.action.context("action")?).await?.state,
        ActionState::Done
    );
    let calls = h.fake.calls();
    let send = calls.iter().position(|c| c.call == Call::Send);
    let kick = calls.iter().position(|c| c.call == Call::Kick);
    assert!(send < kick, "the DM goes before the kick");
    Ok(())
}

#[tokio::test]
async fn a_failed_dm_still_releases_the_ban() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(one("Spam", BAN, true)).await?;
    let created = h.apply(MEMBER, rule).await?;
    h.fake
        .script(Call::Send, Script::refuse(RefusalKind::CannotDm));
    h.run_jobs().await?;
    assert_eq!(
        action(&h, created.action.context("action")?).await?.state,
        ActionState::Done
    );
    Ok(())
}

#[tokio::test]
async fn a_stuck_dm_delays_a_ban_by_at_most_120_seconds() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(one("Spam", BAN, true)).await?;
    let created = h.apply(MEMBER, rule).await?;
    let gate = h.fake.hold_after(Call::Send);
    let task = h.spawn_one(JobKind::SendDm).await?;
    gate.reached().await;
    // The DM is `sending`; the ban defers while the case is young.
    h.fast_forward().await?;
    h.run_one(JobKind::RunAction).await?;
    let id = created.action.context("action")?;
    assert_eq!(action(&h, id).await?.state, ActionState::Pending);
    // 121 seconds later the ban goes ahead without the DM.
    h.age_case(created.case.id, 121).await?;
    h.fast_forward().await?;
    h.run_one(JobKind::RunAction).await?;
    assert_eq!(action(&h, id).await?.state, ActionState::Done);
    gate.release();
    task.await?;
    assert_eq!(dms_to_member(&h), 1);
    Ok(())
}

#[tokio::test]
async fn a_timeout_and_its_dm_run_independently() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(one("Spam", TIMEOUT, true)).await?;
    let created = h.apply(MEMBER, rule).await?;
    h.fake
        .script(Call::Send, Script::refuse(RefusalKind::CannotDm));
    assert!(h.run_one(JobKind::RunAction).await?.is_some());
    h.settle().await?;
    assert_eq!(
        action(&h, created.action.context("action")?).await?.state,
        ActionState::Done
    );
    Ok(())
}

/// Engine review BLOCKER 2: Discord accepts a create and the answer is
/// lost; the run waits to repeat it, but wakes after the nonce window has
/// passed (and Discord has forgotten the nonce). It must settle `unknown`
/// instead of creating again: a second create would deliver a second DM.
#[tokio::test]
async fn a_repeat_never_starts_after_the_window_even_when_the_wait_ends_late() -> Result<()> {
    let h = Harness::new().await?;
    let id = warned(&h).await?;
    h.fake.script(Call::Send, Script::LoseResponse);
    let task = h.spawn_one(JobKind::SendDm).await?;
    // While the run waits to repeat the create, the window passes.
    for _ in 0..200 {
        if h.fake.count(Call::Send) == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(h.fake.count(Call::Send), 1);
    h.db.execute(&format!(
        "UPDATE dms SET first_attempt_at = first_attempt_at - interval '61 seconds' WHERE id = '{}'",
        id.get()
    ))
    .await?;
    h.fake.expire_nonces();
    task.await?;
    h.settle().await?;
    let d = dm(&h, id).await?;
    assert_eq!(d.state, DmState::Unknown);
    assert_eq!(h.fake.count(Call::Send), 1, "no create after the window");
    assert_eq!(dms_to_member(&h), 1);
    Ok(())
}

/// Engine review MAJOR 11: the case is voided while its DM is `sending`;
/// a rate limit then proves nothing was sent. The DM is no longer needed,
/// instead of going back to waiting and later failing.
#[tokio::test]
async fn a_dm_voided_while_sending_and_rate_limited_is_not_needed() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(warning("Spam")).await?;
    let created = h.apply(MEMBER, rule).await?;
    let id = created.dm.context("a DM")?;
    let gate = h
        .fake
        .hold_then(Call::Send, Script::RetryAfter(Duration::from_secs(600)));
    let task = h.spawn_one(JobKind::SendDm).await?;
    gate.reached().await;
    void(&h, created.case.id).await?;
    gate.release();
    task.await?;
    let d = dm(&h, id).await?;
    assert_eq!(d.state, DmState::NotNeeded);
    h.settle().await?;
    assert_eq!(dm(&h, id).await?.state, DmState::NotNeeded);
    assert_eq!(dms_to_member(&h), 0);
    Ok(())
}
