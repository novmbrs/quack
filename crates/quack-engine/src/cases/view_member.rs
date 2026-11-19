//! The member's own case (§4.10 member column, §14.3 `MemberCase`). A
//! separate type from the staff view, with no field for anything staff
//! only (moderator, source, attempts and codes, evidence authors, links,
//! channels, who voided or decided), so none can leak by mistake.

use quack_core::DomainError;
use quack_core::appeal::{AppealEventKind, AppealState, EligibilityFacts, eligibility};
use quack_core::ids::{AccountId, AppealId, CaseId, EvidenceId, FileId, GuildId};
use quack_core::rule::FieldKind;
use quack_db::{cases, guilds};
use quack_text::render::plain;
use quack_text::{Actor, Person};
use serde::Serialize;
use time::OffsetDateTime;

use super::CaseError;
use super::records::CaseRecords;
use super::timeline::member_timeline;
use super::view::TimelineText;
use crate::dms::context_lines;
use crate::{Engine, obs};

/// A case as its member sees it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MemberCaseView {
    /// The case.
    pub id: CaseId,
    /// Its number in the server.
    pub number: u64,
    /// The server.
    pub guild_id: GuildId,
    /// The server's name.
    pub guild_name: String,
    /// The rule's name.
    pub rule_name: String,
    /// The official reason.
    pub reason: String,
    /// When it was added.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// When it was voided.
    #[serde(with = "time::serde::rfc3339::option")]
    pub voided_at: Option<OffsetDateTime>,
    /// Why it was voided.
    pub void_reason: Option<String>,
    /// The decided action's outcome, in member words.
    pub action_text: String,
    /// The undo's outcome, in member words.
    pub undo_text: Option<String>,
    /// A timeout's fixed end.
    #[serde(with = "time::serde::rfc3339::option")]
    pub timeout_ends_at: Option<OffsetDateTime>,
    /// The case DM's outcome, in member words.
    pub dm_text: String,
    /// The case context; message links show as a placeholder.
    pub context: Vec<MemberContextLine>,
    /// Saved messages: text, embed types and titles, file names.
    pub evidence: Vec<MemberEvidence>,
    /// The case this one replaces, when both are about this member.
    pub replaces_case_number: Option<u64>,
    /// The case that replaces this one, when both are about this member.
    pub replaced_by_case_number: Option<u64>,
    /// Whether the member may appeal now.
    pub can_appeal: bool,
    /// The appeal.
    pub appeal: Option<MemberAppeal>,
    /// The member timeline.
    pub timeline: Vec<TimelineText>,
    /// There are more timeline items than shown.
    pub timeline_truncated: bool,
}

/// One answered context field.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MemberContextLine {
    /// The field's label.
    pub label: String,
    /// The field's kind.
    pub kind: FieldKind,
    /// The answer as text (message links as `member.context.link_saved`).
    pub value: String,
}

/// One saved message, without author, link, or channel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MemberEvidence {
    /// The item.
    pub id: EvidenceId,
    /// Its text.
    pub content: Option<String>,
    /// Its embeds' types and titles.
    pub embeds: Vec<MemberEmbed>,
    /// Its files.
    pub files: Vec<MemberEvidenceFile>,
}

/// An embed's type and title.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MemberEmbed {
    /// The embed type.
    #[serde(rename = "type")]
    pub kind: String,
    /// Its title.
    pub title: Option<String>,
}

/// One file, served through the member file route.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MemberEvidenceFile {
    /// The file.
    pub id: FileId,
    /// Its name.
    pub filename: String,
    /// Its content type.
    pub content_type: Option<String>,
    /// Its size.
    pub size_bytes: u64,
}

/// The member's appeal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MemberAppeal {
    /// The appeal.
    pub id: AppealId,
    /// Its state.
    pub state: AppealState,
    /// Its version (sent back with a reply).
    pub version: u32,
    /// The statement.
    pub statement: String,
    /// When it was submitted.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// Whether the member may reply now.
    pub can_reply: bool,
    /// Its events, oldest first, never naming staff.
    pub events: Vec<MemberAppealEvent>,
}

/// One appeal event as the member sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MemberAppealEvent {
    /// What happened.
    pub kind: AppealEventKind,
    /// Its sentence, then the reason or reply.
    pub text: String,
    /// When.
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    /// The member wrote it.
    pub by_you: bool,
    /// Quack closed the appeal because its case was voided (a `closed`
    /// event only).
    pub closed_by_void: bool,
}

