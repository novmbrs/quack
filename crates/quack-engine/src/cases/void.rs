//! Voiding a case (§4.7): keep it, stop counting it, cancel its pending
//! action, queue the undo of a timeout or ban that may have applied, drop
//! its unsent DM, and close its open appeal.

use quack_core::DomainError;
use quack_core::action::{AttemptOutcome, VoidFacts, on_void};
use quack_core::appeal::{AppealEventKind, AppealRow, CLOSED_BECAUSE_VOIDED, close_by_void};
use quack_core::case::VoidCause;
use quack_core::dm::{DmKind, DmState};
use quack_core::ids::{AccountId, ActionId, AppealEventId, AppealId, CaseId, DmId};
use quack_core::limits::VOID_REASON;
use quack_core::modlog::{
    AppealClosedDetail, CaseVoidedDetail, ModLogEntry, ModLogLinks, ModLogSource,
};
use quack_core::permissions::Capability;
use quack_db::appeals::{self, AppealEvent};
use quack_db::cases::{self, Case};
use quack_db::dms::{self, DmOwner};
use quack_db::jobs::{self, NewJob};
use quack_db::messages::{self, Owner};
use quack_db::{DbError, Tx, actions};
use quack_telemetry::metrics;
use quack_telemetry::spans::SpanEvent;
use serde::Serialize;
use time::OffsetDateTime;

use super::CaseError;
use crate::modlog::{self, By};
use crate::obs::{self, event, kv};
use crate::opkey::{Keyed, begin_keyed, complete, request_hash, stored_id};
use crate::{CaseRef, Engine, OpKey, Principal};

/// Who voids a case and why.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct VoidBy<'a> {
    /// The staff member (the voider, or who accepted the appeal).
    pub actor: AccountId,
    /// Where it came from.
    pub source: ModLogSource,
    /// `staff` or `appeal_accepted`.
    pub cause: VoidCause,
    /// The reason stored on the case.
    pub reason: &'a str,
    /// The appeal accepted, when the void is its acceptance.
    pub accepted_appeal: Option<AppealId>,
}

/// What a void changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VoidEffects {
    /// Pending originals now `cancelled`.
    pub cancelled: Vec<ActionId>,
    /// Undo actions queued.
    pub undos: Vec<ActionId>,
    /// The case DM, now `not_needed`.
    pub dm_not_needed: Option<DmId>,
    /// The open appeal Quack closed.
    pub closed_appeal: Option<AppealId>,
    /// The kind of the cancelled original.
    pub cancelled_kind: Option<quack_core::action::ActionKind>,
    /// The original action's kind and its state when the void found it.
    pub original: Option<(
        quack_core::action::ActionKind,
        quack_core::action::ActionState,
    )>,
    /// The kinds of the undo actions queued.
    pub undo_kinds: Vec<quack_core::action::ActionKind>,
}

/// The result of [`void_case`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Voided {
    /// The case.
    pub case: CaseId,
    /// Its number.
    pub number: u64,
    /// Its member.
    pub member: quack_core::ids::MemberId,
    /// The name of the rule version it used (`word::IMPORTED` for an
    /// imported case without one).
    pub rule_name: String,
    /// The same request already voided it.
    pub replayed: bool,
    /// What this void changed; `None` for a replay (read the case then).
    pub effects: Option<VoidEffects>,
}

/// The name of the rule version a case used.
async fn rule_name(conn: &mut quack_db::PgConnection, case: &Case) -> Result<String, DbError> {
    Ok(match case.rule {
        Some((rule, version)) => quack_db::rules::version(conn, case.guild, rule, version)
            .await?
            .map_or_else(
                || quack_text::word::IMPORTED.to_owned(),
                |v| v.definition.name,
            ),
        None => quack_text::word::IMPORTED.to_owned(),
    })
}

#[derive(Serialize)]
struct VoidRequest<'a> {
    case: CaseId,
    reason: &'a str,
}

