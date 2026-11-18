//! Why a ticket operation was refused or failed.

use quack_core::DomainError;
use quack_db::DbError;
use quack_text::Text;

/// Why a ticket operation was refused or failed.
#[derive(Debug, thiserror::Error)]
pub enum TicketError {
    /// No such ticket in this server.
    #[error("ticket not found")]
    NotFound,
    /// Only the ticket's member or a moderator may close it
    /// (`ticket.close.not_allowed`).
    #[error("not allowed to close")]
    NotAllowed,
    /// The transcript expired or was never saved (`file_unavailable`).
    #[error("transcript unavailable")]
    TranscriptUnavailable,
    /// A domain refusal with its own message (capability, 2FA,
    /// `ticket_not_open`).
    #[error("refused: {0:?}")]
    Refused(DomainError),
    /// Discord didn't answer; nothing changed.
    #[error("Discord unavailable")]
    DiscordUnavailable,
    /// The database failed.
    #[error(transparent)]
    Db(#[from] DbError),
}

impl From<DomainError> for TicketError {
    fn from(error: DomainError) -> Self {
        Self::Refused(error)
    }
}

impl TicketError {
    /// The ticket-specific reply, when there is one; other refusals use the
    /// shared `quack-text` messages for their [`DomainError`].
    pub fn text(&self) -> Option<Text> {
        match self {
            Self::NotAllowed => Some(quack_text::ticket::close_not_allowed()),
            Self::Refused(DomainError::TicketNotOpen) => Some(quack_text::ticket::close_not_open()),
            _ => None,
        }
    }
}
