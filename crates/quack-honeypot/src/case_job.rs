//! The `honeypot_case` job (§13.4.2 step 3): a member's pending hits become
//! one case (or join a recent one), or fail with a code. Safe to repeat:
//! case creation uses the batch's operation key, and every hit update is
//! guarded on `pending`.

use quack_core::DomainError;
use quack_core::case::MessageLink;
use quack_core::ids::{CaseId, GuildId, MemberId, MessageId};
use quack_core::messages::MessageKind;
use quack_core::modlog::{HoneypotCaseFailedDetail, ModLogEntry, ModLogLinks};
use quack_core::modules::{HoneypotFailure, HoneypotRefusal};
use quack_core::permissions::{ActorFacts, MemberRefusalCode, is_staff_for_access};
use quack_db::honeypot::{self, Hit};
use quack_db::jobs::{self as queue, ClaimedJob, Handled, JobResult, NewJob, RunAt};
use quack_db::messages::{self, Owner};
use quack_db::{DbError, Tx, guilds};
use quack_engine::cases::{Actor, ApplyError, ApplyRule, HoneypotAdmission, apply_rule};
use quack_engine::modlog::{self, By};
use quack_engine::port::{Fresh, Membership, Outcome};
use quack_engine::{Engine, OpKey};
use quack_telemetry::labels::HoneypotOutcome;
use quack_telemetry::metrics;

use quack_engine::jobs::module::{begin, dead, finish, retry};
use quack_engine::span;

/// The `honeypot_case` payload (the server is the job's).
#[derive(Clone, Copy, Debug, serde::Deserialize)]
struct Payload {
    member_id: MemberId,
}

/// How the batch ends.
enum Settled {
    /// The hits joined this case.
    Case(CaseId),
    /// The hits join this earlier case of a burst, if the policy they were
    /// admitted under still holds when they settle.
    Burst(CaseId),
    /// The hits failed with this code.
    Failed(HoneypotFailure),
}

/// Runs one batch: the member's pending hits admitted under the first
/// hit's settings version. Later hits under another version are left for
/// the next run (`Continue`).
pub(crate) async fn run(engine: &Engine, job: &ClaimedJob) -> Result<Handled, DbError> {
    let (Some(guild), Ok(p)) = (job.guild, job.payload::<Payload>()) else {
        return Ok(dead("bad_payload"));
    };
    span("job honeypot_case", guild, async {
        let (batch, more) = batch(engine, guild, p.member_id).await?;
        let Some(first) = batch.first() else {
            return Ok(Handled::Settle(JobResult::Done));
        };
        let settled = match decide(engine, guild, &batch).await {
            Ok(settled) => settled,
            Err(code) => return Ok(retry(job, code)),
        };
        let tx = begin(engine, job).await?;
        let next = if more {
            JobResult::Continue
        } else {
            JobResult::Done
        };
        settle(tx, job, guild, first, &batch, &settled, next).await
    })
    .await
}

/// The member's pending hits sharing the first hit's admission, and
/// whether others wait behind them.
async fn batch(
    engine: &Engine,
    guild: GuildId,
    member: MemberId,
) -> Result<(Vec<Hit>, bool), DbError> {
    let mut conn = engine.db().conn().await?;
    let mut hits = honeypot::pending_batch(&mut conn, guild, member).await?;
    let Some(first) = hits.first().cloned() else {
        return Ok((hits, false));
    };
    let same = |h: &Hit| h.settings_version == first.settings_version && h.rule == first.rule;
    let split = hits.iter().position(|h| !same(h)).unwrap_or(hits.len());
    let rest = hits.split_off(split);
    Ok((hits, !rest.is_empty()))
}

