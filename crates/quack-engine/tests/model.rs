//! A model-based test: random interleavings of applying rules, voiding,
//! running jobs, crashing mid-run (before a call, and after Discord applied
//! it), losing Discord's answers, requests that land late (after later
//! reads), a void racing an attempt held mid-call, DM windows that pass
//! while a run waits, refusals, staff retries, and members rejoining. Once
//! the queue is quiet:
//!
//! - no ban or timeout Quack placed stays in force for a voided case,
//!   unless its undo is queued and ended where staff see it (`failed`
//!   with a recorded reason, or `skipped`), or (a timeout) it is another
//!   case's of the same member that ends at the same second and is
//!   accounted for itself;
//! - every case's case count matched the model's count when it was made;
//! - every DM produced at most one message (counted by its nonce, so one
//!   case's duplicate can't hide behind another's missing message); a
//!   `sent` DM has exactly one, and a `failed` or `not_needed` DM none;
//! - an apply either made a case or was refused for a domain reason;
//! - nothing is left in progress.
//!
//! The fake assumes Discord applies a moderation request no later than a
//! later request about the same member that it answers.

use std::time::Duration;

use anyhow::{Context as _, Result, ensure};
use proptest::prelude::*;
use quack_core::action::{ActionKind, ActionState};
use quack_core::dm::DmState;
use quack_core::ids::{CaseId, RuleId};
use quack_core::rule::RuleDefinition;
use quack_db::jobs::JobKind;
use quack_engine::CaseRef;
use quack_engine::actions::retry;
use quack_engine::cases::{Applied, ApplyError, apply_rule, void_case};
use quack_engine::port::RefusalKind;
use quack_testkit::fake::{Call, Gate, MemberSetup, Script};
use quack_testkit::rules_for_tests::{BAN, KICK, TIMEOUT, levels};
use quack_testkit::{ENGINE_KINDS, Harness, MEMBER, MEMBER_2, sf};

#[derive(Clone, Debug)]
enum Op {
    Apply { rule: usize, member: usize },
    Void { case: usize },
    RunOne { kind: usize },
    Settle,
    FastForward,
    Lose { call: usize },
    Refuse { call: usize },
    Crash { at: usize },
    Retry { action: usize },
    Rejoin { member: usize },
    LateLoss { call: usize },
    Land,
    CrashAfter { call: usize },
    RaceVoid { call: usize, case: usize },
    ExpireDmWindow,
    Age,
    LateThenVoid { case: usize },
}

/// The moderation calls (indexes into `CALLS`).
const MODERATION: [usize; 5] = [0, 1, 2, 3, 4];

const MEMBERS: [u64; 2] = [MEMBER, MEMBER_2];
const CALLS: [Call; 6] = [
    Call::Ban,
    Call::Timeout,
    Call::Kick,
    Call::Unban,
    Call::RemoveTimeout,
    Call::Send,
];

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => (0..4_usize, 0..2_usize).prop_map(|(rule, member)| Op::Apply { rule, member }),
        2 => (0..8_usize).prop_map(|case| Op::Void { case }),
        3 => (0..ENGINE_KINDS.len()).prop_map(|kind| Op::RunOne { kind }),
        1 => Just(Op::Settle),
        1 => Just(Op::FastForward),
        1 => (0..CALLS.len()).prop_map(|call| Op::Lose { call }),
        1 => (0..CALLS.len()).prop_map(|call| Op::Refuse { call }),
        1 => (0..2_usize).prop_map(|at| Op::Crash { at }),
        1 => (0..8_usize).prop_map(|action| Op::Retry { action }),
        1 => (0..2_usize).prop_map(|member| Op::Rejoin { member }),
        1 => (0..MODERATION.len()).prop_map(|call| Op::LateLoss { call }),
        1 => Just(Op::Land),
        1 => (0..MODERATION.len()).prop_map(|call| Op::CrashAfter { call }),
        1 => (0..2_usize, 0..8_usize).prop_map(|(call, case)| Op::RaceVoid { call, case }),
        1 => Just(Op::ExpireDmWindow),
        1 => Just(Op::Age),
        2 => (0..8_usize).prop_map(|case| Op::LateThenVoid { case }),
    ]
}

