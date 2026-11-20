//! Adding evidence to a case (§4.9, §7.1): up to 10 message links into this
//! server and up to 10 uploaded files, with one optional note, under the
//! case lock and the per-case caps. A link already saved or being saved is
//! reported as such; a failed one is captured again; a removed one comes
//! back. The files become one uploaded-files item.

use std::collections::{HashMap, HashSet};

use quack_core::DomainError;
use quack_core::case::MessageLink;
use quack_core::evidence::{AddAgain, add_again, check_file_room, check_room, clean_note};
use quack_core::ids::{CaseId, EvidenceId, MessageId};
use quack_core::limits::MAX_LINKS_PER_CALL;
use quack_core::modlog::{EvidenceAddedDetail, ModLogEntry, ModLogLinks};
use quack_core::permissions::Capability;
use quack_db::cases;
use quack_db::evidence::{self, Admission};
use quack_db::jobs::{self, NewJob};
use quack_db::messages::{self, Owner};
use serde::Serialize;

use super::{CaseError, CaseRef, parse_links};
use crate::evidence::{NewUpload, UploadRefused, check_uploads, files_for_hash, upload_item};
use crate::modlog::{self, By};
use crate::opkey::{Keyed, begin_keyed, complete, request_hash, stored_id};
use crate::{Engine, OpKey, Principal, obs};

/// What to add: links, files, or both, and one note for every item the
/// call creates.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AddEvidence {
    /// 0–10 message links into this server.
    pub links: Vec<String>,
    /// 0–10 files, which become one uploaded-files item.
    pub files: Vec<NewUpload>,
    /// The staff note (0–500 characters; blank is none).
    pub note: Option<String>,
}

impl AddEvidence {
    /// Message links only, without a note.
    pub fn links(links: &[String]) -> Self {
        Self {
            links: links.to_vec(),
            ..Self::default()
        }
    }
}

/// What [`add_evidence`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EvidenceAdded {
    /// The case.
    pub case: CaseId,
    /// Its number.
    pub number: u64,
    /// New message items, now `saving`.
    pub new: u32,
    /// Items whose capture had failed, captured again.
    pub retrying: u32,
    /// Removed items shown again.
    pub restored: u32,
    /// Items already saved or being saved.
    pub already_saved: u32,
    /// The new uploaded-files item, when files were added.
    pub upload: Option<EvidenceId>,
    /// How many files it holds.
    pub files: u32,
    /// The same request already ran.
    pub replayed: bool,
}

impl EvidenceAdded {
    /// Whether the call changed anything.
    pub const fn changed(&self) -> bool {
        self.new + self.retrying + self.restored > 0 || self.upload.is_some()
    }
}

#[derive(Serialize)]
struct AddRequest<'a> {
    case: CaseId,
    links: &'a [String],
    // Left out when absent, so a links-only request hashes as before.
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<&'a str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    files: Vec<crate::evidence::FileForHash>,
}

impl From<UploadRefused> for CaseError {
    fn from(refused: UploadRefused) -> Self {
        match refused {
            UploadRefused::Domain(d) => Self::Refused(d),
            UploadRefused::Files(files) => Self::FilesRefused(files),
            UploadRefused::DiscordUnavailable => Self::DiscordUnavailable,
        }
    }
}

/// Admits each link under the case lock with the note, enqueueing capture
/// for new, failed, and restored-failed items.
async fn admit_all(
    tx: &mut quack_db::Tx,
    principal: &Principal,
    case: &quack_db::cases::Case,
    links: &[MessageLink],
    note: Option<&str>,
    result: &mut EvidenceAdded,
) -> Result<(), CaseError> {
    for link in links {
        let id = EvidenceId::from_uuid(uuid::Uuid::now_v7());
        let admitted = evidence::admit(
            tx,
            case.guild,
            case.id,
            id,
            (link.channel_id, link.message_id),
            Some(principal.account),
            note,
        )
        .await?;
        match admitted {
            Admission::Added(_) => result.new += 1,
            Admission::Recapture(_) => result.retrying += 1,
            Admission::Restored { .. } => result.restored += 1,
            // `saving` or `saved`: reported as already saved (§7.1).
            Admission::Already(_, state) if add_again(state) == AddAgain::AlreadySaved => {
                result.already_saved += 1;
            }
            Admission::Already(..) => {}
        }
        if admitted.needs_capture() {
            jobs::enqueue(tx, &NewJob::capture_evidence(case.guild, admitted.id())).await?;
        }
    }
    Ok(())
}

/// The checks that need no lock: something to add, at most 10 links into
/// this server, a short note, and files Quack can copy to a ready evidence
/// channel.
async fn prepare(
    engine: &Engine,
    principal: &Principal,
    add: &AddEvidence,
) -> Result<(Vec<MessageLink>, Option<String>), CaseError> {
    if add.links.is_empty() && add.files.is_empty() {
        return Err(CaseError::NoLinks);
    }
    if add.links.len() > MAX_LINKS_PER_CALL {
        return Err(DomainError::TooManyLinks.into());
    }
    let mut parsed = parse_links(principal.guild, &add.links).map_err(CaseError::LinksInvalid)?;
    let mut seen = HashSet::new();
    parsed.retain(|l| seen.insert(l.message_id));
    let note = clean_note(add.note.as_deref()).map_err(|_| CaseError::NoteLength)?;
    check_uploads(engine, principal.guild, &add.files).await?;
    Ok((parsed, note))
}

