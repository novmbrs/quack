//! The staff case wire types (§14.3 `CaseSummary`, `Case`, `CasePreview`),
//! built from the engine's staff views. Member types live in `me`.

use quack_core::action::{ActionKind, ActionState};
use quack_core::appeal::AppealState;
use quack_core::case::{CaseSource, ContextValues};
use quack_core::dm::DmState;
use quack_core::ids::{AccountId, CaseId, MemberId, RuleId};
use quack_core::rule::{Field, Level};
use quack_engine::cases::{CasePreview as PreviewView, CaseView};
use quack_text::action::ActionView;
use quack_text::word;
use serde::Serialize;
use time::OffsetDateTime;
use utoipa::ToSchema;

use super::items::{CaseAppeal, CaseLogEntry, Dm, Evidence, VoidInfo};
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
            // The view's own outcome_text is the full sentence; a summary
            // carries the outcome word, as the case list does.
            action: original.map(|a| ActionBrief::new(a.kind, a.state, v.number)),
            dm_state: v.dm.as_ref().map_or(DmState::NotNeeded, |d| d.state),
            appeal_state: v.appeal.as_ref().map(|a| a.state),
        }
    }
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
    /// What Quack v4 recorded, for an imported case.
    imported: Option<ImportedCase>,
    /// Moderation log entries about the case, oldest first.
    timeline: Vec<CaseLogEntry>,
    member_timeline: Vec<TimelineText>,
    timeline_truncated: bool,
}

/// What Quack v4 recorded about an imported case (`cases.imported`).
#[expect(
    clippy::struct_field_names,
    reason = "the stored object's keys, `v4_*`, are the wire names"
)]
#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct ImportedCase {
    /// v4's action type.
    v4_action: Option<String>,
    /// The v4 moderator's display name.
    v4_moderator: Option<String>,
    /// v4's reason text.
    v4_reason: Option<String>,
    /// v4's id for the case.
    v4_source_id: Option<String>,
}

impl ImportedCase {
    /// Reads the stored object; a missing or non-text value is null.
    fn from_json(v: &serde_json::Value) -> Self {
        let text = |key: &str| match v.get(key) {
            Some(serde_json::Value::String(s)) => Some(s.clone()),
            Some(serde_json::Value::Number(n)) => Some(n.to_string()),
            _ => None,
        };
        Self {
            v4_action: text("v4_action"),
            v4_moderator: text("v4_moderator"),
            v4_reason: text("v4_reason"),
            v4_source_id: text("v4_source_id"),
        }
    }
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
            evidence: v
                .evidence
                .into_iter()
                .filter_map(Evidence::message)
                .collect(),
            appeal: v.appeal.map(|a| CaseAppeal {
                id: a.id,
                state: a.state,
                version: a.version,
                created_at: a.created_at,
            }),
            imported: v.imported.as_ref().map(ImportedCase::from_json),
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

/// Route 23's answer: the voided case and what the void did, in the
/// sentence the bot answers with (`case.void_done`).
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct CaseVoided {
    /// The case after the void.
    pub case: Case,
    /// What the void did, as a sentence.
    pub message: String,
}

/// Route 25's answer: the case and what adding the links did, in the
/// sentence the bot answers with (`evidence.added`).
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct EvidenceAdded {
    /// The case after the links were added.
    pub case: Case,
    /// New messages Quack is saving.
    pub new: u32,
    /// Messages whose earlier save failed, being saved again.
    pub retrying: u32,
    /// Messages that were already saved (or being saved) for the case.
    pub already_saved: u32,
    /// What adding the links did, as a sentence.
    pub message: String,
}
