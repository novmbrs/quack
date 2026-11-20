//! Reads for the evidence surfaces: the image and video gallery of View
//! evidence (§11.7), and the cases the "Add as evidence" message command
//! offers (§11.1).

use quack_core::DomainError;
use quack_core::evidence::FileCopyState;
use quack_core::ids::{EvidenceId, FileId, MemberId};
use quack_core::limits::{MAX_FILES_PER_EVIDENCE, RECENT_CASES};
use quack_core::permissions::Capability;

use super::file::copy_url;
use crate::cases::{CaseError, CaseRef};
use crate::{Engine, Principal, obs};

pub use quack_db::cases::RecentCase;

/// One copied image or video with a fresh CDN URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GalleryFile {
    /// The file.
    pub id: FileId,
    /// Its name.
    pub filename: String,
    /// Its content type.
    pub content_type: Option<String>,
    /// A fresh URL of its copy (signed CDN URLs expire, so never stored).
    pub url: String,
}

/// Whether a content type shows in a media gallery.
fn is_media(content_type: Option<&str>) -> bool {
    content_type.is_some_and(|t| {
        let t = t.trim().to_ascii_lowercase();
        t.starts_with("image/") || t.starts_with("video/")
    })
}

/// Fresh URLs of an item's copied images and videos, in order, for the
/// spoilered gallery of View evidence: only copies in the evidence channel,
/// never the source message's attachment (§7.4). Best effort: a file whose
/// copy can't be read now is left out. A removed item has no gallery.
pub async fn gallery(
    engine: &Engine,
    principal: &Principal,
    case: CaseRef,
    evidence: EvidenceId,
) -> Result<Vec<GalleryFile>, CaseError> {
    let guild = principal.guild;
    obs::use_case("evidence_gallery", Some(guild), async {
        let (item, files) = {
            let mut conn = engine.db().conn().await?;
            principal.require(&mut conn, Capability::Moderate).await??;
            let case = case
                .find(&mut conn, guild)
                .await?
                .ok_or(DomainError::CaseNotFound)?;
            let item = quack_db::evidence::get(&mut conn, guild, evidence)
                .await?
                .filter(|e| e.case == case.id)
                .ok_or(DomainError::EvidenceNotFound)?;
            let files = quack_db::evidence::files_for_case(&mut conn, guild, case.id).await?;
            (item, files)
        };
        if item.is_removed() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        let media = files.iter().filter(|f| {
            f.evidence == item.id
                && f.copy_state == FileCopyState::Copied
                && is_media(f.content_type.as_deref())
        });
        for f in media.take(MAX_FILES_PER_EVIDENCE as usize) {
            if let Ok(Some(url)) = copy_url(engine, f).await {
                out.push(GalleryFile {
                    id: f.id,
                    filename: f.filename.clone(),
                    content_type: f.content_type.clone(),
                    url,
                });
            }
        }
        Ok(out)
    })
    .await
}

/// The cases the "Add as evidence" message command offers (§11.1): the
/// message author's cases in this server and the actor's own, newest
/// first, without repeats, at most 25, voided ones included (marked by
/// the text).
pub async fn recent_cases(
    engine: &Engine,
    principal: &Principal,
    author: Option<MemberId>,
) -> Result<Vec<RecentCase>, CaseError> {
    obs::use_case("recent_cases", Some(principal.guild), async {
        let mut conn = engine.db().conn().await?;
        principal.require(&mut conn, Capability::Moderate).await??;
        Ok(quack_db::cases::recent(
            &mut conn,
            principal.guild,
            author,
            principal.account,
            RECENT_CASES,
        )
        .await?)
    })
    .await
}
