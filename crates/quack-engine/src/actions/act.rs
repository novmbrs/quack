//! Read and act (§5.4 step 2): gather the facts the start pre-check needs,
//! then read Discord's current state and make the change only when it is
//! still needed (§5.1, §5.6). No locks are held here.

use quack_core::action::{
    ActionKind, ActionTarget, AttemptResult, Plan, after_call, after_read_failure,
    plan_after_marker, plan_from_ban, plan_from_member,
};
use quack_core::ids::GuildId;
use quack_core::permissions::{ActorPresence, ApplyFacts, MemberSubject};
use quack_db::actions::{self, Action};
use quack_db::cases::Case;
use quack_db::{DbError, guilds};
use time::OffsetDateTime;

use super::audit_reason;
use crate::Engine;
use crate::port::{Fresh, Membership, Outcome, RefusalKind};

/// What the start pre-check needs from Discord.
pub(super) struct ActFacts {
    /// Owner, Quack, and the member as the pre-check sees them.
    pub apply: ApplyFacts,
}

/// Gathers the server's facts and the member's cached roles (§5.4). A
/// server Quack left, or Discord's Unknown Guild, means Quack is not in the
/// server; anything else unanswered retries the job (`Err` is the code).
pub(super) async fn gather(
    engine: &Engine,
    guild: GuildId,
    case: &Case,
) -> Result<ActFacts, String> {
    let installed = {
        let mut conn = engine.db().conn().await.map_err(|e| db_code(&e))?;
        guilds::get(&mut conn, guild)
            .await
            .map_err(|e| db_code(&e))?
            .is_some_and(|g| g.installed)
    };
    let absent = MemberSubject {
        id: case.member,
        bot: false,
        present: None,
    };
    let not_in_server = ActFacts {
        apply: ApplyFacts {
            owner_id: case.member.account(),
            quack: None,
            actor: ActorPresence::Quack,
            member: absent,
        },
    };
    if !installed {
        return Ok(not_in_server);
    }
    let facts = match engine.discord().guild_facts(guild).await {
        Outcome::Done(facts) => facts,
        Outcome::Refused(r) if r.kind == RefusalKind::UnknownGuild => return Ok(not_in_server),
        other => return Err(other.code()),
    };
    let member = match engine
        .discord()
        .member(guild, case.member, Fresh::Cached)
        .await
    {
        Outcome::Done(Membership::Present(m)) => m.subject(),
        Outcome::Done(Membership::Absent) => absent,
        other => return Err(other.code()),
    };
    Ok(ActFacts {
        apply: ApplyFacts {
            owner_id: facts.owner_id,
            quack: facts
                .quack
                .as_ref()
                .map(quack_core::permissions::MemberFacts::present),
            actor: ActorPresence::Quack,
            member,
        },
    })
}

fn db_code(error: &DbError) -> String {
    format!(
        "db_{}",
        quack_telemetry::labels::Attribute::as_str(error.class())
    )
}

/// What the attempt is about: a remove timeout uses its original's end.
pub(super) async fn target(
    engine: &Engine,
    action: &Action,
    case: &Case,
) -> Result<ActionTarget, DbError> {
    let timeout_ends_at = match (action.kind, action.undo_of) {
        (ActionKind::RemoveTimeout, Some(original)) => {
            let mut conn = engine.db().conn().await?;
            actions::get(&mut conn, case.guild, original)
                .await?
                .and_then(|o| o.timeout_ends_at)
        }
        _ => action.timeout_ends_at,
    };
    Ok(ActionTarget {
        kind: action.kind,
        case_number: case.number,
        timeout_ends_at,
        member_joined_at: case.member_joined_at,
        case_created_at: case.created_at,
    })
}

/// Reads what the attempt is about to change, and changes it only when
/// that is still needed (§5.1, §5.6).
pub(super) async fn act(
    engine: &Engine,
    action: &Action,
    case: &Case,
    target: &ActionTarget,
    rule_name: &str,
) -> AttemptResult {
    let discord = engine.discord();
    let (guild, member) = (case.guild, case.member);
    let plan = if action.kind.reads_member() {
        match discord.member(guild, member, Fresh::Rest).await {
            Outcome::Done(m) => {
                let m = m.present().cloned().map(at_db_precision);
                plan_from_member(target, m.as_ref(), OffsetDateTime::now_utc())
            }
            other => return after_read_failure(other.call()),
        }
    } else {
        match discord.ban(guild, member).await {
            Outcome::Done(ban) => plan_from_ban(target, ban.as_ref()),
            other => return after_read_failure(other.call()),
        }
    };
    let plan = match plan {
        Ok(Plan::CheckMarker(number)) => match marker_case(engine, case, number).await {
            Ok(voided) => plan_after_marker(voided),
            Err(_) => return AttemptResult::NotSent { retry_after: None },
        },
        Ok(plan) => plan,
        Err(_) => return AttemptResult::NotSent { retry_after: None },
    };
    match plan {
        Plan::Finish(result) => result,
        Plan::CheckMarker(_) => AttemptResult::NotSent { retry_after: None },
        Plan::Call => {
            let reason = audit_reason(action.kind, case.number, rule_name);
            let outcome = change(engine, action, target, &reason).await;
            after_call(target, outcome.call(), OffsetDateTime::now_utc())
        }
    }
}

/// The member as the database would store them: `joined_at` at
/// microsecond precision, so the membership the case recorded compares
/// equal to the same membership read again (§5.1 kick).
fn at_db_precision(
    mut m: quack_core::permissions::MemberFacts,
) -> quack_core::permissions::MemberFacts {
    m.joined_at = m.joined_at.map(|at| {
        let micros = at.nanosecond() / 1_000 * 1_000;
        at.replace_nanosecond(micros).unwrap_or(at)
    });
    m
}

async fn marker_case(engine: &Engine, case: &Case, number: u64) -> Result<bool, DbError> {
    let mut conn = engine.db().conn().await?;
    actions::voided_ban_case(&mut conn, case.guild, case.member, number).await
}

/// The change call itself.
async fn change(
    engine: &Engine,
    action: &Action,
    target: &ActionTarget,
    reason: &str,
) -> Outcome<()> {
    let discord = engine.discord();
    let (guild, member) = (action.guild, action.member);
    match action.kind {
        ActionKind::Timeout => match target.timeout_ends_at {
            Some(until) => discord.timeout(guild, member, until, reason).await,
            None => Outcome::Refused(crate::port::Refusal::of(RefusalKind::Other)),
        },
        ActionKind::Kick => discord.kick(guild, member, reason).await,
        ActionKind::Ban => {
            let delete = action.delete_message_seconds.unwrap_or(0);
            discord.ban_member(guild, member, delete, reason).await
        }
        ActionKind::RemoveTimeout => discord.remove_timeout(guild, member, reason).await,
        ActionKind::Unban => discord.unban(guild, member, reason).await,
    }
}
