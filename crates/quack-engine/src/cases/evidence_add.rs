//! Adding evidence to a case (§4.9, §7.1): 1–10 message links into this
//! server, under the case lock and the per-case cap. A link already saved
//! or being saved is reported as such; a failed one is captured again.

use quack_core::DomainError;
use quack_core::evidence::{AddAgain, add_again, check_room};
use quack_core::ids::{CaseId, EvidenceId};
use quack_core::limits::MAX_LINKS_PER_CALL;
use quack_core::modlog::{EvidenceAddedDetail, ModLogEntry, ModLogLinks};
use quack_core::permissions::Capability;
use quack_db::cases;
use quack_db::evidence::{self, Admission};
use quack_db::jobs::{self, NewJob};
use quack_db::messages::{self, Owner};
use serde::Serialize;

use super::{CaseError, CaseRef, parse_links};
use crate::modlog::{self, By};
use crate::opkey::{Keyed, begin_keyed, complete, request_hash, stored_id};
use crate::{Engine, OpKey, Principal, obs};

/// What [`add_evidence`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EvidenceAdded {
    /// The case.
    pub case: CaseId,
    /// Its number.
    pub number: u64,
    /// New items, now `saving`.
    pub new: u32,
    /// Items whose capture had failed, captured again.
    pub retrying: u32,
    /// Items already saved or being saved.
    pub already_saved: u32,
    /// The same request already ran.
    pub replayed: bool,
}

#[derive(Serialize)]
struct AddRequest<'a> {
    case: CaseId,
    links: &'a [String],
}

/// Admits each link under the case lock, enqueueing capture for new and
/// failed items.
async fn admit_all(
    tx: &mut quack_db::Tx,
    principal: &Principal,
    case: &quack_db::cases::Case,
    links: &[quack_core::case::MessageLink],
) -> Result<EvidenceAdded, CaseError> {
    let mut result = EvidenceAdded {
        case: case.id,
        number: case.number,
        new: 0,
        retrying: 0,
        already_saved: 0,
        replayed: false,
    };
    for link in links {
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
        match admitted {
            Admission::Added(id) => {
                result.new += 1;
                jobs::enqueue(tx, &NewJob::capture_evidence(case.guild, id)).await?;
            }
            Admission::Recapture(id) => {
                result.retrying += 1;
                jobs::enqueue(tx, &NewJob::capture_evidence(case.guild, id)).await?;
            }
            // `saving` or `saved`: reported as already saved; a failed
            // capture was readmitted above (§7.1).
            Admission::Already(_, state) if add_again(state) == AddAgain::AlreadySaved => {
                result.already_saved += 1;
            }
            Admission::Already(..) => {}
        }
    }
    Ok(result)
}

/// Adds 1–10 message links to a case's evidence (valid or voided case).
pub async fn add_evidence(
    engine: &Engine,
    principal: &Principal,
    case: CaseRef,
    links: &[String],
    op_key: &OpKey,
) -> Result<EvidenceAdded, CaseError> {
    let guild = principal.guild;
    obs::use_case("add_evidence", Some(guild), async {
        let found = {
            let mut conn = engine.db().conn().await?;
            principal.require(&mut conn, Capability::Moderate).await??;
            case.find(&mut conn, guild)
                .await?
                .ok_or(DomainError::CaseNotFound)?
        };
        if links.is_empty() {
            return Err(CaseError::NoLinks);
        }
        if links.len() > MAX_LINKS_PER_CALL {
            return Err(DomainError::TooManyLinks.into());
        }
        let mut parsed = parse_links(guild, links).map_err(CaseError::LinksInvalid)?;
        let mut seen = std::collections::HashSet::new();
        parsed.retain(|l| seen.insert(l.message_id));
        let key = op_key.scoped(Some(guild), Some(principal.account));
        let hash = request_hash(&AddRequest {
            case: found.id,
            links,
        });
        let mut tx = match begin_keyed(engine.db(), &key, &hash).await? {
            Keyed::Replay(response) => {
                return Ok(EvidenceAdded {
                    case: stored_id(&response, "case_id")?,
                    number: found.number,
                    new: 0,
                    retrying: 0,
                    already_saved: 0,
                    replayed: true,
                });
            }
            Keyed::Fresh(tx) => tx,
        };
        principal.require(&mut tx, Capability::Moderate).await??;
        let locked = cases::lock(&mut tx, guild, found.id)
            .await?
            .ok_or(DomainError::CaseNotFound)?;
        let existing = evidence::for_case(&mut tx, guild, locked.id).await?;
        let fresh = parsed
            .iter()
            .filter(|l| !existing.iter().any(|e| e.message == l.message_id))
            .count();
        let count = u32::try_from(existing.len()).unwrap_or(u32::MAX);
        check_room(count, fresh, parsed.len())?;
        let result = admit_all(&mut tx, principal, &locked, &parsed).await?;
        if result.new + result.retrying > 0 {
            modlog::write(
                &mut tx,
                guild,
                ModLogEntry::EvidenceAdded(EvidenceAddedDetail {
                    case_number: locked.number,
                    count: result.new + result.retrying,
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
        }
        complete(
            &mut tx,
            &key,
            200,
            serde_json::json!({ "case_id": locked.id }),
        )
        .await?;
        tx.commit().await?;
        Ok(result)
    })
    .await
}
