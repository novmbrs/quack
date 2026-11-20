//! The parts of a staff case: its void, DM, evidence, appeal, and log
//! entries (§14.3 `Case`).

use quack_core::appeal::AppealState;
use quack_core::case::VoidCause;
use quack_core::dm::{DmFailure, DmState};
use quack_core::evidence::{EvidenceFailure, EvidenceState, FileCopyState};
use quack_core::ids::{AccountId, AppealId, ChannelId, EvidenceId, FileId, MessageId};
use quack_core::modlog::{ModLogKind, ModLogResult};
use quack_db::evidence::EmbedSummary;
use quack_engine::cases::EvidenceItem;
use serde::Serialize;
use time::OffsetDateTime;
use utoipa::ToSchema;

use crate::wire::{schema, ts};

/// How a case was voided.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct VoidInfo {
    #[schema(value_type = Option<schema::Snowflake>)]
    pub(crate) by: Option<AccountId>,
    pub(crate) reason: String,
    #[serde(with = "ts")]
    #[schema(value_type = String, format = DateTime)]
    pub(crate) at: OffsetDateTime,
    #[schema(value_type = schema::VoidCause)]
    pub(crate) cause: VoidCause,
}

/// The case DM; null on the case when the level sends none.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct Dm {
    #[schema(value_type = schema::DmState)]
    pub(crate) state: DmState,
    #[schema(value_type = Option<schema::DmFailure>)]
    pub(crate) failure: Option<DmFailure>,
    pub(crate) outcome_text: String,
    #[serde(with = "ts::option")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub(crate) settled_at: Option<OffsetDateTime>,
}

/// One file of a saved message.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct EvidenceFile {
    #[schema(value_type = uuid::Uuid)]
    pub(crate) id: FileId,
    pub(crate) filename: String,
    pub(crate) content_type: Option<String>,
    pub(crate) size_bytes: u64,
    #[schema(value_type = schema::FileCopyState)]
    pub(crate) copy_state: FileCopyState,
    pub(crate) state_text: String,
}

/// An embed of a saved message.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct Embed {
    #[serde(rename = "type")]
    pub(crate) kind: String,
    pub(crate) title: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) url: Option<String>,
}

impl From<EmbedSummary> for Embed {
    fn from(e: EmbedSummary) -> Self {
        Self {
            kind: e.kind,
            title: e.title,
            description: e.description,
            url: e.url,
        }
    }
}

/// One saved message, as staff see it.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct Evidence {
    #[schema(value_type = uuid::Uuid)]
    pub(crate) id: EvidenceId,
    #[schema(value_type = schema::EvidenceState)]
    pub(crate) state: EvidenceState,
    #[schema(value_type = Option<schema::EvidenceFailure>)]
    pub(crate) failure: Option<EvidenceFailure>,
    pub(crate) state_text: String,
    #[schema(value_type = schema::Snowflake)]
    pub(crate) channel_id: ChannelId,
    #[schema(value_type = schema::Snowflake)]
    pub(crate) message_id: MessageId,
    pub(crate) url: String,
    #[schema(value_type = Option<schema::Snowflake>)]
    pub(crate) author_id: Option<AccountId>,
    pub(crate) content: Option<String>,
    #[serde(with = "ts::option")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub(crate) message_created_at: Option<OffsetDateTime>,
    #[serde(with = "ts::option")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub(crate) message_edited_at: Option<OffsetDateTime>,
    pub(crate) embeds: Vec<Embed>,
    pub(crate) files: Vec<EvidenceFile>,
    pub(crate) files_not_saved: u32,
    #[schema(value_type = Option<schema::Snowflake>)]
    pub(crate) added_by: Option<AccountId>,
    #[serde(with = "ts")]
    #[schema(value_type = String, format = DateTime)]
    pub(crate) created_at: OffsetDateTime,
}

impl Evidence {
    /// A saved message that isn't removed, as this wire type describes it;
    /// `None` for uploaded files and removed items, which it can't
    /// describe (they are listed once the wire type grows them, §14.3).
    pub(crate) fn message(e: EvidenceItem) -> Option<Self> {
        if e.removed.is_some() {
            return None;
        }
        let (Some(channel_id), Some(message_id), Some(url)) = (e.channel_id, e.message_id, e.url)
        else {
            return None;
        };
        Some(Self {
            id: e.id,
            state: e.state,
            failure: e.failure,
            state_text: e.state_text,
            channel_id,
            message_id,
            url,
            author_id: e.author_id,
            content: e.content,
            message_created_at: e.message_created_at,
            message_edited_at: e.message_edited_at,
            embeds: e.embeds.into_iter().map(Into::into).collect(),
            files: e
                .files
                .into_iter()
                .map(|f| EvidenceFile {
                    id: f.id,
                    filename: f.filename,
                    content_type: f.content_type,
                    size_bytes: f.size_bytes,
                    copy_state: f.copy_state,
                    state_text: f.state_text,
                })
                .collect(),
            files_not_saved: e.files_not_saved,
            added_by: e.added_by,
            created_at: e.created_at,
        })
    }
}

/// The case's appeal, in brief.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct CaseAppeal {
    #[schema(value_type = uuid::Uuid)]
    pub(crate) id: AppealId,
    #[schema(value_type = schema::AppealState)]
    pub(crate) state: AppealState,
    pub(crate) version: u32,
    #[serde(with = "ts")]
    #[schema(value_type = String, format = DateTime)]
    pub(crate) created_at: OffsetDateTime,
}

/// One moderation log entry about the case.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct CaseLogEntry {
    #[schema(value_type = schema::ModLogKind)]
    pub(crate) kind: ModLogKind,
    #[schema(value_type = schema::ModLogResult)]
    pub(crate) result: ModLogResult,
    #[schema(value_type = Option<schema::Snowflake>)]
    pub(crate) actor_id: Option<AccountId>,
    pub(crate) text: String,
    #[serde(with = "ts")]
    #[schema(value_type = String, format = DateTime)]
    pub(crate) created_at: OffsetDateTime,
}
