//! Applying a rule (§4.2): steps 7–22, one transaction.

use std::time::Duration;

use quack_core::DomainError;
use quack_core::action::{ActionKind, timeout_ends_at};
use quack_core::case::{
    CaseDecision, ContextValues, check_replacement, fill_message_links, validate_context,
};
use quack_core::escalation::counting_cutoff;
use quack_core::ids::{ActionId, CaseId, DmId, EvidenceId, MessageRowId};
use quack_core::limits::MAX_EVIDENCE_ITEMS;
use quack_core::modlog::{CaseCreatedDetail, ModLogEntry, ModLogLinks};
use quack_core::permissions::check_action_permission;
use quack_core::rule::{LevelAction, RuleDefinition};
use quack_db::cases::{self, NewCase};
use quack_db::dms::{self, DmOwner};
use quack_db::evidence::{self, Admission};
use quack_db::jobs::{self, NewJob, RunAt};
use quack_db::messages::{self, NewTrackedMessage, Owner};
use quack_db::{Conflict, DbError, Tx, actions, drafts, guilds, honeypot, rules};
use quack_telemetry::spans::SpanEvent;
use time::OffsetDateTime;

use super::apply::{Prepared, evidence_sources, log_refusal};
use super::{Actor, Applied, ApplyError, ApplyRule, CaseCreated, Receipt};
use crate::modlog::{self, By};
use crate::obs::{event, kv};
use crate::{Engine, opkey};

/// Statement and lock timeout inside case creation (§4.2 step 22).
const STATEMENT_TIMEOUT: Duration = Duration::from_secs(2);
/// The whole transaction's deadline (§4.2 step 22).
const DEADLINE: Duration = Duration::from_secs(3);
/// How long a kick or ban waits for its DM at most (§6.3).
const DM_WAIT: time::Duration = quack_core::dm::ACTION_WAIT;

/// Runs steps 8–22 in `tx` (which holds the operation key), commits, and
/// reports the new case.
pub(super) async fn run(
    engine: &Engine,
    request: &ApplyRule,
    prepared: &Prepared,
    mut tx: Tx,
) -> Result<Applied, ApplyError> {
    let started = std::time::Instant::now();
    let body = tokio::time::timeout(DEADLINE, body(request, prepared, &mut tx)).await;
    let created = match body {
        Ok(Ok(created)) => created,
        Ok(Err(error)) => {
            let _ = tx.rollback().await;
            if let ApplyError::Refused(DomainError::MemberRefused(refusal)) = &error {
                log_refusal(engine, request, prepared.source, *refusal).await?;
            }
            return Err(error);
        }
        Err(_) => {
            // The deadline passed before COMMIT was sent: dropping the
            // transaction rolls it back.
            return Err(ApplyError::Deadline);
        }
    };
    let left = DEADLINE
        .saturating_sub(started.elapsed())
        .max(Duration::from_millis(500));
    match tokio::time::timeout(left, tx.commit()).await {
        Ok(Ok(())) => {}
        Ok(Err(DbError::Unavailable(..))) | Err(_) => return Err(ApplyError::Unconfirmed),
        Ok(Err(error)) => return Err(error.into()),
    }
    let d = created.case.decision;
    event(
        SpanEvent::CaseCreated,
        vec![
            kv("case.id", created.case.id.get()),
            kv("case.number", created.case.number),
            kv("level_index", d.map_or(0, |d| d.level_index)),
            kv(
                "action_kind",
                d.and_then(|d| d.action_kind())
                    .map_or("none", ActionKind::as_str),
            ),
        ],
    );
    Ok(Applied::Created(Box::new(created)))
}