/// Adds message links, files, or both to a case's evidence (valid or voided
/// case), with one note on each item it creates (§7.1).
pub async fn add_evidence(
    engine: &Engine,
    principal: &Principal,
    case: CaseRef,
    add: &AddEvidence,
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
        let (parsed, note) = prepare(engine, principal, add).await?;
        let key = op_key.scoped(Some(guild), Some(principal.account));
        let hash = request_hash(&AddRequest {
            case: found.id,
            links: &add.links,
            note: note.as_deref(),
            files: files_for_hash(&add.files),
        });
        let mut tx = match begin_keyed(engine.db(), &key, &hash).await? {
            Keyed::Replay(response) => return replayed(&response, found.number),
            Keyed::Fresh(tx) => tx,
        };
        principal.require(&mut tx, Capability::Moderate).await??;
        let locked = cases::lock(&mut tx, guild, found.id)
            .await?
            .ok_or(DomainError::CaseNotFound)?;
        check_caps(&mut tx, &locked, &parsed, add.files.len()).await?;
        let mut result = EvidenceAdded {
            case: locked.id,
            number: locked.number,
            new: 0,
            retrying: 0,
            restored: 0,
            already_saved: 0,
            upload: None,
            files: 0,
            replayed: false,
        };
        if !add.files.is_empty() {
            let item = upload_item(
                guild,
                locked.id,
                principal.account,
                note.clone(),
                &add.files,
            );
            evidence::insert_upload(&mut tx, &item).await?;
            jobs::enqueue(&mut tx, &NewJob::capture_evidence(guild, item.id)).await?;
            result.upload = Some(item.id);
            result.files = u32::try_from(item.files.len()).unwrap_or(u32::MAX);
        }
        admit_all(
            &mut tx,
            principal,
            &locked,
            &parsed,
            note.as_deref(),
            &mut result,
        )
        .await?;
        if result.changed() {
            record(&mut tx, principal, &locked, &result).await?;
        }
        complete(
            &mut tx,
            &key,
            200,
            serde_json::json!({ "case_id": locked.id, "upload_id": result.upload }),
        )
        .await?;
        tx.commit().await?;
        Ok(result)
    })
    .await
}

/// What a replayed request answers: the case and its upload item, no
/// counts (the first answer had them).
fn replayed(response: &serde_json::Value, number: u64) -> Result<EvidenceAdded, CaseError> {
    Ok(EvidenceAdded {
        case: stored_id(response, "case_id")?,
        number,
        new: 0,
        retrying: 0,
        restored: 0,
        already_saved: 0,
        upload: response
            .get("upload_id")
            .and_then(|v| serde_json::from_value(v.clone()).ok()),
        files: 0,
        replayed: true,
    })
}

/// `evidence_added` (items created, captured again, or restored) and the
/// receipt bump.
async fn record(
    tx: &mut quack_db::Tx,
    principal: &Principal,
    case: &quack_db::cases::Case,
    result: &EvidenceAdded,
) -> Result<(), CaseError> {
    let count = result.new + result.retrying + result.restored + u32::from(result.upload.is_some());
    modlog::write(
        tx,
        case.guild,
        ModLogEntry::EvidenceAdded(EvidenceAddedDetail {
            case_number: case.number,
            count,
        }),
        By {
            actor: Some(principal.account),
            source: principal.origin.mod_log_source(),
        },
        ModLogLinks {
            case_id: Some(case.id),
            member_id: Some(case.member),
            ..ModLogLinks::default()
        },
    )
    .await?;
    messages::bump(tx, case.guild, Owner::Case(case.id)).await?;
    Ok(())
}

/// The per-case caps under the case lock (§7.1): items that aren't removed
/// count; a link to a removed item brings it (and its files) back.
async fn check_caps(
    tx: &mut quack_db::Tx,
    case: &quack_db::cases::Case,
    links: &[MessageLink],
    new_files: usize,
) -> Result<(), CaseError> {
    let existing = evidence::for_case(tx, case.guild, case.id).await?;
    let files = evidence::files_for_case(tx, case.guild, case.id).await?;
    let mut per_item: HashMap<EvidenceId, usize> = HashMap::new();
    for f in &files {
        *per_item.entry(f.evidence).or_default() += 1;
    }
    let by_message: HashMap<MessageId, &evidence::Evidence> = existing
        .iter()
        .filter_map(|e| e.message.map(|m| (m, e)))
        .collect();
    let mut fresh = 0;
    let mut restored_files = 0;
    for link in links {
        match by_message.get(&link.message_id) {
            Some(e) if !e.is_removed() => {}
            Some(e) => {
                fresh += 1;
                restored_files += per_item.get(&e.id).copied().unwrap_or(0);
            }
            None => fresh += 1,
        }
    }
    let active = existing.iter().filter(|e| !e.is_removed());
    let count = u32::try_from(active.clone().count()).unwrap_or(u32::MAX);
    let active_files: usize = active
        .map(|e| per_item.get(&e.id).copied().unwrap_or(0))
        .sum();
    let upload = usize::from(new_files > 0);
    check_room(count, fresh + upload, links.len())?;
    if new_files + restored_files > 0 {
        check_file_room(
            u32::try_from(active_files).unwrap_or(u32::MAX),
            new_files + restored_files,
        )?;
    }
    Ok(())
}
