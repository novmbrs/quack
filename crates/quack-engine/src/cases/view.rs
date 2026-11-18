//! The staff case view (§4.10, §14.3 `Case`): every field, for staff with
//! `moderate`.

use quack_core::DomainError;
use quack_core::action::{
    ActionFailure, ActionKind, ActionSkipReason, ActionState, AttemptOutcome,
};
use quack_core::appeal::AppealState;
use quack_core::case::{CaseSource, ContextValues, VoidCause};
use quack_core::dm::{DmFailure, DmState};
use quack_core::evidence::{EvidenceFailure, EvidenceState, FileCopyState};
use quack_core::ids::{
    AccountId, ActionId, AppealId, CaseId, ChannelId, EvidenceId, FileId, MemberId, MessageId,
    RuleId,
};
use quack_core::modlog::{ModLogKind, ModLogResult};
use quack_core::permissions::Capability;
use quack_core::rule::Field;
use quack_db::evidence::EmbedSummary;
use quack_text::render::plain;
use serde::Serialize;
use time::OffsetDateTime;

use super::records::CaseRecords;
use super::timeline::member_timeline;
use super::{CaseError, CaseRef};
use crate::{Engine, Principal, modlog, obs};

/// Most staff timeline entries a view carries.
const TIMELINE_LIMIT: u32 = 100;

/// A case as staff see it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CaseView {
    /// The case.
    pub id: CaseId,
    /// Its number.
    pub number: u64,
    /// The member.
    pub member_id: MemberId,
    /// The rule applied (none for imported cases).
    pub rule_id: Option<RuleId>,
    /// The rule's name in that version.
    pub rule_name: String,
    /// The version applied.
    pub rule_version: Option<u32>,
    /// The chosen level.
    pub level_index: Option<u8>,
    /// The case count including this case.
    pub case_count: Option<u32>,
    /// The level's start.
    pub level_from_case_count: Option<u32>,
    /// Where it came from.
    pub source: CaseSource,
    /// Who added it.
    pub moderator_id: Option<AccountId>,
    /// When it was added.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// The official reason.
    pub reason: String,
    /// Whether the version was appealable.
    pub appealable: bool,
    /// The version's context fields.
    pub fields: Vec<Field>,
    /// The answers.
    pub context: ContextValues,
    /// The context's revision (for editing).
    pub context_revision: u32,
    /// Set once voided.
    pub void: Option<VoidItem>,
    /// The case this one replaces.
    pub replaces_case_number: Option<u64>,
    /// The case that replaces this one.
    pub replaced_by_case_number: Option<u64>,
    /// The original action, then its undo.
    pub actions: Vec<ActionItem>,
    /// The case DM (`None`: the level sends none).
    pub dm: Option<DmItem>,
    /// Saved messages.
    pub evidence: Vec<EvidenceItem>,
    /// The appeal.
    pub appeal: Option<AppealItem>,
    /// What Quack v4 recorded, for imported cases.
    pub imported: Option<serde_json::Value>,
    /// Moderation log entries linked to the case, oldest first.
    pub timeline: Vec<TimelineItem>,
    /// The member timeline (§4.11).
    pub member_timeline: Vec<TimelineText>,
    /// There are more member timeline items than shown.
    pub timeline_truncated: bool,
}

/// How a case was voided.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct VoidItem {
    /// Who voided it.
    pub by: Option<AccountId>,
    /// Why.
    pub reason: String,
    /// When.
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    /// Staff void or accepted appeal.
    pub cause: VoidCause,
}

/// One action with its outcome sentence, attempts, and controls.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ActionItem {
    /// The action.
    pub id: ActionId,
    /// What it does.
    pub kind: ActionKind,
    /// The original it undoes.
    pub undo_of_action_id: Option<ActionId>,
    /// Where it stands.
    pub state: ActionState,
    /// The outcome sentence.
    pub outcome_text: String,
    /// A timeout's fixed end.
    #[serde(with = "time::serde::rfc3339::option")]
    pub timeout_ends_at: Option<OffsetDateTime>,
    /// A ban's message deletion window.
    pub delete_message_seconds: Option<u32>,
    /// Attempts in the current round.
    pub attempts: u32,
    /// When the next attempt is due.
    #[serde(with = "time::serde::rfc3339::option")]
    pub next_attempt_at: Option<OffsetDateTime>,
    /// Why it failed.
    pub failure_reason: Option<ActionFailure>,
    /// Discord's HTTP status for the failure.
    pub http_status: Option<u16>,
    /// Discord's error code for the failure.
    pub discord_code: Option<u32>,
    /// Why an undo was left in place.
    pub skip_reason: Option<ActionSkipReason>,
    /// The case that still applies.
    pub skipped_because_case_number: Option<u64>,
    /// Staff who requested it.
    pub requested_by: Option<AccountId>,
    /// When a failure was dismissed.
    #[serde(with = "time::serde::rfc3339::option")]
    pub dismissed_at: Option<OffsetDateTime>,
    /// Who dismissed it.
    pub dismissed_by: Option<AccountId>,
    /// When it was done.
    #[serde(with = "time::serde::rfc3339::option")]
    pub done_at: Option<OffsetDateTime>,
    /// When it was created.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// Every attempt.
    pub attempt_log: Vec<AttemptItem>,
    /// Retry is allowed now.
    pub can_retry: bool,
    /// Dismiss is allowed now.
    pub can_dismiss: bool,
    /// Undo is allowed now.
    pub can_undo: bool,
}

