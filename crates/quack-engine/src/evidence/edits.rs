//! Staff edits of one evidence item (§7.5): its note, whether the member
//! sees an upload, and removal. Each takes an operation key, re-checks
//! `moderate` + 2FA in its transaction, and locks the case, then the item.

use quack_core::DomainError;
use quack_core::evidence::{check_editable, clean_note};
use quack_core::ids::{CaseId, EvidenceId};
use quack_core::modlog::{
    EvidenceRemovedDetail, EvidenceVisibilityDetail, ModLogEntry, ModLogLinks,
};
use quack_core::permissions::Capability;
use quack_db::evidence::{self, Edited, Evidence};
use quack_db::messages::{self, Owner};
use quack_db::operation_keys::OperationKey;
use quack_db::{Tx, cases};
use serde::Serialize;

use crate::cases::{CaseError, CaseRef};
use crate::modlog::{self, By};
use crate::opkey::{Keyed, begin_keyed, complete, request_hash, stored_id};
use crate::{Engine, OpKey, Principal, obs};

/// What an edit did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EvidenceEdited {
    /// The item's case.
    pub case: CaseId,
    /// Its number.
    pub number: u64,
    /// The item.
    pub evidence: EvidenceId,
    /// Whether anything changed (setting the current value changes nothing
    /// and records nothing).
    pub changed: bool,
    /// The same request already ran.
    pub replayed: bool,
}

#[derive(Serialize)]
struct EditRequest<'a> {
    evidence: EvidenceId,
    edit: &'a str,
    note: Option<&'a str>,
    revision: Option<u32>,
    visible: Option<bool>,
}

/// An edit that holds its key and locks.
struct Began {
    tx: Tx,
    key: OperationKey,
    case: quack_db::cases::Case,
    item: Evidence,
}

enum Start {
    Replay(EvidenceEdited),
    Fresh(Box<Began>),
}

/// Authorizes, finds the item (on `case` when given), reserves the key,
/// and locks the case and the item.
async fn begin(
    engine: &Engine,
    principal: &Principal,
    case: Option<CaseRef>,
    evidence: EvidenceId,
    op_key: &OpKey,
    request: &EditRequest<'_>,
) -> Result<Start, CaseError> {
    let guild = principal.guild;
    let (found, number) = {
        let mut conn = engine.db().conn().await?;
        principal.require(&mut conn, Capability::Moderate).await??;
        let item = evidence::get(&mut conn, guild, evidence)
            .await?
            .ok_or(DomainError::EvidenceNotFound)?;
        let owner = cases::get(&mut conn, guild, item.case)
            .await?
            .ok_or(DomainError::EvidenceNotFound)?;
        if let Some(named) = case {
            let named = named
                .find(&mut conn, guild)
                .await?
                .ok_or(DomainError::CaseNotFound)?;
            if named.id != owner.id {
                return Err(DomainError::EvidenceNotFound.into());
            }
        }
        (item, owner.number)
    };
    let key = op_key.scoped(Some(guild), Some(principal.account));
    let hash = request_hash(request);
    let mut tx = match begin_keyed(engine.db(), &key, &hash).await? {
        Keyed::Replay(response) => {
            return Ok(Start::Replay(EvidenceEdited {
                case: stored_id(&response, "case_id")?,
                number,
                evidence,
                changed: false,
                replayed: true,
            }));
        }
        Keyed::Fresh(tx) => tx,
    };
    principal.require(&mut tx, Capability::Moderate).await??;
    let case = cases::lock(&mut tx, guild, found.case)
        .await?
        .ok_or(DomainError::EvidenceNotFound)?;
    let item = evidence::lock(&mut tx, guild, evidence)
        .await?
        .ok_or(DomainError::EvidenceNotFound)?;
    Ok(Start::Fresh(Box::new(Began {
        tx,
        key,
        case,
        item,
    })))
}

/// Completes the key and commits.
async fn finish(began: Began, changed: bool) -> Result<EvidenceEdited, CaseError> {
    let Began {
        mut tx,
        key,
        case,
        item,
    } = began;
    complete(
        &mut tx,
        &key,
        200,
        serde_json::json!({ "case_id": case.id }),
    )
    .await?;
    tx.commit().await?;
    Ok(EvidenceEdited {
        case: case.id,
        number: case.number,
        evidence: item.id,
        changed,
        replayed: false,
    })
}

fn by(principal: &Principal) -> By {
    By {
        actor: Some(principal.account),
        source: principal.origin.mod_log_source(),
    }
}

