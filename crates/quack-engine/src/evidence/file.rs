//! Resolving one evidence file to a fresh Discord CDN URL (§7.4): fetch the
//! copy message when the file was copied, else (or when the copy is gone)
//! the source message, and find the attachment by its ID. The member gets
//! only files of items they see (§7.4): never a removed item's or a
//! staff-only upload's.

use quack_core::DomainError;
use quack_core::evidence::{FileCopyState, member_sees};
use quack_core::ids::{AccountId, AttachmentId, CaseId, ChannelId, FileId, MessageId};
use quack_core::permissions::Capability;
use quack_db::DbError;
use quack_db::cases::Case;

use crate::port::Outcome;
use crate::{CaseRef, Engine, Principal, obs};

/// Who asks for the file.
#[derive(Clone, Copy, Debug)]
pub enum FileViewer<'a> {
    /// Staff with `moderate` + 2FA, for a case of their server.
    Staff {
        /// The staff member.
        principal: &'a Principal,
        /// The case.
        case: CaseRef,
    },
    /// The member, for one of their own cases.
    Member {
        /// Their account.
        account: AccountId,
        /// The case.
        case: CaseId,
    },
}

/// Why a file couldn't be resolved.
#[derive(Debug, thiserror::Error)]
pub enum FileError {
    /// No such case for this viewer, or no such file on it (`not_found`).
    #[error("not found")]
    NotFound,
    /// Neither message has the attachment any more (`file_unavailable`).
    #[error("file unavailable")]
    Unavailable,
    /// Discord didn't answer.
    #[error("Discord unavailable")]
    DiscordUnavailable,
    /// A refusal (capability, 2FA).
    #[error("refused: {0:?}")]
    Refused(DomainError),
    /// The database failed.
    #[error(transparent)]
    Db(#[from] DbError),
}

impl From<DomainError> for FileError {
    fn from(error: DomainError) -> Self {
        Self::Refused(error)
    }
}

/// The CDN URL of one evidence file, for `viewer`.
pub async fn resolve_file(
    engine: &Engine,
    viewer: FileViewer<'_>,
    file: FileId,
) -> Result<String, FileError> {
    let case = find(engine, viewer).await?;
    obs::use_case("resolve_evidence_file", Some(case.guild), async {
        let found = {
            let mut conn = engine.db().conn().await?;
            quack_db::evidence::file(&mut conn, case.guild, case.id, file).await?
        };
        let (file, item) = found.ok_or(FileError::NotFound)?;
        if matches!(viewer, FileViewer::Member { .. })
            && !member_sees(item.kind, item.visible_to_member, item.is_removed())
        {
            return Err(FileError::NotFound);
        }
        fresh_url(engine, &file, item.source()).await
    })
    .await
}

/// A fresh URL for a file: its copy, else the source message's attachment
/// (an uploaded file has no source message, and its interaction attachment
/// URL is short-lived, so only the copy serves it).
pub(crate) async fn fresh_url(
    engine: &Engine,
    file: &quack_db::evidence::EvidenceFile,
    source: Option<(ChannelId, MessageId)>,
) -> Result<String, FileError> {
    if file.copy_state == FileCopyState::Copied
        && let Some((channel, message, attachment)) = file.copy
        && let Some(url) = locate(engine, channel, message, attachment).await?
    {
        return Ok(url);
    }
    match (file.attachment_id, source) {
        (Some(attachment), Some((channel, message))) => {
            locate(engine, channel, message, attachment)
                .await?
                .ok_or(FileError::Unavailable)
        }
        _ => Err(FileError::Unavailable),
    }
}

/// The viewer's case.
async fn find(engine: &Engine, viewer: FileViewer<'_>) -> Result<Case, FileError> {
    let mut conn = engine.db().conn().await?;
    let found = match viewer {
        FileViewer::Staff { principal, case } => {
            principal.require(&mut conn, Capability::Moderate).await??;
            case.find(&mut conn, principal.guild).await?
        }
        FileViewer::Member { account, case } => {
            quack_db::cases::get_for_member(&mut conn, account.member(), case).await?
        }
    };
    found.ok_or(FileError::NotFound)
}

/// Where an attachment is now; `None` when the message or the attachment
/// is gone.
async fn locate(
    engine: &Engine,
    channel: ChannelId,
    message: MessageId,
    attachment: AttachmentId,
) -> Result<Option<String>, FileError> {
    match engine.discord().fetch_message(channel, message).await {
        Outcome::Done(m) => Ok(m
            .attachments
            .into_iter()
            .find(|a| a.id == attachment)
            .map(|a| a.url)),
        Outcome::Refused(_) => Ok(None),
        Outcome::RetryAfter(_) | Outcome::NotSent(_) | Outcome::Unknown(_) => {
            Err(FileError::DiscordUnavailable)
        }
    }
}