/// One attempt at an action.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AttemptItem {
    /// Its number.
    pub number: u32,
    /// When it started.
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    /// When it finished.
    #[serde(with = "time::serde::rfc3339::option")]
    pub finished_at: Option<OffsetDateTime>,
    /// How it ended.
    pub outcome: Option<AttemptOutcome>,
    /// Discord's HTTP status.
    pub http_status: Option<u16>,
    /// Discord's error code.
    pub discord_code: Option<u32>,
}

/// The case DM.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DmItem {
    /// Where it stands.
    pub state: DmState,
    /// Why it failed.
    pub failure: Option<DmFailure>,
    /// The outcome sentence.
    pub outcome_text: String,
    /// When it settled.
    #[serde(with = "time::serde::rfc3339::option")]
    pub settled_at: Option<OffsetDateTime>,
}

/// One saved message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EvidenceItem {
    /// The item.
    pub id: EvidenceId,
    /// Its state.
    pub state: EvidenceState,
    /// Why it failed.
    pub failure: Option<EvidenceFailure>,
    /// The state sentence.
    pub state_text: String,
    /// The message's channel.
    pub channel_id: ChannelId,
    /// The message.
    pub message_id: MessageId,
    /// The message's link.
    pub url: String,
    /// The message's author.
    pub author_id: Option<AccountId>,
    /// Its text.
    pub content: Option<String>,
    /// When it was sent.
    #[serde(with = "time::serde::rfc3339::option")]
    pub message_created_at: Option<OffsetDateTime>,
    /// When it was edited.
    #[serde(with = "time::serde::rfc3339::option")]
    pub message_edited_at: Option<OffsetDateTime>,
    /// Its embeds.
    pub embeds: Vec<EmbedSummary>,
    /// Its files.
    pub files: Vec<EvidenceFileItem>,
    /// Files over the caps.
    pub files_not_saved: u32,
    /// Who added it.
    pub added_by: Option<AccountId>,
    /// When.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// One file of a saved message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EvidenceFileItem {
    /// The file.
    pub id: FileId,
    /// Its name.
    pub filename: String,
    /// Its content type.
    pub content_type: Option<String>,
    /// Its size.
    pub size_bytes: u64,
    /// Its copy's state.
    pub copy_state: FileCopyState,
    /// The state sentence.
    pub state_text: String,
}

/// The case's appeal, in brief.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AppealItem {
    /// The appeal.
    pub id: AppealId,
    /// Its state.
    pub state: AppealState,
    /// Its version.
    pub version: u32,
    /// When it was submitted.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// One moderation log entry on the staff timeline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TimelineItem {
    /// Its kind.
    pub kind: ModLogKind,
    /// Its result.
    pub result: ModLogResult,
    /// Who acted.
    pub actor_id: Option<AccountId>,
    /// The sentence.
    pub text: String,
    /// When.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// One member timeline item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TimelineText {
    /// When.
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    /// The sentence (member-facing words, never naming staff).
    pub text: String,
}

/// A case of the principal's server, as staff see it (§4.10).
pub async fn get(
    engine: &Engine,
    principal: &Principal,
    case: CaseRef,
) -> Result<CaseView, CaseError> {
    obs::use_case("get_case", Some(principal.guild), async {
        let mut conn = engine.db().conn().await?;
        principal.require(&mut conn, Capability::Moderate).await??;
        let found = case
            .find(&mut conn, principal.guild)
            .await?
            .ok_or(DomainError::CaseNotFound)?;
        let records = CaseRecords::of(&mut conn, found).await?;
        let entries =
            quack_db::modlog::for_case(&mut conn, principal.guild, records.case.id, TIMELINE_LIMIT)
                .await?;
        let mut timeline = Vec::new();
        let (member_timeline, timeline_truncated) = member_timeline(&records, &entries);
        for entry in &entries {
            timeline.push(TimelineItem {
                kind: entry.entry.kind(),
                result: entry.result,
                actor_id: entry.actor,
                text: modlog::entry_text(engine, &mut conn, entry).await?,
                created_at: entry.created_at,
            });
        }
        Ok(view(
            &records,
            timeline,
            member_timeline,
            timeline_truncated,
        ))
    })
    .await
}

