//! The action and evidence items of the staff case view.

use quack_core::action::{
    ActionFailure, ActionKind, ActionSkipReason, ActionState, AttemptOutcome,
};
use quack_core::evidence::{EvidenceFailure, EvidenceKind, EvidenceState, FileCopyState};
use quack_core::ids::{AccountId, ActionId, ChannelId, EvidenceId, FileId, MessageId};
use quack_db::evidence::EmbedSummary;
use quack_text::render::plain;
use serde::Serialize;
use time::OffsetDateTime;

use super::records::CaseRecords;

/// One action with its outcome sentence, attempts, and controls.
#[allow(
    clippy::struct_excessive_bools,
    reason = "the three controls and no_change are independent facts"
)]
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
    /// Who dismissed it (`None` when no one did, or imported history
    /// didn't record who).
    pub dismissed_by: Option<AccountId>,
    /// It is done because the desired state already held (an "already
    /// timed out" timeout, an "already gone" kick).
    pub no_change: bool,
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

/// One evidence item as staff see it: a saved message or uploaded files.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EvidenceItem {
    /// The item.
    pub id: EvidenceId,
    /// A saved message or uploaded files.
    pub kind: EvidenceKind,
    /// Its state.
    pub state: EvidenceState,
    /// Why it failed.
    pub failure: Option<EvidenceFailure>,
    /// The state sentence.
    pub state_text: String,
    /// The message's channel (`None` for uploaded files).
    pub channel_id: Option<ChannelId>,
    /// The message (`None` for uploaded files).
    pub message_id: Option<MessageId>,
    /// The message's link (`None` for uploaded files).
    pub url: Option<String>,
    /// Whether the member sees it on their case page (always for a saved
    /// message; uploads start staff-only).
    pub visible_to_member: bool,
    /// The staff note.
    pub note: Option<String>,
    /// The note's revision, sent back with an edit.
    pub note_revision: u32,
    /// Who last changed the note.
    pub note_updated_by: Option<AccountId>,
    /// When the note last changed.
    #[serde(with = "time::serde::rfc3339::option")]
    pub note_updated_at: Option<OffsetDateTime>,
    /// Set once staff removed it (shown collapsed; hidden everywhere else).
    pub removed: Option<RemovedItem>,
    /// Messages around a staff-added message, oldest first; `None` when
    /// none were saved.
    pub surrounding: Option<Vec<SurroundingItem>>,
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

/// Who removed an item, and when.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RemovedItem {
    /// The staff member.
    pub by: AccountId,
    /// When.
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    /// `evidence.removed_by`, naming them.
    pub text: String,
}

/// One message saved around a linked message (text only).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SurroundingItem {
    /// The message.
    pub message_id: MessageId,
    /// Before the linked message (else after).
    pub before: bool,
    /// Its author.
    pub author_id: AccountId,
    /// Its text (at most 500 characters).
    pub content: Option<String>,
    /// When it was sent.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// How many files it had (never copied).
    pub files: u32,
}

/// One file of an evidence item.
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

pub(super) fn action_item(
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
        no_change: a.no_change,
        dismissed_at: a.dismissed.map(|(at, _)| at),
        dismissed_by: a.dismissed.and_then(|(_, by)| by),
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

pub(super) fn evidence_item(r: &CaseRecords, e: &quack_db::evidence::Evidence) -> EvidenceItem {
    EvidenceItem {
        id: e.id,
        kind: e.kind,
        state: e.state,
        failure: e.failure,
        state_text: plain(&quack_text::evidence::state_text(e.state, e.failure)),
        channel_id: e.channel,
        message_id: e.message,
        url: e
            .source()
            .map(|(c, m)| format!("https://discord.com/channels/{}/{c}/{m}", e.guild)),
        visible_to_member: e.visible_to_member,
        note: e.note.clone(),
        note_revision: e.note_revision,
        note_updated_by: e.note_updated.and_then(|(_, by)| by),
        note_updated_at: e.note_updated.map(|(at, _)| at),
        removed: e.removed.map(|(at, by)| RemovedItem {
            by,
            at,
            text: plain(&quack_text::evidence::removed_by(&r.names.person(by), at)),
        }),
        surrounding: e.surrounding.as_ref().map(|list| {
            list.iter()
                .map(|m| SurroundingItem {
                    message_id: m.message_id,
                    before: m.before,
                    author_id: m.author_id,
                    content: m.content.clone(),
                    created_at: m.created_at,
                    files: m.files,
                })
                .collect()
        }),
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
                state_text: plain(&quack_text::evidence::file(
                    &f.filename,
                    e.kind,
                    f.copy_state,
                )),
            })
            .collect(),
        files_not_saved: e.files_not_saved,
        added_by: e.added_by,
        created_at: e.created_at,
    }
}