struct ModelCase {
    id: CaseId,
    member: usize,
    rule: usize,
    voided: bool,
}

fn rules() -> Vec<RuleDefinition> {
    vec![
        levels("Warn", &[None], true),
        levels("Mute", &[Some(TIMEOUT)], true),
        levels("Kick", &[Some(KICK)], true),
        levels("Ban", &[Some(BAN)], true),
    ]
}

async fn step(h: &Harness, ids: &[RuleId], model: &mut Vec<ModelCase>, op: Op) -> Result<()> {
    match op {
        Op::Apply { rule, member } => {
            let expected = 1 + model
                .iter()
                .filter(|c| c.member == member && c.rule == rule && !c.voided)
                .count();
            match apply_rule(&h.engine, h.request(MEMBERS[member], ids[rule])).await {
                Ok(Applied::Created(created)) => {
                    let count = created.case.decision.context("decision")?.case_count;
                    ensure!(
                        count as usize == expected,
                        "case count {count}, model {expected}"
                    );
                    model.push(ModelCase {
                        id: created.case.id,
                        member,
                        rule,
                        voided: false,
                    });
                }
                // A member who left or was banned can't get a case; Discord
                // may be scripted to refuse the facts read.
                Err(ApplyError::Refused(_) | ApplyError::DiscordUnavailable) => {}
                other => anyhow::bail!("unexpected apply result: {other:?}"),
            }
        }
        Op::Void { case } => {
            if let Some(c) = model.get_mut(case) {
                let voided = void_case(
                    &h.engine,
                    &h.moderator(),
                    CaseRef::Id(c.id),
                    "Model.",
                    &h.key("vdm"),
                )
                .await;
                if voided.is_ok() {
                    c.voided = true;
                }
            }
        }
        Op::RunOne { kind } => {
            h.run_one(ENGINE_KINDS[kind]).await?;
        }
        Op::Settle => {
            h.settle().await?;
        }
        Op::FastForward => h.fast_forward().await?,
        Op::Lose { call } => h.fake.script(CALLS[call], Script::LoseResponse),
        Op::Refuse { call } => h
            .fake
            .script(CALLS[call], Script::refuse(RefusalKind::MissingPermissions)),
        Op::Crash { at } => crash(h, if at == 0 { Call::GetBan } else { Call::Member }).await?,
        Op::Retry { action } => {
            let mut conn = h.db.db.conn().await?;
            let failed =
                quack_db::actions::list(&mut conn, h.guild, ActionState::Failed, false, None, 50)
                    .await?;
            drop(conn);
            if let Some(item) = failed.rows.get(action) {
                let _ = retry(&h.engine, &h.moderator(), item.action.id, &h.key("ar")).await;
            }
        }
        Op::LateLoss { call } => h.fake.script(CALLS[MODERATION[call]], Script::Late),
        Op::Land => {
            h.fake.land_late();
        }
        Op::CrashAfter { call } => crash_after(h, CALLS[MODERATION[call]]).await?,
        Op::RaceVoid { call, case } => {
            let id = model.get(case).map(|c| c.id);
            if race_void(h, if call == 0 { Call::Ban } else { Call::Timeout }, id).await?
                && let Some(c) = model.get_mut(case)
            {
                c.voided = true;
            }
        }
        Op::ExpireDmWindow => expire_dm_window(h).await?,
        Op::Age => age_all(h).await?,
        Op::LateThenVoid { case } => late_then_void(h, model, case).await?,
        Op::Rejoin { member } => {
            let id = MEMBERS[member];
            h.fake.world(|w| {
                if w.member(h.guild, sf(id)).is_none() && w.ban(h.guild, sf(id)).is_none() {
                    let mut setup = MemberSetup::new(sf(id), 5);
                    setup.joined_at = time::OffsetDateTime::now_utc();
                    w.add_member(h.guild, &setup);
                }
            });
        }
    }
    Ok(())
}

type Task = tokio::task::JoinHandle<quack_testkit::Ran>;

/// Waits until the run reaches `gate` (`Some`: it is held there) or ends
/// without reaching it (`None`).
async fn held(mut task: Task, gate: &Gate) -> Option<Task> {
    tokio::select! {
        () = gate.reached() => Some(task),
        _ = &mut task => None,
    }
}

