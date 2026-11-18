//! Resolving one evidence file to a fresh Discord CDN URL (§7.4): fetch the
//! copy message when the file was copied, else (or when the copy is gone)
//! the source message, and find the attachment by its ID.

use quack_core::DomainError;
use quack_core::evidence::FileCopyState;
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
        let (file, source_channel, source_message) = found.ok_or(FileError::NotFound)?;
        if file.copy_state == FileCopyState::Copied
            && let Some((channel, message, attachment)) = file.copy
            && let Some(url) = locate(engine, channel, message, attachment).await?
        {
            return Ok(url);
        }
        match file.attachment_id {
            Some(attachment) => locate(engine, source_channel, source_message, attachment)
                .await?
                .ok_or(FileError::Unavailable),
            None => Err(FileError::Unavailable),
        }
    })
    .await
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