/// Voids a valid case (§4.7). A second void is `case_already_voided`; a
/// replay of the same operation key returns the original result.
pub async fn void_case(
    engine: &Engine,
    principal: &Principal,
    case: CaseRef,
    reason: &str,
    op_key: &OpKey,
) -> Result<Voided, CaseError> {
    let guild = principal.guild;
    obs::use_case("void_case", Some(guild), async {
        let mut conn = engine.db().conn().await?;
        principal.require(&mut conn, Capability::Moderate).await??;
        let found = case
            .find(&mut conn, guild)
            .await?
            .ok_or(DomainError::CaseNotFound)?;
        drop(conn);
        if !VOID_REASON.allows(reason) {
            return Err(CaseError::ReasonLength);
        }
        let reason = reason.trim();
        let key = op_key.scoped(Some(guild), Some(principal.account));
        let hash = request_hash(&VoidRequest {
            case: found.id,
            reason,
        });
        let mut tx = match begin_keyed(engine.db(), &key, &hash).await? {
            Keyed::Replay(response) => {
                let id: CaseId = stored_id(&response, "case_id")?;
                let mut conn = engine.db().conn().await?;
                return Ok(Voided {
                    case: id,
                    number: found.number,
                    member: found.member,
                    rule_name: rule_name(&mut conn, &found).await?,
                    replayed: true,
                    effects: None,
                });
            }
            Keyed::Fresh(tx) => tx,
        };
        principal.require(&mut tx, Capability::Moderate).await??;
        let locked = cases::lock(&mut tx, guild, found.id)
            .await?
            .ok_or(DomainError::CaseNotFound)?;
        if !locked.is_valid() {
            return Err(DomainError::CaseAlreadyVoided.into());
        }
        let by = VoidBy {
            actor: principal.account,
            source: principal.origin.mod_log_source(),
            cause: VoidCause::Staff,
            reason,
            accepted_appeal: None,
        };
        let effects = void_in_tx(&mut tx, &locked, by).await?;
        let rule_name = rule_name(&mut tx, &locked).await?;
        complete(
            &mut tx,
            &key,
            200,
            serde_json::json!({ "case_id": locked.id }),
        )
        .await?;
        tx.commit().await?;
        voided_metrics(&locked, VoidCause::Staff, &effects);
        Ok(Voided {
            case: locked.id,
            number: locked.number,
            member: locked.member,
            rule_name,
            replayed: false,
            effects: Some(effects),
        })
    })
    .await
}

/// Counts a committed void (and what it changed) and adds its span event.
pub(crate) fn voided_metrics(voided: &Case, cause: VoidCause, effects: &VoidEffects) {
    metrics().cases.voided(obs::void_cause(cause));
    if let Some(kind) = effects.cancelled_kind {
        metrics().actions.finished(
            obs::action_kind(kind),
            quack_telemetry::labels::ActionState::Cancelled,
        );
    }
    if let Some(dm) = effects.dm_not_needed {
        obs::dm_settled(dm, DmKind::Case, DmState::NotNeeded, None);
    }
    if effects.closed_appeal.is_some() {
        metrics()
            .appeals
            .transition(obs::appeal_transition(AppealEventKind::Closed));
    }
    event(
        SpanEvent::CaseVoided,
        vec![kv("case.id", voided.id.get()), kv("cause", cause.as_str())],
    );
}

/// The void's writes (§4.7 step 2.3–2.9), inside the caller's
/// transaction, with `case` locked and valid.
pub(crate) async fn void_in_tx(
    tx: &mut Tx,
    case: &Case,
    by: VoidBy<'_>,
) -> Result<VoidEffects, DbError> {
    let guild = case.guild;
    if !cases::void(tx, guild, case.id, by.cause, Some(by.actor), by.reason).await? {
        return Err(DbError::Stale);
    }
    let mut effects = VoidEffects::default();
    void_actions(tx, case, by.actor, &mut effects).await?;
    effects.dm_not_needed = dms::cancel_pending_case_dm(tx, guild, case.id).await?;
    if by.accepted_appeal.is_none() {
        effects.closed_appeal = close_open_appeal(tx, case, by.source).await?;
    }
    messages::bump(tx, guild, Owner::Case(case.id)).await?;
    let number = case.number;
    modlog::write(
        tx,
        guild,
        ModLogEntry::CaseVoided(CaseVoidedDetail {
            case_number: number,
            cause: by.cause,
            cancelled_action_ids: effects.cancelled.clone(),
            undo_action_ids: effects.undos.clone(),
        }),
        By {
            actor: Some(by.actor),
            source: by.source,
        },
        ModLogLinks {
            case_id: Some(case.id),
            member_id: Some(case.member),
            appeal_id: by.accepted_appeal,
            ..ModLogLinks::default()
        },
    )
    .await?;
    Ok(effects)
}