async fn body(
    request: &ApplyRule,
    prepared: &Prepared,
    tx: &mut Tx,
) -> Result<CaseCreated, ApplyError> {
    let guild = request.guild;
    tx.set_timeouts(STATEMENT_TIMEOUT, STATEMENT_TIMEOUT)
        .await?;
    still_allowed(tx, request).await?;
    // 9. The case number; its row lock serializes creation per server.
    let number = guilds::next_case_number(tx, guild).await?;
    // 10–13. The rule under lock, the count, the level, checks 9 and 10.
    let created_at = now_micros();
    let (def, values, decision) = decide(tx, request, prepared, created_at).await?;
    // 14. The replaced case.
    let replaces_number = replaced(tx, request).await?;
    // 15. The case.
    let case_id = CaseId::from_uuid(uuid::Uuid::now_v7());
    let scoped = prepared.key.case_scoped();
    cases::insert(
        tx,
        &NewCase {
            id: case_id,
            guild,
            number,
            member: request.member,
            member_joined_at: prepared.facts.joined_at,
            source: prepared.source,
            moderator: moderator(request),
            decision: &decision,
            context: &values,
            operation_key: Some(&scoped),
            request_hash: Some(&prepared.hash),
            replaces: request.replaces,
            created_at,
        },
    )
    .await
    .map_err(|e| match e.conflict() {
        Some(Conflict::CaseAlreadyReplaced) => ApplyError::Refused(DomainError::ReplacementInvalid),
        _ => e.into(),
    })?;
    // 16–19. Action, DM, evidence, receipt.
    let action = insert_action(tx, request, case_id, &decision, created_at).await?;
    let dm = if decision.sends_dm {
        let dm = DmId::from_uuid(uuid::Uuid::now_v7());
        dms::insert(tx, dm, guild, request.member, DmOwner::Case(case_id)).await?;
        Some(dm)
    } else {
        None
    };
    let evidence = insert_evidence(tx, request, case_id, &values, &def).await?;
    let receipt = insert_receipt(tx, request, case_id).await?;
    // 20. The moderation log.
    log_created(
        tx,
        request,
        prepared,
        case_id,
        number,
        &decision,
        replaces_number,
    )
    .await?;
    // 21. Jobs, and what else the case changes.
    if let Some(dm) = dm {
        jobs::enqueue(tx, &NewJob::send_dm(guild, dm)).await?;
    }
    if let Some((action, kind)) = action {
        let waits = decision.sends_dm && matches!(kind, ActionKind::Kick | ActionKind::Ban);
        let at = if waits {
            RunAt::At(created_at + DM_WAIT)
        } else {
            RunAt::Now
        };
        jobs::enqueue(tx, &NewJob::run_action(guild, action).at(at)).await?;
    }
    for id in &evidence {
        jobs::enqueue(tx, &NewJob::capture_evidence(guild, *id)).await?;
    }
    if let Some(replaced) = request.replaces {
        messages::bump(tx, guild, Owner::Case(replaced)).await?;
    }
    if let Some(draft) = request.draft {
        drafts::delete(tx, draft).await?;
    }
    opkey::complete(
        tx,
        &prepared.key,
        201,
        serde_json::json!({ "case_id": case_id }),
    )
    .await?;
    let case = cases::get(tx, guild, case_id)
        .await?
        .ok_or(DbError::NotFound)?;
    Ok(CaseCreated {
        case,
        action: action.map(|(id, _)| id),
        dm,
        evidence,
        receipt,
    })
}

fn moderator(request: &ApplyRule) -> Option<quack_core::ids::AccountId> {
    match &request.actor {
        Actor::Staff(p) => Some(p.account),
        Actor::Quack => None,
    }
}

/// The capability (re-checked inside the transaction, §2) and, for
/// honeypot cases, the configuration the hits were admitted under (step 8).
async fn still_allowed(tx: &mut Tx, request: &ApplyRule) -> Result<(), ApplyError> {
    if let Actor::Staff(principal) = &request.actor {
        principal
            .require(tx, quack_core::permissions::Capability::Moderate)
            .await??;
    }
    if let Some(admission) = request.honeypot {
        let settings = honeypot::lock_settings(tx, request.guild, false).await?;
        let holds = settings.is_some_and(|s| {
            s.enabled
                && s.version == admission.settings_version
                && s.channel == Some(admission.channel)
                && s.rule == Some(request.rule)
        });
        if !holds {
            return Err(ApplyError::HoneypotPolicyChanged);
        }
    }
    Ok(())
}

