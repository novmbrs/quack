//! A model-based test: random interleavings of applying rules, voiding,
//! running jobs, crashing mid-run, losing Discord's answers, refusals,
//! staff retries, and members rejoining. Once the queue is quiet:
//!
//! - no ban or timeout Quack placed stays in force for a voided case,
//!   unless its undo is queued and ended where staff see it (`failed`
//!   after Discord refused, or `skipped`);
//! - every case's case count matched the model's count when it was made;
//! - a member got a DM message only for a DM that is `sent` or `unknown`,
//!   and every `sent` DM has its message;
//! - nothing is left in progress.

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
use quack_engine::cases::{Applied, apply_rule, void_case};
use quack_engine::port::RefusalKind;
use quack_testkit::fake::{Call, MemberSetup, Script};
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
}

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
            if let Ok(Applied::Created(created)) =
                apply_rule(&h.engine, h.request(MEMBERS[member], ids[rule])).await
            {
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
            if let Some(item) = failed.get(action) {
                let _ = retry(&h.engine, &h.moderator(), item.action.id, &h.key("ar")).await;
            }
        }
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

/// Kills the next action run at its next `call` (if it gets there).
async fn crash(h: &Harness, call: Call) -> Result<()> {
    let gate = h.fake.hold(call);
    if let Ok(task) = h.spawn_one(JobKind::RunAction).await {
        if tokio::time::timeout(Duration::from_secs(2), gate.reached())
            .await
            .is_ok()
        {
            task.abort();
        }
        let _ = task.await;
    }
    h.fake.clear_scripts();
    gate.release();
    h.expire_leases().await
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
        }
        if !c.voided {
            continue;
        }
        let member = sf::<quack_core::ids::MemberId>(MEMBERS[c.member]);
        // Whatever is still in force for a voided case must have its undo
        // queued, and that undo ended where staff see it: `failed` (Discord
        // definitely refused; retry from the failed-actions page) or
        // `skipped` (left in place, reported).
        let reported = actions.iter().any(|a| {
            a.undo_of.is_some() && matches!(a.state, ActionState::Failed | ActionState::Skipped)
        });
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
            // Two timeouts made in the same second end at the same second;
            // the one in force may be a valid case's (§20.4 #9).
            let mut owned_by_valid = false;
            for other in model.iter().filter(|o| !o.voided && o.member == c.member) {
                let theirs = quack_db::actions::for_case(&mut conn, h.guild, other.id).await?;
                owned_by_valid |= theirs.iter().any(|a| {
                    a.kind == ActionKind::Timeout
                        && a.state == ActionState::Done
                        && a.timeout_ends_at == Some(current)
                });
            }
            ensure!(
                reported || owned_by_valid,
                "a timeout for voided case {} is in force",
                case.number
            );
        }
    }
    for (i, id) in MEMBERS.iter().enumerate() {
        let messages = h.fake.world(|w| w.dms_to(sf(*id)).len());
        let delivered =
            h.db.count(&format!(
                "SELECT count(*) FROM dms WHERE member_id = {id} AND state IN ('sent', 'unknown')"
            ))
            .await?;
        let sent =
            h.db.count(&format!(
                "SELECT count(*) FROM dms WHERE member_id = {id} AND state = 'sent'"
            ))
            .await?;
        let messages = i64::try_from(messages)?;
        ensure!(
            messages <= delivered,
            "member {i}: {messages} DMs for {delivered} delivered"
        );
        ensure!(
            sent <= messages,
            "member {i}: {sent} sent DMs but {messages} messages"
        );
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