/// Steps 2–4: the exemption from current facts, a burst, or a new case.
/// `Err` carries the code to retry with.
async fn decide(engine: &Engine, guild: GuildId, batch: &[Hit]) -> Result<Settled, String> {
    let Some(first) = batch.first() else {
        return Err("empty_batch".to_owned());
    };
    let installed = {
        let mut conn = engine.db().conn().await.map_err(|e| e.to_string())?;
        guilds::get(&mut conn, guild)
            .await
            .map_err(|e| e.to_string())?
            .is_some_and(|g| g.installed)
    };
    if !installed {
        return Ok(refused(HoneypotRefusal::Member(
            MemberRefusalCode::QuackNotInServer,
        )));
    }
    if exempt(engine, guild, first.member).await? {
        return Ok(Settled::Failed(HoneypotFailure::Exempt));
    }
    let burst = {
        let mut conn = engine.db().conn().await.map_err(|e| e.to_string())?;
        honeypot::burst_case(
            &mut conn,
            guild,
            first.member,
            first.settings_version,
            first.created_at,
        )
        .await
        .map_err(|e| e.to_string())?
    };
    if let Some(case) = burst {
        return Ok(Settled::Burst(case));
    }
    let Some(rule) = first.rule else {
        return Ok(refused(HoneypotRefusal::RuleUnavailable));
    };
    let request = ApplyRule {
        guild,
        actor: Actor::Quack,
        member: first.member,
        rule,
        pinned_version: None,
        context: Vec::new(),
        evidence_links: Vec::new(),
        source_message: Some(MessageLink {
            guild_id: guild,
            channel_id: first.channel,
            message_id: first.message,
        }),
        receipt: None,
        replaces: None,
        op_key: OpKey::new("honeypot", format!("honeypot:{}", first.message)),
        draft: None,
        honeypot: Some(HoneypotAdmission {
            settings_version: first.settings_version,
            channel: first.channel,
        }),
    };
    // Boxed: the case path is a large future.
    match Box::pin(apply_rule(engine, request)).await {
        Ok(applied) => Ok(Settled::Case(applied.case_id())),
        Err(ApplyError::Refused(DomainError::MemberRefused(r))) => {
            Ok(refused(HoneypotRefusal::Member(r.code())))
        }
        Err(ApplyError::HoneypotPolicyChanged | ApplyError::OperationKeyReused) => {
            Ok(refused(HoneypotRefusal::PolicyChanged))
        }
        Err(
            ApplyError::Refused(_) | ApplyError::ContextInvalid(_) | ApplyError::LinksInvalid(_),
        ) => Ok(refused(HoneypotRefusal::RuleUnavailable)),
        Err(e) => Err(format!("apply_{e}")),
    }
}

const fn refused(refusal: HoneypotRefusal) -> Settled {
    Settled::Failed(HoneypotFailure::Refused(refusal))
}

/// Step 2: the poster is now staff for Discord access, or a bot.
async fn exempt(engine: &Engine, guild: GuildId, member: MemberId) -> Result<bool, String> {
    let discord = engine.discord();
    let Outcome::Done(facts) = discord.guild_facts(guild).await else {
        return Err("guild_facts".to_owned());
    };
    let m = match discord.member(guild, member, Fresh::Cached).await {
        Outcome::Done(Membership::Present(m)) => m,
        // Gone: a ban still applies, and the case path decides.
        Outcome::Done(Membership::Absent) => return Ok(false),
        other => return Err(other.code()),
    };
    let roles = {
        let mut conn = engine.db().conn().await.map_err(|e| e.to_string())?;
        quack_db::settings::get(&mut conn, guild)
            .await
            .map_err(|e| e.to_string())?
            .map_or_default(|s| s.moderator_role_ids)
    };
    let actor = ActorFacts {
        is_owner: member.account() == facts.owner_id,
        permissions: m.permissions,
        role_ids: m.role_ids.clone(),
        mfa_enabled: false,
    };
    Ok(m.bot || is_staff_for_access(&actor, &roles))
}