/// The member's own case. Someone else's case and a missing case are both
/// `case_not_found` (§4.10).
pub async fn member_case(
    engine: &Engine,
    member: AccountId,
    case: CaseId,
) -> Result<MemberCaseView, CaseError> {
    obs::use_case("member_case", None, async {
        let mut conn = engine.db().conn().await?;
        let found = cases::get_for_member(&mut conn, member.member(), case)
            .await?
            .ok_or(DomainError::CaseNotFound)?;
        let guild = guilds::get(&mut conn, found.guild).await?;
        let log = quack_db::modlog::for_case(&mut conn, found.guild, found.id, 1000).await?;
        let records = CaseRecords::of(&mut conn, found).await?;
        Ok(project(&records, guild.map_or_default(|g| g.name), &log))
    })
    .await
}

pub(crate) fn project(
    r: &CaseRecords,
    guild_name: String,
    log: &[quack_db::modlog::StoredEntry],
) -> MemberCaseView {
    let case = &r.case;
    let mine = |c: &quack_db::cases::CaseRef| (c.member == case.member).then_some(c.number);
    let action_text = r.original().map_or_else(
        || plain(&quack_text::action::member_none()),
        |a| plain(&quack_text::action::member_sentence(&r.action_view(a))),
    );
    let (timeline, timeline_truncated) = member_timeline(r, log);
    MemberCaseView {
        id: case.id,
        number: case.number,
        guild_id: case.guild,
        guild_name,
        rule_name: r.rule_name(),
        reason: r.reason(),
        created_at: case.created_at,
        voided_at: case.void.as_ref().map(|v| v.at),
        void_reason: case.void.as_ref().map(|v| v.reason.clone()),
        action_text,
        undo_text: r
            .undo()
            .map(|u| plain(&quack_text::action::member_sentence(&r.action_view(u)))),
        timeout_ends_at: r.original().and_then(|a| a.timeout_ends_at),
        dm_text: plain(&quack_text::dm::member_sentence(r.dm_view().as_ref())),
        context: context(r),
        evidence: evidence(r),
        replaces_case_number: r.replaces.as_ref().and_then(mine),
        replaced_by_case_number: r.replaced_by.as_ref().and_then(mine),
        can_appeal: eligibility(&EligibilityFacts {
            is_member: true,
            case_valid: case.is_valid(),
            appealable: r.definition.as_ref().is_some_and(|d| d.appealable),
            has_appeal: r.appeal.is_some(),
            source: case.source,
        })
        .is_ok(),
        appeal: appeal(r),
        timeline,
        timeline_truncated,
    }
}

fn context(r: &CaseRecords) -> Vec<MemberContextLine> {
    let Some(def) = &r.definition else {
        return Vec::new();
    };
    context_lines(def, &r.case.context)
        .into_iter()
        .map(|line| MemberContextLine {
            label: line.label.clone(),
            kind: line.kind,
            value: if line.is_link() {
                plain(&quack_text::case::member_link_saved())
            } else {
                plain(&quack_text::case::context_value(&line, usize::MAX))
            },
        })
        .collect()
}

fn evidence(r: &CaseRecords) -> Vec<MemberEvidence> {
    r.evidence
        .iter()
        .map(|e| MemberEvidence {
            id: e.id,
            content: e.content.clone(),
            embeds: e
                .embeds
                .iter()
                .map(|m| MemberEmbed {
                    kind: m.kind.clone(),
                    title: m.title.clone(),
                })
                .collect(),
            files: r
                .files
                .iter()
                .filter(|f| f.evidence == e.id)
                .map(|f| MemberEvidenceFile {
                    id: f.id,
                    filename: f.filename.clone(),
                    content_type: f.content_type.clone(),
                    size_bytes: f.size_bytes,
                })
                .collect(),
        })
        .collect()
}

fn appeal(r: &CaseRecords) -> Option<MemberAppeal> {
    let a = r.appeal.as_ref()?;
    let you = Person::unnamed(a.member.account());
    Some(MemberAppeal {
        id: a.id,
        state: a.state,
        version: a.version,
        statement: a.statement.clone(),
        created_at: a.created_at,
        can_reply: a.state == AppealState::NeedsInformation && r.case.is_valid(),
        events: r
            .appeal_events
            .iter()
            .map(|e| {
                let by_you = e.actor == Some(a.member.account());
                let actor = match (e.kind, e.closed_by_void) {
                    (AppealEventKind::Closed, true) => Some(Actor::Quack),
                    _ => e.actor.map(|x| Actor::Person(Person::unnamed(x))),
                };
                let lead = plain(&quack_text::appeal::event(
                    e.kind,
                    actor.as_ref(),
                    &you,
                    true,
                ));
                MemberAppealEvent {
                    kind: e.kind,
                    text: format!("{lead}\n{}", e.text),
                    at: e.created_at,
                    by_you,
                    closed_by_void: e.kind == AppealEventKind::Closed && e.closed_by_void,
                }
            })
            .collect(),
    })
}
