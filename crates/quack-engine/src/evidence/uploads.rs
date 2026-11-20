//! Files staff upload as evidence (§7.1): where their bytes come from, the
//! checks that refuse an upload before anything is saved, and the rows an
//! accepted upload becomes.

use quack_core::DomainError;
use quack_core::evidence::{
    FileCopyState, OfferedFile, RefusedFile, check_upload_count, refused_uploads, upload_filename,
};
use quack_core::ids::{AccountId, AttachmentId, CaseId, EvidenceId, FileId, GuildId};
use quack_core::permissions::Permission;
use quack_db::evidence::{NewFile, NewUploadItem};
use serde::Serialize;

use crate::Engine;
use crate::channels::{ChannelCheck, staff_channel};

/// Where an uploaded file's bytes come from.
#[derive(Clone, PartialEq, Eq)]
pub enum UploadSource {
    /// A Discord interaction attachment (the Add evidence modal's file
    /// field, or a slash command's ATTACHMENT option): the capture job
    /// downloads it from its short-lived CDN URL.
    Discord {
        /// Discord's attachment id.
        attachment_id: AttachmentId,
        /// The `cdn.discordapp.com/ephemeral-attachments/…` URL.
        url: String,
    },
    /// Bytes the dashboard sent; staged in PostgreSQL until copied.
    Bytes(Vec<u8>),
}

impl std::fmt::Debug for UploadSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Discord { attachment_id, .. } => f
                .debug_struct("Discord")
                .field("attachment_id", attachment_id)
                .finish_non_exhaustive(),
            Self::Bytes(bytes) => write!(f, "Bytes({} bytes)", bytes.len()),
        }
    }
}

/// One file offered for upload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewUpload {
    /// Its name as offered (cleaned with `upload_filename` when stored).
    pub filename: String,
    /// Its content type: Discord's for interaction attachments, the
    /// browser's for dashboard uploads.
    pub content_type: Option<String>,
    /// Its size; for [`UploadSource::Bytes`] the bytes' length wins.
    pub size_bytes: u64,
    /// Where the bytes are.
    pub source: UploadSource,
}

impl NewUpload {
    /// A dashboard upload.
    pub fn bytes(
        filename: impl Into<String>,
        content_type: Option<String>,
        bytes: Vec<u8>,
    ) -> Self {
        Self {
            filename: filename.into(),
            content_type,
            size_bytes: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            source: UploadSource::Bytes(bytes),
        }
    }

    fn size(&self) -> u64 {
        match &self.source {
            UploadSource::Bytes(b) => u64::try_from(b.len()).unwrap_or(u64::MAX),
            UploadSource::Discord { .. } => self.size_bytes,
        }
    }
}

/// Why an upload was refused before anything was saved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UploadRefused {
    /// Too many files, or the evidence channel isn't ready
    /// (`error.too_many_files`, `error.evidence_channel_missing`,
    /// `error.evidence_channel_not_staff_only`).
    Domain(DomainError),
    /// These files can't be copied (`error.files_not_allowed`).
    Files(Vec<RefusedFile>),
    /// Discord didn't answer the channel check; nothing was saved.
    DiscordUnavailable,
}

/// The pure checks (§7.1): at most 10 files, each one Quack copies.
pub fn check_files(files: &[NewUpload]) -> Result<(), UploadRefused> {
    check_upload_count(files.len()).map_err(UploadRefused::Domain)?;
    let offered: Vec<OfferedFile<'_>> = files
        .iter()
        .map(|f| OfferedFile {
            filename: &f.filename,
            size_bytes: f.size(),
            content_type: f.content_type.as_deref(),
        })
        .collect();
    let refused = refused_uploads(&offered);
    if refused.is_empty() {
        Ok(())
    } else {
        Err(UploadRefused::Files(refused))
    }
}