/// Steps 5–6 in one fenced transaction with the job's result.
async fn settle(
    mut tx: Tx,
    job: &ClaimedJob,
    guild: GuildId,
    first: &Hit,
    batch: &[Hit],
    settled: &Settled,
    next: JobResult,
) -> Result<Handled, DbError> {
    let ids: Vec<MessageId> = batch.iter().map(|h| h.message).collect();
    // A burst attach checks, under the same share lock case creation takes,
    // that the module is still on with the policy the hits came in under;
    // otherwise they fail and their messages are kept.
    let resolved;
    let settled = match settled {
        Settled::Burst(case) => {
            let s = honeypot::lock_settings(&mut tx, guild, false).await?;
            let holds = s.is_some_and(|s| {
                s.enabled
                    && s.version == first.settings_version
                    && s.channel == Some(first.channel)
                    && s.rule == first.rule
            });
            resolved = if holds {
                Settled::Case(*case)
            } else {
                refused(HoneypotRefusal::PolicyChanged)
            };
            &resolved
        }
        other => other,
    };
    let (changed, outcome) = match settled {
        Settled::Case(case) | Settled::Burst(case) => {
            let changed = honeypot::mark_case_created(&mut tx, guild, &ids, *case).await?;
            let cleanups: Vec<NewJob> = batch
                .iter()
                .map(|h| NewJob::honeypot_cleanup(guild, h.channel, h.message))
                .collect();
            queue::enqueue_many(&mut tx, &cleanups).await?;
            messages::bump(&mut tx, guild, Owner::Server(MessageKind::HoneypotWarning)).await?;
            (changed, HoneypotOutcome::Actioned)
        }
        Settled::Failed(failure) => {
            let changed = honeypot::mark_failed(&mut tx, guild, &ids, failure.as_str()).await?;
            match failure {
                HoneypotFailure::Refused(refusal) if changed > 0 => {
                    log_failed(&mut tx, guild, first, *refusal).await?;
                    (changed, HoneypotOutcome::Failed)
                }
                HoneypotFailure::Exempt => (changed, HoneypotOutcome::Exempt),
                _ => (changed, HoneypotOutcome::Failed),
            }
        }
    };
    let handled = finish(tx, job, next).await?;
    for _ in 0..changed {
        metrics().honeypot.hit(outcome);
    }
    tracing::info!(member_id = first.member.get(), hits = changed, outcome = ?outcome, "honeypot batch settled");
    Ok(handled)
}

async fn log_failed(
    tx: &mut Tx,
    guild: GuildId,
    first: &Hit,
    refusal: HoneypotRefusal,
) -> Result<(), DbError> {
    modlog::write(
        tx,
        guild,
        ModLogEntry::HoneypotCaseFailed(HoneypotCaseFailedDetail { refusal }),
        By::QUACK,
        ModLogLinks {
            member_id: Some(first.member),
            rule_id: first.rule,
            ..ModLogLinks::default()
        },
    )
    .await?;
    Ok(())
}

/// Finalization (budget spent, database only): the batch's hits fail
/// `discord_unavailable`, logged.
pub(crate) async fn finalize(engine: &Engine, job: &ClaimedJob) -> Result<Handled, DbError> {
    let (Some(guild), Ok(p)) = (job.guild, job.payload::<Payload>()) else {
        return Ok(dead("bad_payload"));
    };
    span("job honeypot_case finalize", guild, async {
        let (batch, more) = batch(engine, guild, p.member_id).await?;
        let Some(first) = batch.first() else {
            return Ok(Handled::Settle(JobResult::Done));
        };
        let tx = begin(engine, job).await?;
        let failed = refused(HoneypotRefusal::DiscordUnavailable);
        // Hits admitted under a later version get a fresh budget.
        let next = if more {
            JobResult::Reschedule { at: RunAt::Now }
        } else {
            JobResult::Done
        };
        settle(tx, job, guild, first, &batch, &failed, next).await
    })
    .await
}
