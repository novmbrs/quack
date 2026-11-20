//! Adding evidence to a case (§4.9, §7.1): up to 10 message links into this
//! server and up to 10 uploaded files, with one optional note, under the
//! case lock and the per-case caps. A link already saved or being saved is
//! reported as such; a failed one is captured again; a removed one comes
//! back, each only for someone who can read its channel now. A message in
//! the evidence channel, or one of Quack's copies, is never a source. The
//! files become one uploaded-files item.

use std::collections::{HashMap, HashSet};

use quack_core::DomainError;
use quack_core::case::{LinkProblem, MessageLink};
use quack_core::evidence::{
    AddAgain, EvidenceState, add_again, check_file_room, check_room, clean_note,
};
use quack_core::ids::{CaseId, EvidenceId, MessageId};
use quack_core::limits::{MAX_LINKS_PER_CALL, MAX_STAGED_BYTES_PER_GUILD, MAX_STAGED_BYTES_TOTAL};
use quack_core::modlog::{EvidenceAddedDetail, ModLogEntry, ModLogLinks};
use quack_core::permissions::Capability;
use quack_db::evidence::{self, Admission, NewUploadItem};
use quack_db::jobs::{self, NewJob};
use quack_db::messages::{self, Owner};
use quack_db::{DbError, cases};
use serde::Serialize;

use super::{CaseError, CaseRef, parse_links};
use crate::evidence::{
    NewUpload, UploadRefused, check_uploads, evidence_copies, files_for_hash, unreadable,
    upload_item,
};
use crate::modlog::{self, By};
use crate::opkey::{Keyed, begin_keyed, complete, is_stored, replay_of, request_hash, stored_id};
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
/// for new, failed, and restored-failed items. `checked` holds the messages
/// whose channel the actor was found able to read; restoring or capturing
/// again any other existing item (it changed since) is [`Again`].
async fn admit_all(
    tx: &mut quack_db::Tx,
    principal: &Principal,
    case: &quack_db::cases::Case,
    links: &[MessageLink],
    checked: &HashSet<MessageId>,
    note: Option<&str>,
    result: &mut EvidenceAdded,
) -> Result<(), Attempt> {
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
        .await
        .map_err(CaseError::from)?;
        if matches!(
            admitted,
            Admission::Recapture(_) | Admission::Restored { .. }
        ) && !checked.contains(&link.message_id)
        {
            return Err(Attempt::Again);
        }
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
            jobs::enqueue(tx, &NewJob::capture_evidence(case.guild, admitted.id()))
                .await
                .map_err(CaseError::from)?;
        }
    }
    Ok(())
}

/// How one attempt of the write ended.
enum Attempt {
    /// An item changed since its access was checked: check and write again.
    Again,
    /// Refused or failed.
    Failed(CaseError),
}

impl<E: Into<CaseError>> From<E> for Attempt {
    fn from(e: E) -> Self {
        Self::Failed(e.into())
    }
}

/// A link with its position in the request.
type Indexed = (usize, MessageLink);

/// The checks that need neither a lock nor Discord: something to add, at
/// most 10 links into this server (each with its position in the request),
/// a short note, and at most 10 files Quack copies.
fn prepare(
    principal: &Principal,
    add: &AddEvidence,
) -> Result<(Vec<Indexed>, Option<String>), CaseError> {
    if add.links.is_empty() && add.files.is_empty() {
        return Err(CaseError::NoLinks);
    }
    if add.links.len() > MAX_LINKS_PER_CALL {
        return Err(DomainError::TooManyLinks.into());
    }
    let parsed = parse_links(principal.guild, &add.links).map_err(CaseError::LinksInvalid)?;
    let mut seen = HashSet::new();
    let parsed = parsed
        .into_iter()
        .enumerate()
        .filter(|(_, l)| seen.insert(l.message_id))
        .collect();
    let note = clean_note(add.note.as_deref()).map_err(|_| CaseError::NoteLength)?;
    if !add.files.is_empty() {
        crate::evidence::check_files(&add.files)?;
    }
    Ok((parsed, note))
}

/// The checks that read Discord or the case before the write (§7.1): a
/// ready evidence channel for files; no link into the evidence channel or
/// to one of Quack's copies; and the actor's access to the channel of every
/// link whose item on the case is removed or failed, since adding it again
/// restores it or captures it again (one already saved or saving is only
/// reported). Returns the messages checked that way.
async fn check_sources(
    engine: &Engine,
    principal: &Principal,
    case: CaseId,
    links: &[Indexed],
    files: &[NewUpload],
) -> Result<HashSet<MessageId>, CaseError> {
    check_uploads(engine, principal.guild, files).await?;
    let plain: Vec<MessageLink> = links.iter().map(|(_, l)| *l).collect();
    let copies = evidence_copies(engine, principal.guild, &plain).await?;
    let existing: HashSet<MessageId> = {
        let mut conn = engine.db().conn().await?;
        evidence::for_case(&mut conn, principal.guild, case)
            .await?
            .into_iter()
            .filter(|e| e.is_removed() || e.state == EvidenceState::Failed)
            .filter_map(|e| e.message)
            .collect()
    };
    let again: Vec<&MessageLink> = plain
        .iter()
        .filter(|l| existing.contains(&l.message_id) && !copies.contains(&l.message_id))
        .collect();
    let blind = unreadable(engine, principal.guild, principal.account, &again)
        .await
        .map_err(|_| CaseError::DiscordUnavailable)?;
    let problems: Vec<(usize, LinkProblem)> = links
        .iter()
        .filter_map(|(i, l)| {
            if copies.contains(&l.message_id) {
                Some((*i, LinkProblem::EvidenceChannel))
            } else if blind.contains(&l.message_id) {
                Some((*i, LinkProblem::NoAccess))
            } else {
                None
            }
        })
        .collect();
    if !problems.is_empty() {
        return Err(CaseError::LinksInvalid(problems));
    }
    Ok(again.iter().map(|l| l.message_id).collect())
}