/// Steps 10–13: the rule under lock, the context against that version,
/// the count, the level, and member checks 9 and 10.
async fn decide(
    tx: &mut Tx,
    request: &ApplyRule,
    prepared: &Prepared,
    created_at: OffsetDateTime,
) -> Result<(RuleDefinition, ContextValues, CaseDecision), ApplyError> {
    let (version, def) = locked_rule(tx, request).await?;
    let values = context_for(request, prepared, version, &def)?;
    let since = counting_cutoff(created_at, def.counting_window_days);
    let counted =
        cases::count_counted(tx, request.guild, request.member, request.rule, since).await?;
    let decision = CaseDecision::decide(request.rule, version, &def, counted)
        .ok_or_else(|| ApplyError::Db(DbError::Decode("rule without levels".to_owned())))?;
    if let Some(kind) = decision.action_kind() {
        check_action_permission(&prepared.facts.apply, kind).map_err(DomainError::MemberRefused)?;
    }
    Ok((def, values, decision))
}

/// Step 18: one `saving` evidence row per source.
async fn insert_evidence(
    tx: &mut Tx,
    request: &ApplyRule,
    case: CaseId,
    values: &ContextValues,
    def: &RuleDefinition,
) -> Result<Vec<EvidenceId>, ApplyError> {
    let sources = evidence_sources(request, values, Some(def))?;
    if sources.len() > MAX_EVIDENCE_ITEMS as usize {
        return Err(DomainError::EvidenceLimitReached { count: 0 }.into());
    }
    let mut ids = Vec::new();
    for link in &sources {
        let id = EvidenceId::from_uuid(uuid::Uuid::now_v7());
        let admitted = evidence::admit(
            tx,
            request.guild,
            case,
            id,
            link.channel_id,
            link.message_id,
            moderator(request),
        )
        .await?;
        if let Admission::Added(id) | Admission::Recapture(id) = admitted {
            ids.push(id);
        }
    }
    Ok(ids)
}

/// Step 20: `case_created`.
async fn log_created(
    tx: &mut Tx,
    request: &ApplyRule,
    prepared: &Prepared,
    case: CaseId,
    number: u64,
    decision: &CaseDecision,
    replaces_number: Option<u64>,
) -> Result<(), ApplyError> {
    let by = match &request.actor {
        Actor::Staff(p) => By {
            actor: Some(p.account),
            source: p.origin.mod_log_source(),
        },
        Actor::Quack => By::QUACK,
    };
    modlog::write(
        tx,
        request.guild,
        ModLogEntry::CaseCreated(CaseCreatedDetail {
            source: prepared.source,
            case_number: number,
            level_index: decision.level_index,
            level_from_case_count: decision.level_from_case_count,
            case_count: decision.case_count,
            action_kind: decision.action_kind(),
            timeout_seconds: decision.timeout_seconds(),
            ban_delete_message_seconds: decision.ban_delete_message_seconds(),
            sends_dm: decision.sends_dm,
            replaces_case_number: replaces_number,
        }),
        by,
        ModLogLinks {
            case_id: Some(case),
            member_id: Some(request.member),
            rule_id: Some(request.rule),
            ..ModLogLinks::default()
        },
    )
    .await?;
    Ok(())
}

/// `created_at`, captured once, at the database's microsecond precision.
fn now_micros() -> OffsetDateTime {
    let now = OffsetDateTime::now_utc();
    let micros = now.nanosecond() / 1_000 * 1_000;
    now.replace_nanosecond(micros).unwrap_or(now)
}