fn links(case: &quack_db::cases::Case) -> ModLogLinks {
    ModLogLinks {
        case_id: Some(case.id),
        member_id: Some(case.member),
        ..ModLogLinks::default()
    }
}

/// Sets or clears an item's staff note (0–500 characters; blank clears),
/// if it is still at `expected_revision` (else `note_changed`). Not
/// recorded in the moderation log. Refused on a removed item.
pub async fn set_note(
    engine: &Engine,
    principal: &Principal,
    case: Option<CaseRef>,
    evidence: EvidenceId,
    note: Option<&str>,
    expected_revision: u32,
    op_key: &OpKey,
) -> Result<EvidenceEdited, CaseError> {
    obs::use_case("set_evidence_note", Some(principal.guild), async {
        let note = clean_note(note).map_err(|_| CaseError::NoteLength)?;
        let request = EditRequest {
            evidence,
            edit: "note",
            note: note.as_deref(),
            revision: Some(expected_revision),
            visible: None,
        };
        let mut began = match begin(engine, principal, case, evidence, op_key, &request).await? {
            Start::Replay(done) => return Ok(done),
            Start::Fresh(began) => *began,
        };
        check_editable(began.item.kind, began.item.is_removed(), false)?;
        if began.item.note_revision != expected_revision {
            return Err(DomainError::NoteChanged.into());
        }
        let edited = evidence::set_note(
            &mut began.tx,
            principal.guild,
            evidence,
            note.as_deref(),
            expected_revision,
            principal.account,
        )
        .await?;
        finish(began, edited == Edited::Changed).await
    })
    .await
}

/// Shows an uploaded-files item to the case's member on their case page,
/// or hides it (§7.4). Saved messages are always visible
/// (`evidence_not_upload`). Logged `evidence_visibility_changed`.
pub async fn set_visibility(
    engine: &Engine,
    principal: &Principal,
    case: Option<CaseRef>,
    evidence: EvidenceId,
    visible: bool,
    op_key: &OpKey,
) -> Result<EvidenceEdited, CaseError> {
    obs::use_case("set_evidence_visibility", Some(principal.guild), async {
        let request = EditRequest {
            evidence,
            edit: "visibility",
            note: None,
            revision: None,
            visible: Some(visible),
        };
        let mut began = match begin(engine, principal, case, evidence, op_key, &request).await? {
            Start::Replay(done) => return Ok(done),
            Start::Fresh(began) => *began,
        };
        check_editable(began.item.kind, began.item.is_removed(), true)?;
        let edited =
            evidence::set_visibility(&mut began.tx, principal.guild, evidence, visible).await?;
        if edited == Edited::Changed {
            let entry = ModLogEntry::EvidenceVisibilityChanged(EvidenceVisibilityDetail {
                case_number: began.case.number,
                visible_to_member: visible,
            });
            let links = links(&began.case);
            modlog::write(&mut began.tx, principal.guild, entry, by(principal), links).await?;
        }
        finish(began, edited == Edited::Changed).await
    })
    .await
}

/// Removes an item added by mistake (§7.5): hidden from Discord and the
/// member, shown collapsed on the staff dashboard, never deleted (its copies
/// stay in the evidence channel), out of the per-case caps. Logged
/// `evidence_removed`. Removing a removed item changes nothing; adding a
/// removed message's link again brings it back.
pub async fn remove_evidence(
    engine: &Engine,
    principal: &Principal,
    case: Option<CaseRef>,
    evidence: EvidenceId,
    op_key: &OpKey,
) -> Result<EvidenceEdited, CaseError> {
    obs::use_case("remove_evidence", Some(principal.guild), async {
        let request = EditRequest {
            evidence,
            edit: "remove",
            note: None,
            revision: None,
            visible: None,
        };
        let mut began = match begin(engine, principal, case, evidence, op_key, &request).await? {
            Start::Replay(done) => return Ok(done),
            Start::Fresh(began) => *began,
        };
        let edited =
            evidence::remove(&mut began.tx, principal.guild, evidence, principal.account).await?;
        if edited == Edited::Changed {
            let entry = ModLogEntry::EvidenceRemoved(EvidenceRemovedDetail {
                case_number: began.case.number,
                evidence_kind: began.item.kind,
            });
            let links = links(&began.case);
            modlog::write(&mut began.tx, principal.guild, entry, by(principal), links).await?;
            messages::bump(&mut began.tx, principal.guild, Owner::Case(began.case.id)).await?;
        }
        finish(began, edited == Edited::Changed).await
    })
    .await
}