/// Whether uploads can be copied now (§7.1): an evidence channel is set
/// and passes the staff-only check (§13.1.3) with View Channel, Send
/// Messages, and Attach Files for Quack. The capture job checks again
/// before every upload (§7.3).
pub async fn check_destination(engine: &Engine, guild: GuildId) -> Result<(), UploadRefused> {
    let channel = {
        let mut conn = engine
            .db()
            .conn()
            .await
            .map_err(|_| UploadRefused::DiscordUnavailable)?;
        quack_db::settings::get(&mut conn, guild)
            .await
            .map_err(|_| UploadRefused::DiscordUnavailable)?
            .and_then(|s| s.evidence_channel)
    };
    let Some(channel) = channel else {
        return Err(UploadRefused::Domain(DomainError::EvidenceChannelMissing));
    };
    let needs = Permission::ViewChannel | Permission::SendMessages | Permission::AttachFiles;
    match staff_channel(engine, guild, channel, needs).await {
        ChannelCheck::Ok => Ok(()),
        ChannelCheck::Missing => Err(UploadRefused::Domain(DomainError::EvidenceChannelMissing)),
        ChannelCheck::Problems(_) => Err(UploadRefused::Domain(
            DomainError::EvidenceChannelNotStaffOnly,
        )),
        ChannelCheck::Unavailable => Err(UploadRefused::DiscordUnavailable),
    }
}

/// Every up-front check of an upload: the files, then the destination.
/// No files is fine (nothing to check).
pub async fn check_uploads(
    engine: &Engine,
    guild: GuildId,
    files: &[NewUpload],
) -> Result<(), UploadRefused> {
    if files.is_empty() {
        return Ok(());
    }
    check_files(files)?;
    check_destination(engine, guild).await
}

/// The rows an accepted upload becomes: one `saving` item, its files
/// `pending` (in order), and the bytes of dashboard files.
pub(crate) fn upload_item(
    guild: GuildId,
    case: CaseId,
    added_by: AccountId,
    note: Option<String>,
    files: &[NewUpload],
) -> NewUploadItem {
    let mut item = NewUploadItem {
        id: EvidenceId::from_uuid(uuid::Uuid::now_v7()),
        guild,
        case,
        added_by,
        note,
        files: Vec::new(),
        staged: Vec::new(),
    };
    for (i, f) in files.iter().enumerate() {
        let id = FileId::from_uuid(uuid::Uuid::now_v7());
        let (attachment_id, original_url) = match &f.source {
            UploadSource::Discord { attachment_id, url } => {
                (Some(*attachment_id), Some(url.clone()))
            }
            UploadSource::Bytes(bytes) => {
                item.staged.push((id, bytes.clone()));
                (None, None)
            }
        };
        item.files.push(NewFile {
            id,
            position: u8::try_from(i + 1).unwrap_or(u8::MAX),
            attachment_id,
            filename: upload_filename(&f.filename),
            content_type: f.content_type.clone(),
            size_bytes: f.size(),
            original_url,
            copy_state: FileCopyState::Pending,
        });
    }
    item
}

/// What a request hash sees of one file: its name, type, size, and the
/// attachment id or a SHA-256 of the bytes (never the bytes or the URL).
#[derive(Serialize)]
pub(crate) struct FileForHash {
    filename: String,
    content_type: Option<String>,
    size_bytes: u64,
    attachment_id: Option<AttachmentId>,
    sha256: Option<Vec<u8>>,
}

/// The files as a request hash sees them.
pub(crate) fn files_for_hash(files: &[NewUpload]) -> Vec<FileForHash> {
    files
        .iter()
        .map(|f| {
            let (attachment_id, sha256) = match &f.source {
                UploadSource::Discord { attachment_id, .. } => (Some(*attachment_id), None),
                UploadSource::Bytes(bytes) => {
                    let digest = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, bytes);
                    (None, Some(digest.as_ref().to_vec()))
                }
            };
            FileForHash {
                filename: f.filename.clone(),
                content_type: f.content_type.clone(),
                size_bytes: f.size(),
                attachment_id,
                sha256,
            }
        })
        .collect()
}
