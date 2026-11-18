//! The member's projections (§4.10 member column, §14.3 `MemberCase`).
//! These types have no field for anything staff-only — moderator, source,
//! attempts, codes, evidence authors, links, channels, or who voided or
//! decided — so none can leak by mistake.

use quack_core::action::{ActionKind, ActionState};
use quack_core::appeal::{AppealEventKind, AppealState, EligibilityFacts, eligibility};
use quack_core::ids::{AppealId, CaseId, EvidenceId, FileId, GuildId};
use quack_core::rule::FieldKind;
use quack_db::cases::MemberCaseSummary as Row;
use quack_engine::cases::MemberCaseView;
use quack_text::action::ActionView;
use quack_text::render::plain;
use serde::Serialize;
use time::OffsetDateTime;
use utoipa::ToSchema;

use crate::routes::cases::TimelineText;
use crate::wire::{icon_url, schema, ts};

/// The server a member's case is in.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct CaseGuild {
    #[schema(value_type = schema::Snowflake)]
    guild_id: GuildId,
    name: String,
    icon_url: Option<String>,
}

impl CaseGuild {
    pub(crate) fn new(guild_id: GuildId, name: String, icon: Option<&str>) -> Self {
        Self {
            icon_url: icon_url(guild_id.get(), icon),
            guild_id,
            name,
        }
    }
}

/// One of the member's own cases.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct MemberCaseSummary {
    #[schema(value_type = uuid::Uuid)]
    id: CaseId,
    number: u64,
    guild: CaseGuild,
    rule_name: String,
    /// The official reason.
    reason: String,
    #[serde(with = "ts")]
    #[schema(value_type = String, format = DateTime)]
    created_at: OffsetDateTime,
    #[serde(with = "ts::option")]
    #[schema(value_type = Option<String>, format = DateTime)]
    voided_at: Option<OffsetDateTime>,
    /// What happened, in the member's words.
    action_text: String,
    /// Whether the member may appeal now.
    can_appeal: bool,
    #[schema(value_type = Option<schema::AppealState>)]
    appeal_state: Option<AppealState>,
}

fn action_text(action: Option<(ActionKind, ActionState)>, number: u64) -> String {
    action.map_or_else(
        || plain(&quack_text::action::member_none()),
        |(kind, state)| {
            plain(&quack_text::action::member_sentence(&ActionView::new(
                kind, state, number,
            )))
        },
    )
}

impl From<Row> for MemberCaseSummary {
    fn from(r: Row) -> Self {
        Self {
            id: r.id,
            number: r.number,
            guild: CaseGuild::new(r.guild, r.guild_name, r.guild_icon_hash.as_deref()),
            rule_name: r
                .rule_name
                .unwrap_or_else(|| quack_text::word::IMPORTED.to_owned()),
            reason: r.reason.unwrap_or_default(),
            created_at: r.created_at,
            voided_at: r.voided_at,
            action_text: action_text(r.action, r.number),
            can_appeal: eligibility(&EligibilityFacts {
                is_member: true,
                case_valid: r.voided_at.is_none(),
                appealable: r.appealable,
                has_appeal: r.appeal_state.is_some(),
                source: r.source,
            })
            .is_ok(),
            appeal_state: r.appeal_state,
        }
    }
}

/// One answered context field.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ContextLine {
    label: String,
    #[schema(value_type = schema::FieldKind)]
    kind: FieldKind,
    /// The answer as text; message links show as a placeholder.
    value: String,
}

/// An embed's type and title.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct MemberEmbed {
    #[serde(rename = "type")]
    kind: String,
    title: Option<String>,
}

/// One file, served by the member file route.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct MemberFile {
    #[schema(value_type = uuid::Uuid)]
    id: FileId,
    filename: String,
    content_type: Option<String>,
    size_bytes: u64,
}

