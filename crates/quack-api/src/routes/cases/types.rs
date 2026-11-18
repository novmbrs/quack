//! The staff case wire types (§14.3 `CaseSummary`, `Case`, `CasePreview`),
//! built from the engine's staff views. Member types live in `me`.

use quack_core::action::{ActionKind, ActionState};
use quack_core::appeal::AppealState;
use quack_core::case::{CaseSource, ContextValues, VoidCause};
use quack_core::dm::{DmFailure, DmState};
use quack_core::evidence::{EvidenceFailure, EvidenceState, FileCopyState};
use quack_core::ids::{
    AccountId, AppealId, CaseId, ChannelId, EvidenceId, FileId, MemberId, MessageId, RuleId,
};
use quack_core::modlog::{ModLogKind, ModLogResult};
use quack_core::rule::{Field, Level};
use quack_db::evidence::EmbedSummary;
use quack_engine::cases::{CasePreview as PreviewView, CaseView, EvidenceItem};
use quack_text::action::ActionView;
use quack_text::word;
use serde::Serialize;
use time::OffsetDateTime;
use utoipa::ToSchema;

use crate::routes::actions::Action;
use crate::wire::{schema, ts};

/// A case's original action, in brief.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct ActionBrief {
    #[schema(value_type = schema::ActionKind)]
    kind: ActionKind,
    #[schema(value_type = schema::ActionState)]
    state: ActionState,
    /// The outcome in a word ("Done", "Failed", …).
    outcome_text: String,
}

impl ActionBrief {
    fn new(kind: ActionKind, state: ActionState, number: u64) -> Self {
        let view = ActionView::new(kind, state, number);
        Self {
            kind,
            state,
            outcome_text: word::outcome(quack_text::action::outcome(&view)).to_owned(),
        }
    }
}

/// One case in a list.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct CaseSummary {
    #[schema(value_type = uuid::Uuid)]
    pub id: CaseId,
    pub number: u64,
    #[schema(value_type = schema::Snowflake)]
    pub member_id: MemberId,
    #[schema(value_type = Option<uuid::Uuid>)]
    pub rule_id: Option<RuleId>,
    /// The rule's name in the case's version ("Imported" without one).
    pub rule_name: String,
    pub rule_version: Option<u32>,
    pub level_index: Option<u16>,
    #[schema(value_type = schema::CaseSource)]
    pub source: CaseSource,
    #[schema(value_type = Option<schema::Snowflake>)]
    pub moderator_id: Option<AccountId>,
    #[serde(with = "ts")]
    #[schema(value_type = String, format = DateTime)]
    pub created_at: OffsetDateTime,
    #[serde(with = "ts::option")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub voided_at: Option<OffsetDateTime>,
    pub action: Option<ActionBrief>,
    /// The case DM's state; `not_needed` when the level sends none.
    #[schema(value_type = schema::DmState)]
    pub dm_state: DmState,
    #[schema(value_type = Option<schema::AppealState>)]
    pub appeal_state: Option<AppealState>,
}

impl From<quack_db::cases::CaseSummary> for CaseSummary {
    fn from(c: quack_db::cases::CaseSummary) -> Self {
        Self {
            id: c.id,
            number: c.number,
            member_id: c.member,
            rule_id: c.rule.map(|(id, _)| id),
            rule_name: c.rule_name.unwrap_or_else(|| word::IMPORTED.to_owned()),
            rule_version: c.rule.map(|(_, v)| v),
            level_index: c.level_index,
            source: c.source,
            moderator_id: c.moderator,
            created_at: c.created_at,
            voided_at: c.voided_at,
            action: c.action.map(|(k, s)| ActionBrief::new(k, s, c.number)),
            dm_state: c.dm_state.unwrap_or(DmState::NotNeeded),
            appeal_state: c.appeal_state,
        }
    }
}