/// Step 2.4–2.5: cancel a pending original; queue the undo of a timeout
/// or ban that may have applied.
async fn void_actions(
    tx: &mut Tx,
    case: &Case,
    voider: AccountId,
    effects: &mut VoidEffects,
) -> Result<(), DbError> {
    let guild = case.guild;
    let Some(original) = actions::lock_original(tx, guild, case.id).await? else {
        return Ok(());
    };
    let has_undo = actions::undo_of(tx, guild, original.id).await?.is_some();
    effects.original = Some((original.kind, original.state));
    // `on_void` queues a done ban's unban even when it was `no_change`, so
    // the case that now owns what Discord shows removes it (§5.6).
    let attempts: Vec<Option<AttemptOutcome>> = actions::attempts(tx, original.id)
        .await?
        .into_iter()
        .map(|a| a.outcome)
        .collect();
    let effect = on_void(&VoidFacts {
        kind: original.kind,
        state: original.state,
        has_undo,
        attempts: &attempts,
        timeout_ends_at: original.timeout_ends_at,
        now: OffsetDateTime::now_utc(),
    });
    if effect.cancel
        && let Some(id) = actions::cancel_pending_original(tx, guild, case.id).await?
    {
        effects.cancelled.push(id);
        effects.cancelled_kind = Some(original.kind);
    }
    if let Some(kind) = effect.queue_undo {
        let undo = ActionId::from_uuid(uuid::Uuid::now_v7());
        actions::insert(
            tx,
            &actions::NewAction {
                id: undo,
                guild,
                case: case.id,
                member: case.member,
                kind,
                undo_of: Some(original.id),
                timeout_ends_at: None,
                delete_message_seconds: None,
                requested_by: Some(voider),
                created_at: None,
            },
        )
        .await?;
        jobs::enqueue(tx, &NewJob::run_action(guild, undo)).await?;
        effects.undos.push(undo);
        effects.undo_kinds.push(kind);
    }
    Ok(())
}

/// Step 2.7: Quack closes the case's open appeal, with its DM.
async fn close_open_appeal(
    tx: &mut Tx,
    case: &Case,
    source: ModLogSource,
) -> Result<Option<AppealId>, DbError> {
    let guild = case.guild;
    let Some(appeal) = appeals::lock_for_case(tx, guild, case.id).await? else {
        return Ok(None);
    };
    let row = AppealRow {
        state: appeal.state,
        version: appeal.version,
        closed_by_void: false,
    };
    let Some(change) = close_by_void(&row)
        .map_err(|_| DbError::Invalid("the appeal's version can't advance".to_owned()))?
    else {
        return Ok(None);
    };
    appeals::transition(tx, guild, appeal.id, appeal.state, None, change.to, None)
        .await?
        .ok_or(DbError::Stale)?;
    let event_id = AppealEventId::from_uuid(uuid::Uuid::now_v7());
    appeals::insert_event(
        tx,
        guild,
        &AppealEvent {
            id: event_id,
            appeal: appeal.id,
            kind: AppealEventKind::Closed,
            actor: None,
            text: CLOSED_BECAUSE_VOIDED.to_owned(),
            closed_by_void: true,
            created_at: OffsetDateTime::now_utc(),
        },
    )
    .await?;
    let dm = DmId::from_uuid(uuid::Uuid::now_v7());
    dms::insert(
        tx,
        dm,
        guild,
        case.member,
        DmOwner::AppealUpdate(case.id, event_id),
    )
    .await?;
    jobs::enqueue(tx, &NewJob::send_dm(guild, dm)).await?;
    messages::bump(tx, guild, Owner::Appeal(appeal.id)).await?;
    modlog::write(
        tx,
        guild,
        ModLogEntry::AppealClosed(AppealClosedDetail {
            case_number: case.number,
            because_case_voided: true,
        }),
        By {
            actor: None,
            source,
        },
        ModLogLinks {
            case_id: Some(case.id),
            member_id: Some(case.member),
            appeal_id: Some(appeal.id),
            ..ModLogLinks::default()
        },
    )
    .await?;
    Ok(Some(appeal.id))
}
