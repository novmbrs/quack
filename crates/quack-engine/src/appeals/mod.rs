//! Appeals (§8): a member's request to reconsider a case, and the staff
//! decisions on it. Every transition locks the case, then the appeal, and
//! re-checks the case's validity and the appeal's state and version there.

mod decide;
mod read;
mod submit;

use quack_core::DomainError;
use quack_core::appeal::{AppealEventKind, AppealRow};
use quack_core::ids::{AccountId, AppealEventId, AppealId, CaseId};
use quack_core::modlog::{
    AppealClosedDetail, CaseNumberDetail, ModLogEntry, ModLogLinks, ModLogSource,
};
use quack_db::appeals::{self, Appeal, AppealEvent};
use quack_db::cases::Case;
use quack_db::{DbError, Tx};
use time::OffsetDateTime;

use crate::modlog::{self, By};
use crate::opkey::KeyError;

pub use decide::{Decided, decide};
pub use read::{AppealEventView, AppealView, get, list};
pub use submit::{Replied, Submitted, add_information, submit};

/// Why an appeal operation was refused.
#[derive(Debug, thiserror::Error)]
pub enum AppealError {
    /// A refusal with a `quack-text` message (not found, changed, exists,
    /// not allowed, case voided, capability, 2FA).
    #[error("refused: {0:?}")]
    Refused(DomainError),
    /// The statement or reply is empty or longer than 4,000 characters.
    #[error("text length")]
    TextLength,
    /// The decision reason is empty or longer than 2,000 characters.
    #[error("reason length")]
    ReasonLength,
    /// The operation key was used for a different request.
    #[error("operation key reused")]
    OperationKeyReused,
    /// The database failed.
    #[error(transparent)]
    Db(#[from] DbError),
}

impl From<DomainError> for AppealError {
    fn from(error: DomainError) -> Self {
        Self::Refused(error)
    }
}

impl From<KeyError> for AppealError {
    fn from(error: KeyError) -> Self {
        match error {
            KeyError::Reused => Self::OperationKeyReused,
            KeyError::Db(db) => Self::Db(db),
        }
    }
}

/// The appeal as the transition table reads it, under its lock.
async fn row(tx: &mut Tx, appeal: &Appeal) -> Result<AppealRow, DbError> {
    let events = appeals::events(tx, appeal.guild, appeal.id).await?;
    let closed_by_void = events
        .iter()
        .rev()
        .find(|e| e.kind == AppealEventKind::Closed)
        .is_some_and(|e| e.closed_by_void);
    Ok(AppealRow {
        state: appeal.state,
        version: appeal.version,
        closed_by_void,
    })
}

/// Appends an event to the appeal's timeline.
async fn event(
    tx: &mut Tx,
    appeal: &Appeal,
    kind: AppealEventKind,
    actor: AccountId,
    text: &str,
) -> Result<AppealEventId, DbError> {
    let id = AppealEventId::from_uuid(uuid::Uuid::now_v7());
    appeals::insert_event(
        tx,
        appeal.guild,
        &AppealEvent {
            id,
            appeal: appeal.id,
            kind,
            actor: Some(actor),
            text: text.trim().to_owned(),
            closed_by_void: false,
            created_at: OffsetDateTime::now_utc(),
        },
    )
    .await?;
    Ok(id)
}

/// The log entry for an appeal event.
async fn log(
    tx: &mut Tx,
    case: &Case,
    appeal: AppealId,
    kind: AppealEventKind,
    actor: AccountId,
    source: ModLogSource,
) -> Result<(), DbError> {
    let detail = CaseNumberDetail {
        case_number: case.number,
    };
    let entry = match kind {
        AppealEventKind::Submitted => ModLogEntry::AppealSubmitted(detail),
        AppealEventKind::InformationRequested => ModLogEntry::AppealInformationRequested(detail),
        AppealEventKind::InformationProvided => ModLogEntry::AppealInformationProvided(detail),
        AppealEventKind::Accepted => ModLogEntry::AppealAccepted(detail),
        AppealEventKind::Rejected => ModLogEntry::AppealRejected(detail),
        AppealEventKind::Reopened => ModLogEntry::AppealReopened(detail),
        AppealEventKind::Closed => ModLogEntry::AppealClosed(AppealClosedDetail {
            case_number: case.number,
            because_case_voided: false,
        }),
    };
    modlog::write(
        tx,
        case.guild,
        entry,
        By {
            actor: Some(actor),
            source,
        },
        ModLogLinks {
            case_id: Some(case.id),
            member_id: Some(case.member),
            appeal_id: Some(appeal),
            ..ModLogLinks::default()
        },
    )
    .await?;
    Ok(())
}

/// The ids a stored appeal result refers to.
#[derive(serde::Serialize, serde::Deserialize)]
struct Stored {
    appeal_id: AppealId,
    case_id: CaseId,
}