/// Kills the next action run at its next `call` (if it gets there).
async fn crash(h: &Harness, call: Call) -> Result<()> {
    let gate = h.fake.hold(call);
    if let Ok(task) = h.spawn_one(JobKind::RunAction).await
        && let Some(task) = held(task, &gate).await
    {
        task.abort();
        let _ = task.await;
    }
    h.fake.clear_scripts();
    gate.release();
    h.expire_leases().await
}

/// Kills the next action run right after Discord applied its `call` (the
/// answer never reached Quack).
async fn crash_after(h: &Harness, call: Call) -> Result<()> {
    let gate = h.fake.hold_after(call);
    if let Ok(task) = h.spawn_one(JobKind::RunAction).await
        && let Some(task) = held(task, &gate).await
    {
        task.abort();
        let _ = task.await;
    }
    h.fake.clear_scripts();
    gate.release();
    h.expire_leases().await
}

/// Holds the next action run inside its `call`, voids `case` while it is
/// held, then lets the call go on. Returns whether the void went through.
async fn race_void(h: &Harness, call: Call, case: Option<CaseId>) -> Result<bool> {
    let gate = h.fake.hold(call);
    let mut voided = false;
    if let Ok(task) = h.spawn_one(JobKind::RunAction).await
        && let Some(task) = held(task, &gate).await
    {
        if let Some(id) = case {
            voided = void_case(
                &h.engine,
                &h.moderator(),
                CaseRef::Id(id),
                "Model.",
                &h.key("vdm"),
            )
            .await
            .is_ok();
        }
        gate.release();
        let _ = task.await;
    }
    h.fake.clear_scripts();
    gate.release();
    Ok(voided)
}

/// An attempt's request reaches Discord but lands only later; the case is
/// voided before any retry confirms it.
async fn late_then_void(h: &Harness, model: &mut [ModelCase], case: usize) -> Result<()> {
    h.fake.script(Call::Ban, Script::Late);
    h.fake.script(Call::Timeout, Script::Late);
    age_all(h).await?;
    h.run_one(JobKind::RunAction).await?;
    h.fake.clear_scripts();
    if let Some(c) = model.get_mut(case)
        && void_case(
            &h.engine,
            &h.moderator(),
            CaseRef::Id(c.id),
            "Model.",
            &h.key("vdm"),
        )
        .await
        .is_ok()
    {
        c.voided = true;
    }
    Ok(())
}

/// Every case is two hundred seconds older: a kick or ban no longer waits
/// for its DM (§6.3).
async fn age_all(h: &Harness) -> Result<()> {
    h.db.execute("UPDATE cases SET created_at = created_at - interval '200 seconds'")
        .await?;
    Ok(())
}