async fn locked_rule(
    tx: &mut Tx,
    request: &ApplyRule,
) -> Result<(u32, RuleDefinition), ApplyError> {
    let lock = rules::lock_for_share(tx, request.guild, request.rule)
        .await?
        .ok_or(DomainError::RuleNotFound)?;
    if lock.archived {
        return Err(DomainError::RuleArchived.into());
    }
    if request
        .pinned_version
        .is_some_and(|v| v != lock.current_version)
    {
        return Err(DomainError::RuleChanged.into());
    }
    let version = rules::version(tx, request.guild, request.rule, lock.current_version)
        .await?
        .ok_or(DbError::NotFound)?;
    Ok((lock.current_version, version.definition))
}

/// The context validated against the version under lock (skipped when the
/// early check already used that version).
fn context_for(
    request: &ApplyRule,
    prepared: &Prepared,
    version: u32,
    def: &RuleDefinition,
) -> Result<ContextValues, ApplyError> {
    if let Some((early, values)) = &prepared.early_version
        && *early == version
    {
        return Ok(values.clone());
    }
    let fields = def.context_fields();
    let mut context = request.context.clone();
    if let Some(link) = &request.source_message {
        fill_message_links(&fields, &mut context, link);
    }
    validate_context(&fields, request.guild, &context).map_err(ApplyError::ContextInvalid)
}

async fn replaced(tx: &mut Tx, request: &ApplyRule) -> Result<Option<u64>, ApplyError> {
    let Some(id) = request.replaces else {
        return Ok(None);
    };
    let case = cases::get(tx, request.guild, id)
        .await?
        .ok_or(DomainError::ReplacementInvalid)?;
    let already = cases::replaced_by(tx, request.guild, id).await?.is_some();
    check_replacement(!case.is_valid(), already)?;
    Ok(Some(case.number))
}

async fn insert_action(
    tx: &mut Tx,
    request: &ApplyRule,
    case: CaseId,
    decision: &CaseDecision,
    created_at: OffsetDateTime,
) -> Result<Option<(ActionId, ActionKind)>, ApplyError> {
    let Some(level_action) = decision.action else {
        return Ok(None);
    };
    let id = ActionId::from_uuid(uuid::Uuid::now_v7());
    let kind = level_action.kind();
    let (ends, delete) = match level_action {
        LevelAction::Timeout { duration_seconds } => {
            (Some(timeout_ends_at(created_at, duration_seconds)), None)
        }
        LevelAction::Kick => (None, None),
        LevelAction::Ban {
            delete_message_seconds,
        } => (None, Some(delete_message_seconds)),
    };
    actions::insert(
        tx,
        &actions::NewAction {
            id,
            guild: request.guild,
            case,
            member: request.member,
            kind,
            undo_of: None,
            timeout_ends_at: ends,
            delete_message_seconds: delete,
            requested_by: None,
            created_at: Some(created_at),
        },
    )
    .await?;
    Ok(Some((id, kind)))
}

async fn insert_receipt(
    tx: &mut Tx,
    request: &ApplyRule,
    case: CaseId,
) -> Result<Option<MessageRowId>, ApplyError> {
    let Some(receipt) = &request.receipt else {
        return Ok(None);
    };
    let (channel, private, token) = match receipt {
        Receipt::Private {
            channel,
            token,
            expires_at,
        } => (
            *channel,
            true,
            Some((token.expose().to_owned(), *expires_at)),
        ),
        Receipt::Public { channel, token } => (
            *channel,
            false,
            token.as_ref().map(|(t, at)| (t.expose().to_owned(), *at)),
        ),
    };
    let id = MessageRowId::from_uuid(uuid::Uuid::now_v7());
    messages::insert(
        tx,
        &NewTrackedMessage {
            id,
            guild: request.guild,
            owner: Owner::Case(case),
            channel: Some(channel),
            private,
            interaction_token: token,
        },
    )
    .await?;
    Ok(Some(id))
}