impl CaseSummary {
    /// The summary of a full staff view.
    pub(crate) fn from_view(v: &CaseView) -> Self {
        let original = v.actions.iter().find(|a| a.undo_of_action_id.is_none());
        Self {
            id: v.id,
            number: v.number,
            member_id: v.member_id,
            rule_id: v.rule_id,
            rule_name: v.rule_name.clone(),
            rule_version: v.rule_version,
            level_index: v.level_index,
            source: v.source,
            moderator_id: v.moderator_id,
            created_at: v.created_at,
            voided_at: v.void.as_ref().map(|x| x.at),
            action: original.map(|a| ActionBrief {
                kind: a.kind,
                state: a.state,
                outcome_text: a.outcome_text.clone(),
            }),
            dm_state: v.dm.as_ref().map_or(DmState::NotNeeded, |d| d.state),
            appeal_state: v.appeal.as_ref().map(|a| a.state),
        }
    }
}

/// How a case was voided.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct VoidInfo {
    #[schema(value_type = Option<schema::Snowflake>)]
    by: Option<AccountId>,
    reason: String,
    #[serde(with = "ts")]
    #[schema(value_type = String, format = DateTime)]
    at: OffsetDateTime,
    #[schema(value_type = schema::VoidCause)]
    cause: VoidCause,
}

/// The case DM; null on the case when the level sends none.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct Dm {
    #[schema(value_type = schema::DmState)]
    state: DmState,
    #[schema(value_type = Option<schema::DmFailure>)]
    failure: Option<DmFailure>,
    outcome_text: String,
    #[serde(with = "ts::option")]
    #[schema(value_type = Option<String>, format = DateTime)]
    settled_at: Option<OffsetDateTime>,
}

/// One file of a saved message.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct EvidenceFile {
    #[schema(value_type = uuid::Uuid)]
    id: FileId,
    filename: String,
    content_type: Option<String>,
    size_bytes: u64,
    #[schema(value_type = schema::FileCopyState)]
    copy_state: FileCopyState,
    state_text: String,
}

/// An embed of a saved message.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct Embed {
    #[serde(rename = "type")]
    kind: String,
    title: Option<String>,
    description: Option<String>,
    url: Option<String>,
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
    id: EvidenceId,
    #[schema(value_type = schema::EvidenceState)]
    state: EvidenceState,
    #[schema(value_type = Option<schema::EvidenceFailure>)]
    failure: Option<EvidenceFailure>,
    state_text: String,
    #[schema(value_type = schema::Snowflake)]
    channel_id: ChannelId,
    #[schema(value_type = schema::Snowflake)]
    message_id: MessageId,
    url: String,
    #[schema(value_type = Option<schema::Snowflake>)]
    author_id: Option<AccountId>,
    content: Option<String>,
    #[serde(with = "ts::option")]
    #[schema(value_type = Option<String>, format = DateTime)]
    message_created_at: Option<OffsetDateTime>,
    #[serde(with = "ts::option")]
    #[schema(value_type = Option<String>, format = DateTime)]
    message_edited_at: Option<OffsetDateTime>,
    embeds: Vec<Embed>,
    files: Vec<EvidenceFile>,
    files_not_saved: u32,
    #[schema(value_type = Option<schema::Snowflake>)]
    added_by: Option<AccountId>,
    #[serde(with = "ts")]
    #[schema(value_type = String, format = DateTime)]
    created_at: OffsetDateTime,
}

impl From<EvidenceItem> for Evidence {
    fn from(e: EvidenceItem) -> Self {
        Self {
            id: e.id,
            state: e.state,
            failure: e.failure,
            state_text: e.state_text,
            channel_id: e.channel_id,
            message_id: e.message_id,
            url: e.url,
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
        }
    }
}

/// The case's appeal, in brief.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct CaseAppeal {
    #[schema(value_type = uuid::Uuid)]
    id: AppealId,
    #[schema(value_type = schema::AppealState)]
    state: AppealState,
    version: u32,
    #[serde(with = "ts")]
    #[schema(value_type = String, format = DateTime)]
    created_at: OffsetDateTime,
}