/// A DM run loses its first create's answer, then wakes after the window
/// passed and Discord forgot the nonce.
async fn expire_dm_window(h: &Harness) -> Result<()> {
    h.fake.script(Call::Send, Script::LoseResponse);
    let before = h.fake.count(Call::Send);
    if let Ok(task) = h.spawn_one(JobKind::SendDm).await {
        for _ in 0..400 {
            if h.fake.count(Call::Send) > before || task.is_finished() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        h.db.execute(
            "UPDATE dms SET first_attempt_at = first_attempt_at - interval '61 seconds' \
             WHERE state = 'sending'",
        )
        .await?;
        h.fake.expire_nonces();
        let _ = task.await;
    }
    h.fake.clear_scripts();
    Ok(())
}

/// Whether a case's undo ended where staff see it: `failed` (Discord
/// definitely refused, or the original stayed unconfirmed; retry from the
/// failed-actions page) or `skipped` (left in place, reported).
fn undo_reported(actions: &[quack_db::actions::Action]) -> bool {
    actions.iter().any(|a| {
        a.undo_of.is_some() && matches!(a.state, ActionState::Failed | ActionState::Skipped)
    })
}

async fn check(h: &Harness, model: &[ModelCase]) -> Result<()> {
    let mut conn = h.db.db.conn().await?;
    let now = time::OffsetDateTime::now_utc();
    for c in model {
        let case = quack_db::cases::get(&mut conn, h.guild, c.id)
            .await?
            .context("case")?;
        ensure!(case.is_valid() != c.voided, "case {} validity", case.number);
        let actions = quack_db::actions::for_case(&mut conn, h.guild, c.id).await?;
        for a in &actions {
            ensure!(
                !matches!(a.state, ActionState::Pending | ActionState::Running),
                "case {} action {:?} still {:?}",
                case.number,
                a.kind,
                a.state
            );
        }
        let dms = quack_db::dms::for_case(&mut conn, h.guild, c.id).await?;
        for d in &dms {
            ensure!(
                !matches!(d.state, DmState::Pending | DmState::Sending),
                "DM still in progress"
            );
            let nonce = quack_engine::port::Nonce::new(d.id.to_u22());
            let messages = h.fake.world(|w| w.created_with(nonce.as_str()));
            ensure!(
                messages <= 1,
                "case {}: {messages} messages for one DM",
                case.number
            );
            match d.state {
                DmState::Sent => ensure!(messages == 1, "a sent DM without its message"),
                DmState::Failed | DmState::NotNeeded => {
                    ensure!(messages == 0, "a {:?} DM has a message", d.state);
                }
                _ => {}
            }
        }
        if !c.voided {
            continue;
        }
        let member = sf::<quack_core::ids::MemberId>(MEMBERS[c.member]);
        // Whatever is still in force for a voided case must have its undo
        // queued, and that undo ended where staff see it.
        let reported = undo_reported(&actions);
        let marker = h.fake.world(|w| {
            w.ban(h.guild, member)
                .and_then(quack_core::permissions::BanFacts::case_marker)
        });
        ensure!(
            marker != Some(case.number) || reported,
            "a ban for voided case {} is in force",
            case.number
        );
        let end = actions
            .iter()
            .find(|a| a.kind == ActionKind::Timeout)
            .and_then(|a| a.timeout_ends_at);
        let current = h
            .fake
            .world(|w| w.member(h.guild, member).and_then(|m| m.timeout_until));
        if let (Some(end), Some(current)) = (end, current)
            && end == current
            && current > now
        {
            // Two timeouts made in the same second end at the same second
            // (§20.4 #9), so the one in force may be another case's: a valid
            // case's that Quack placed, or a voided case's that may have
            // applied and whose undo staff see (it can land after this
            // case's timeout was cancelled unsent).
            let mut owned_elsewhere = false;
            for other in model
                .iter()
                .filter(|o| o.member == c.member && o.id != c.id)
            {
                let theirs = quack_db::actions::for_case(&mut conn, h.guild, other.id).await?;
                let placed = theirs.iter().any(|a| {
                    a.kind == ActionKind::Timeout
                        && a.timeout_ends_at == Some(current)
                        && if other.voided {
                            a.attempts > 0
                        } else {
                            a.state == ActionState::Done
                        }
                });
                owned_elsewhere |= placed && (!other.voided || undo_reported(&theirs));
            }
            ensure!(
                reported || owned_elsewhere,
                "a timeout for voided case {} is in force",
                case.number
            );
        }
    }
    Ok(())
}

async fn run(ops: Vec<Op>) -> Result<()> {
    let h = Harness::new().await?;
    let mut ids = Vec::new();
    for def in rules() {
        ids.push(h.rule(def).await?);
    }
    let mut model = Vec::new();
    for op in ops {
        step(&h, &ids, &mut model, op).await?;
    }
    h.fake.clear_scripts();
    h.settle().await?;
    // Whatever was still on its way lands now, after everything settled.
    h.fake.land_late();
    h.settle().await?;
    check(&h, &model).await
}

proptest! {
    // About 0.6–0.8 seconds per case against a real database: 150 keeps the
    // whole engine suite near two minutes.
    #![proptest_config(ProptestConfig { cases: 150, ..ProptestConfig::default() })]
    #[test]
    fn invariants_hold_under_any_interleaving(ops in prop::collection::vec(op(), 1..30)) {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let result = rt.block_on(run(ops));
        prop_assert!(result.is_ok(), "{result:?}");
    }
}
