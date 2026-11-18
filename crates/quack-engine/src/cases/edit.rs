//! Editing case context (§4.9): validated against the case's own rule
//! version, compared on the revision the editor opened, and new message
//! links become evidence in the same transaction.

use quack_core::DomainError;
use quack_core::case::{ContextInput, MessageLink, validate_context};
use quack_core::evidence::check_room;
use quack_core::ids::{CaseId, EvidenceId};
use quack_core::modlog::{CaseContextEditedDetail, ModLogEntry, ModLogLinks};
use quack_core::permissions::Capability;
use quack_core::rule::RuleDefinition;
use quack_db::evidence::{self, Admission};
use quack_db::jobs::{self, NewJob};
use quack_db::messages::{self, Owner};
use quack_db::{Tx, cases, rules};
use serde::Serialize;

use super::{CaseError, CaseRef};
use crate::modlog::{self, By};
use crate::opkey::{Keyed, begin_keyed, complete, request_hash, stored_id};
use crate::{Engine, OpKey, Principal, obs};

/// What [`edit_context`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContextEdited {
    /// The case.
    pub case: CaseId,
    /// Its number.
    pub number: u64,
    /// The new revision (on a replay, the current one).
    pub revision: u32,
    /// Message links that became new evidence.
    pub new_evidence: u32,
    /// The same request already ran.
    pub replayed: bool,
}

#[derive(Serialize)]
struct EditRequest<'a> {
    case: CaseId,
    context: Vec<(&'a str, String)>,
    expected_revision: u32,
}

/// Replaces a case's context (valid or voided case), if it is still at
/// `expected_revision`; otherwise `context_changed` and nothing is saved.
pub async fn edit_context(
    engine: &Engine,
    principal: &Principal,
    case: CaseRef,
    context: &[(String, ContextInput)],
    expected_revision: u32,
    op_key: &OpKey,
) -> Result<ContextEdited, CaseError> {
    let guild = principal.guild;
    obs::use_case("edit_context", Some(guild), async {
        let (found, def) = {
            let mut conn = engine.db().conn().await?;
            principal.require(&mut conn, Capability::Moderate).await??;
            let found = case
                .find(&mut conn, guild)
                .await?
                .ok_or(DomainError::CaseNotFound)?;
            let def = match found.rule {
                Some((rule, version)) => rules::version(&mut conn, guild, rule, version)
                    .await?
                    .map(|v| v.definition),
                None => None,
            };
            (found, def.unwrap_or_else(no_fields))
        };
        let fields = def.context_fields();
        let values =
            validate_context(&fields, guild, context).map_err(CaseError::ContextInvalid)?;
        let key = op_key.scoped(Some(guild), Some(principal.account));
        let hash = request_hash(&EditRequest {
            case: found.id,
            context: super::apply::context_for_hash(context),
            expected_revision,
        });
        let mut tx = match begin_keyed(engine.db(), &key, &hash).await? {
            Keyed::Replay(response) => {
                return Ok(ContextEdited {
                    case: stored_id(&response, "case_id")?,
                    number: found.number,
                    revision: found.context_revision,
                    new_evidence: 0,
                    replayed: true,
                });
            }
            Keyed::Fresh(tx) => tx,
        };
        principal.require(&mut tx, Capability::Moderate).await??;
        let locked = cases::lock(&mut tx, guild, found.id)
            .await?
            .ok_or(DomainError::CaseNotFound)?;
        if locked.context_revision != expected_revision {
            return Err(CaseError::ContextChanged);
        }
        let revision = cases::set_context(&mut tx, guild, locked.id, expected_revision, &values)
            .await?
            .ok_or(CaseError::ContextChanged)?;
        let links = values.evidence_links(&fields, guild);
        let new_evidence = admit_links(&mut tx, principal, &locked, &links).await?;
        let changed = locked.context.changed_keys(&values);
        modlog::write(
            &mut tx,
            guild,
            ModLogEntry::CaseContextEdited(CaseContextEditedDetail {
                case_number: locked.number,
                changed_keys: changed,
            }),
            By {
                actor: Some(principal.account),
                source: principal.origin.mod_log_source(),
            },
            ModLogLinks {
                case_id: Some(locked.id),
                member_id: Some(locked.member),
                ..ModLogLinks::default()
            },
        )
        .await?;
        messages::bump(&mut tx, guild, Owner::Case(locked.id)).await?;
        complete(
            &mut tx,
            &key,
            200,
            serde_json::json!({ "case_id": locked.id }),
        )
        .await?;
        tx.commit().await?;
        Ok(ContextEdited {
            case: locked.id,
            number: locked.number,
            revision,
            new_evidence,
            replayed: false,
        })
    })
    .await
}

/// A rule version with no fields: imported cases are edited through the
/// free-text `details` field.
fn no_fields() -> RuleDefinition {
    RuleDefinition {
        name: String::new(),
        description: String::new(),
        reason: String::new(),
        appealable: false,
        counting_window_days: None,
        fields: Vec::new(),
        levels: Vec::new(),
    }
}

/// New links that are not yet evidence get `saving` rows and capture jobs,
/// under the per-case cap (counted under the case lock).
async fn admit_links(
    tx: &mut Tx,
    principal: &Principal,
    case: &quack_db::cases::Case,
    links: &[MessageLink],
) -> Result<u32, CaseError> {
    if links.is_empty() {
        return Ok(0);
    }
    let existing = evidence::for_case(tx, case.guild, case.id).await?;
    let fresh: Vec<&MessageLink> = links
        .iter()
        .filter(|l| !existing.iter().any(|e| e.message == l.message_id))
        .collect();
    let count = u32::try_from(existing.len()).unwrap_or(u32::MAX);
    check_room(count, fresh.len(), 0)?;
    let mut added = 0;
    for link in fresh {
        let id = EvidenceId::from_uuid(uuid::Uuid::now_v7());
        let admitted = evidence::admit(
            tx,
            case.guild,
            case.id,
            id,
            link.channel_id,
            link.message_id,
            Some(principal.account),
        )
        .await?;
        if let Admission::Added(id) | Admission::Recapture(id) = admitted {
            jobs::enqueue(tx, &NewJob::capture_evidence(case.guild, id)).await?;
            added += 1;
        }
    }
    Ok(added)
}