/// One moderation log entry about the case.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct CaseLogEntry {
    #[schema(value_type = schema::ModLogKind)]
    kind: ModLogKind,
    #[schema(value_type = schema::ModLogResult)]
    result: ModLogResult,
    #[schema(value_type = Option<schema::Snowflake>)]
    actor_id: Option<AccountId>,
    text: String,
    #[serde(with = "ts")]
    #[schema(value_type = String, format = DateTime)]
    created_at: OffsetDateTime,
}

/// One item of the member timeline.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct TimelineText {
    #[serde(with = "ts")]
    #[schema(value_type = String, format = DateTime)]
    pub at: OffsetDateTime,
    pub text: String,
}

/// A case as staff see it.
#[expect(
    clippy::struct_field_names,
    reason = "`case_count` is the glossary term"
)]
#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct Case {
    #[serde(flatten)]
    summary: CaseSummary,
    case_count: Option<u32>,
    level_from_case_count: Option<u32>,
    reason: String,
    appealable: bool,
    #[schema(value_type = Vec<schema::Field>)]
    fields: Vec<Field>,
    #[schema(value_type = std::collections::HashMap<String, schema::ContextValue>)]
    context: ContextValues,
    context_revision: u32,
    void: Option<VoidInfo>,
    replaces_case_number: Option<u64>,
    replaced_by_case_number: Option<u64>,
    actions: Vec<Action>,
    dm: Option<Dm>,
    evidence: Vec<Evidence>,
    appeal: Option<CaseAppeal>,
    #[schema(value_type = Option<Object>)]
    imported: Option<serde_json::Value>,
    /// Moderation log entries about the case, oldest first.
    timeline: Vec<CaseLogEntry>,
    member_timeline: Vec<TimelineText>,
    timeline_truncated: bool,
}

impl From<CaseView> for Case {
    fn from(v: CaseView) -> Self {
        let summary = CaseSummary::from_view(&v);
        Self {
            summary,
            case_count: v.case_count,
            level_from_case_count: v.level_from_case_count,
            reason: v.reason,
            appealable: v.appealable,
            fields: v.fields,
            context: v.context,
            context_revision: v.context_revision,
            void: v.void.map(|x| VoidInfo {
                by: x.by,
                reason: x.reason,
                at: x.at,
                cause: x.cause,
            }),
            replaces_case_number: v.replaces_case_number,
            replaced_by_case_number: v.replaced_by_case_number,
            actions: v
                .actions
                .into_iter()
                .map(|a| Action::from_item(v.id, v.number, a))
                .collect(),
            dm: v.dm.map(|d| Dm {
                state: d.state,
                failure: d.failure,
                outcome_text: d.outcome_text,
                settled_at: d.settled_at,
            }),
            evidence: v.evidence.into_iter().map(Into::into).collect(),
            appeal: v.appeal.map(|a| CaseAppeal {
                id: a.id,
                state: a.state,
                version: a.version,
                created_at: a.created_at,
            }),
            imported: v.imported,
            timeline: v
                .timeline
                .into_iter()
                .map(|t| CaseLogEntry {
                    kind: t.kind,
                    result: t.result,
                    actor_id: t.actor_id,
                    text: t.text,
                    created_at: t.created_at,
                })
                .collect(),
            member_timeline: v
                .member_timeline
                .into_iter()
                .map(|t| TimelineText {
                    at: t.at,
                    text: t.text,
                })
                .collect(),
            timeline_truncated: v.timeline_truncated,
        }
    }
}

/// What applying a rule would choose now (route 58).
#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct CasePreview {
    /// Send back as `rule_version` when adding the case.
    rule_version: u32,
    counted_cases: u32,
    case_count: u32,
    /// The level (1-based).
    level_index: u16,
    #[schema(value_type = schema::Level)]
    level: Level,
    counting_window_days: Option<u16>,
    #[serde(with = "ts")]
    #[schema(value_type = String, format = DateTime)]
    at: OffsetDateTime,
}

impl From<PreviewView> for CasePreview {
    fn from(p: PreviewView) -> Self {
        Self {
            rule_version: p.rule_version,
            counted_cases: p.counted_cases,
            case_count: p.case_count,
            level_index: p.level_index,
            level: p.level,
            counting_window_days: p.counting_window_days,
            at: p.at,
        }
    }
}