fn view(
    r: &CaseRecords,
    timeline: Vec<TimelineItem>,
    member_timeline: Vec<TimelineText>,
    timeline_truncated: bool,
) -> CaseView {
    let case = &r.case;
    let now = OffsetDateTime::now_utc();
    let member = quack_text::Person::unnamed(case.member.account());
    let d = case.decision;
    CaseView {
        id: case.id,
        number: case.number,
        member_id: case.member,
        rule_id: case.rule.map(|(id, _)| id),
        rule_name: r.rule_name(),
        rule_version: case.rule.map(|(_, v)| v),
        level_index: d.map(|d| d.level_index),
        case_count: d.map(|d| d.case_count),
        level_from_case_count: d.map(|d| d.level_from_case_count),
        source: case.source,
        moderator_id: case.moderator,
        created_at: case.created_at,
        reason: r.reason(),
        appealable: r.definition.as_ref().is_some_and(|d| d.appealable),
        fields: r
            .definition
            .as_ref()
            .map_or_default(|d| d.context_fields().into_owned()),
        context: case.context.clone(),
        context_revision: case.context_revision,
        void: case.void.as_ref().map(|v| VoidItem {
            by: v.by,
            reason: v.reason.clone(),
            at: v.at,
            cause: v.cause,
        }),
        replaces_case_number: r.replaces.map(|c| c.number),
        replaced_by_case_number: r.replaced_by.map(|c| c.number),
        actions: r
            .actions
            .iter()
            .map(|a| action_item(r, a, &member, now))
            .collect(),
        dm: r.dm.as_ref().map(|d| DmItem {
            state: d.state,
            failure: d.failure,
            outcome_text: plain(&quack_text::dm::sentence(r.dm_view().as_ref(), &member)),
            settled_at: d.settled_at,
        }),
        evidence: r.evidence.iter().map(|e| evidence_item(r, e)).collect(),
        appeal: r.appeal.as_ref().map(|a| AppealItem {
            id: a.id,
            state: a.state,
            version: a.version,
            created_at: a.created_at,
        }),
        imported: case.imported.clone(),
        timeline,
        member_timeline,
        timeline_truncated,
    }
}

fn action_item(
    r: &CaseRecords,
    a: &quack_db::actions::Action,
    member: &quack_text::Person,
    now: OffsetDateTime,
) -> ActionItem {
    let controls = r.controls(a, now);
    let view = r.action_view(a);
    ActionItem {
        id: a.id,
        kind: a.kind,
        undo_of_action_id: a.undo_of,
        state: a.state,
        outcome_text: plain(&quack_text::action::sentence(
            &view,
            member,
            quack_text::action::Detail::Full,
        )),
        timeout_ends_at: view.timeout_ends_at,
        delete_message_seconds: a.delete_message_seconds,
        attempts: a.attempts,
        next_attempt_at: a.next_attempt_at,
        failure_reason: a.failure.map(|f| f.reason),
        http_status: a.failure.and_then(|f| f.http_status),
        discord_code: a.failure.and_then(|f| f.discord_code),
        skip_reason: a.skip.map(|(s, _)| s),
        skipped_because_case_number: view.still_applies_case,
        requested_by: a.requested_by,
        dismissed_at: a.dismissed.map(|(at, _)| at),
        dismissed_by: a.dismissed.map(|(_, by)| by),
        done_at: a.done_at,
        created_at: a.created_at,
        attempt_log: r
            .attempts
            .iter()
            .filter(|t| t.action == a.id)
            .map(|t| AttemptItem {
                number: t.number,
                started_at: t.started_at,
                finished_at: t.finished_at,
                outcome: t.outcome,
                http_status: t.http_status,
                discord_code: t.discord_code,
            })
            .collect(),
        can_retry: controls.can_retry(),
        can_dismiss: controls.can_dismiss(),
        can_undo: controls.can_undo(),
    }
}

fn evidence_item(r: &CaseRecords, e: &quack_db::evidence::Evidence) -> EvidenceItem {
    EvidenceItem {
        id: e.id,
        state: e.state,
        failure: e.failure,
        state_text: plain(&quack_text::evidence::state_text(e.state, e.failure)),
        channel_id: e.channel,
        message_id: e.message,
        url: format!(
            "https://discord.com/channels/{}/{}/{}",
            e.guild, e.channel, e.message
        ),
        author_id: e.author,
        content: e.content.clone(),
        message_created_at: e.message_created_at,
        message_edited_at: e.message_edited_at,
        embeds: e.embeds.clone(),
        files: r
            .files
            .iter()
            .filter(|f| f.evidence == e.id)
            .map(|f| EvidenceFileItem {
                id: f.id,
                filename: f.filename.clone(),
                content_type: f.content_type.clone(),
                size_bytes: f.size_bytes,
                copy_state: f.copy_state,
                state_text: plain(&quack_text::evidence::file(&f.filename, f.copy_state)),
            })
            .collect(),
        files_not_saved: e.files_not_saved,
        added_by: e.added_by,
        created_at: e.created_at,
    }
}

/// A case of the principal's server by number, as staff see it.
pub async fn get_by_number(
    engine: &Engine,
    principal: &Principal,
    number: u64,
) -> Result<CaseView, CaseError> {
    get(engine, principal, CaseRef::Number(number)).await
}