/// One saved message: text, embed types and titles, file names.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct MemberEvidence {
    #[schema(value_type = uuid::Uuid)]
    id: EvidenceId,
    content: Option<String>,
    embeds: Vec<MemberEmbed>,
    files: Vec<MemberFile>,
}

/// One appeal event, never naming staff.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct MemberAppealEvent {
    #[schema(value_type = schema::AppealEventKind)]
    kind: AppealEventKind,
    text: String,
    #[serde(with = "ts")]
    #[schema(value_type = String, format = DateTime)]
    at: OffsetDateTime,
    /// The member wrote it.
    by_you: bool,
}

/// The member's appeal.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct MemberAppeal {
    #[schema(value_type = uuid::Uuid)]
    id: AppealId,
    #[schema(value_type = schema::AppealState)]
    state: AppealState,
    /// Send back as `expected_version` with a reply.
    version: u32,
    statement: String,
    #[serde(with = "ts")]
    #[schema(value_type = String, format = DateTime)]
    created_at: OffsetDateTime,
    /// Whether the member may reply now.
    can_reply: bool,
    events: Vec<MemberAppealEvent>,
}

/// The member's own case.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct MemberCase {
    #[serde(flatten)]
    summary: MemberCaseSummary,
    void_reason: Option<String>,
    undo_text: Option<String>,
    #[serde(with = "ts::option")]
    #[schema(value_type = Option<String>, format = DateTime)]
    timeout_ends_at: Option<OffsetDateTime>,
    dm_text: String,
    context: Vec<ContextLine>,
    evidence: Vec<MemberEvidence>,
    /// Only when both cases are about this member.
    replaces_case_number: Option<u64>,
    replaced_by_case_number: Option<u64>,
    appeal: Option<MemberAppeal>,
    timeline: Vec<TimelineText>,
    timeline_truncated: bool,
}

impl MemberCase {
    pub(crate) fn new(v: MemberCaseView, icon_hash: Option<&str>) -> Self {
        Self {
            summary: MemberCaseSummary {
                id: v.id,
                number: v.number,
                guild: CaseGuild::new(v.guild_id, v.guild_name, icon_hash),
                rule_name: v.rule_name,
                reason: v.reason,
                created_at: v.created_at,
                voided_at: v.voided_at,
                action_text: v.action_text,
                can_appeal: v.can_appeal,
                appeal_state: v.appeal.as_ref().map(|a| a.state),
            },
            void_reason: v.void_reason,
            undo_text: v.undo_text,
            timeout_ends_at: v.timeout_ends_at,
            dm_text: v.dm_text,
            context: v
                .context
                .into_iter()
                .map(|c| ContextLine {
                    label: c.label,
                    kind: c.kind,
                    value: c.value,
                })
                .collect(),
            evidence: v
                .evidence
                .into_iter()
                .map(|e| MemberEvidence {
                    id: e.id,
                    content: e.content,
                    embeds: e
                        .embeds
                        .into_iter()
                        .map(|m| MemberEmbed {
                            kind: m.kind,
                            title: m.title,
                        })
                        .collect(),
                    files: e
                        .files
                        .into_iter()
                        .map(|f| MemberFile {
                            id: f.id,
                            filename: f.filename,
                            content_type: f.content_type,
                            size_bytes: f.size_bytes,
                        })
                        .collect(),
                })
                .collect(),
            replaces_case_number: v.replaces_case_number,
            replaced_by_case_number: v.replaced_by_case_number,
            appeal: v.appeal.map(|a| MemberAppeal {
                id: a.id,
                state: a.state,
                version: a.version,
                statement: a.statement,
                created_at: a.created_at,
                can_reply: a.can_reply,
                events: a
                    .events
                    .into_iter()
                    .map(|e| MemberAppealEvent {
                        kind: e.kind,
                        text: e.text,
                        at: e.at,
                        by_you: e.by_you,
                    })
                    .collect(),
            }),
            timeline: v
                .timeline
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
