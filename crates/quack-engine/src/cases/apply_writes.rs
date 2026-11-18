//! The rows one rule application writes (§4.2 steps 17–21): the action,
//! the evidence, the receipt, and the `case_created` entry.

use quack_core::DomainError;
use quack_core::action::{ActionKind, timeout_ends_at};
use quack_core::case::{CaseDecision, ContextValues};
use quack_core::ids::{ActionId, CaseId, EvidenceId, MessageRowId};
use quack_core::limits::MAX_EVIDENCE_ITEMS;
use quack_core::modlog::{CaseCreatedDetail, ModLogEntry, ModLogLinks};
use quack_core::rule::{LevelAction, RuleDefinition};
use quack_db::evidence::{self, Admission};
use quack_db::messages::{self, NewTrackedMessage, Owner};
use quack_db::{DbError, Tx, actions};
use time::OffsetDateTime;

use super::apply::{Prepared, evidence_sources};
use super::apply_tx::moderator;
use super::{Actor, ApplyError, ApplyRule, Receipt};
use crate::modlog::{self, By};

/// Step 18: one `saving` evidence row per source.
pub(super) async fn insert_evidence(
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
pub(super) async fn log_created(
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

/// The level's action row; a timeout's end is fixed from `created_at`.
pub(super) async fn insert_action(
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
            let end = timeout_ends_at(created_at, duration_seconds)
                .ok_or_else(|| DbError::Invalid("timeout end out of range".to_owned()))?;
            (Some(end), None)
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

/// The receipt's tracked message, when the request has a destination.
pub(super) async fn insert_receipt(
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