/// The staging ceilings (§7.1), under a lock that serializes staging across
/// the service: the server's and everyone's staged bytes, with this
/// upload's, must stay within their caps, else `uploads_busy`.
async fn check_staging(tx: &mut quack_db::Tx, item: &NewUploadItem) -> Result<(), CaseError> {
    if item.staged.is_empty() {
        return Ok(());
    }
    let adding: u64 = item
        .staged
        .iter()
        .map(|(_, b)| u64::try_from(b.len()).unwrap_or(u64::MAX))
        .sum();
    let staged = evidence::lock_staged_totals(tx, item.guild).await?;
    if staged.guild.saturating_add(adding) > MAX_STAGED_BYTES_PER_GUILD
        || staged.total.saturating_add(adding) > MAX_STAGED_BYTES_TOTAL
    {
        return Err(DomainError::UploadsBusy.into());
    }
    Ok(())
}

/// Adds message links, files, or both to a case's evidence (valid or voided
/// case), with one note on each item it creates (§7.1). A repeat of a
/// request that already ran replays it after authorization, whatever the
/// evidence channel's state now.
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
        let (parsed, note) = prepare(principal, add)?;
        let key = op_key.scoped(Some(guild), Some(principal.account));
        let hash = request_hash(&AddRequest {
            case: found.id,
            links: &add.links,
            note: note.as_deref(),
            files: files_for_hash(&add.files),
        });
        if let Some(response) = replay_of(engine.db(), &key, &hash).await? {
            return replayed(&response, found.number);
        }
        let links: Vec<MessageLink> = parsed.iter().map(|(_, l)| *l).collect();
        // An item that changed between the access check and the lock is
        // checked again, once.
        for _ in 0..2 {
            let checked = check_sources(engine, principal, found.id, &parsed, &add.files).await?;
            let written = write(
                engine,
                principal,
                &found,
                &links,
                &checked,
                note.as_deref(),
                &add.files,
                (&key, &hash),
            )
            .await;
            match written {
                Ok(result) => return Ok(result),
                Err(Attempt::Failed(e)) => return Err(e),
                Err(Attempt::Again) => {}
            }
        }
        Err(DbError::Stale.into())
    })
    .await
}

/// Whether an add already ran under this operation key, so a dashboard
/// upload can skip its up-front evidence channel check: a replay never
/// depends on the channel (§4.3), and [`add_evidence`] still compares the
/// request's hash before answering with the stored result.
pub async fn add_evidence_ran(
    engine: &Engine,
    principal: &Principal,
    op_key: &OpKey,
) -> Result<bool, CaseError> {
    let key = op_key.scoped(Some(principal.guild), Some(principal.account));
    Ok(is_stored(engine.db(), &key).await?)
}

/// The write: the key, the caps, the upload item, the links, the log entry,
/// and the receipt bump, in one transaction under the case lock.
#[allow(clippy::too_many_arguments, reason = "one transaction's inputs")]
async fn write(
    engine: &Engine,
    principal: &Principal,
    found: &quack_db::cases::Case,
    links: &[MessageLink],
    checked: &HashSet<MessageId>,
    note: Option<&str>,
    files: &[NewUpload],
    (key, hash): (&quack_db::operation_keys::OperationKey, &[u8; 32]),
) -> Result<EvidenceAdded, Attempt> {
    let guild = principal.guild;
    let mut tx = match begin_keyed(engine.db(), key, hash).await? {
        Keyed::Replay(response) => return Ok(replayed(&response, found.number)?),
        Keyed::Fresh(tx) => tx,
    };
    principal.require(&mut tx, Capability::Moderate).await??;
    let locked = cases::lock(&mut tx, guild, found.id)
        .await?
        .ok_or(DomainError::CaseNotFound)?;
    check_caps(&mut tx, &locked, links, files.len()).await?;
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
    if !files.is_empty() {
        let item = upload_item(
            guild,
            locked.id,
            principal.account,
            note.map(str::to_owned),
            files,
        );
        check_staging(&mut tx, &item).await?;
        evidence::insert_upload(&mut tx, &item).await?;
        jobs::enqueue(&mut tx, &NewJob::capture_evidence(guild, item.id)).await?;
        result.upload = Some(item.id);
        result.files = u32::try_from(item.files.len()).unwrap_or(u32::MAX);
    }
    admit_all(
        &mut tx,
        principal,
        &locked,
        links,
        checked,
        note,
        &mut result,
    )
    .await?;
    if result.changed() {
        record(&mut tx, principal, &locked, &result).await?;
    }
    complete(
        &mut tx,
        key,
        200,
        serde_json::json!({ "case_id": locked.id, "upload_id": result.upload }),
    )
    .await?;
    tx.commit().await?;
    Ok(result)
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
