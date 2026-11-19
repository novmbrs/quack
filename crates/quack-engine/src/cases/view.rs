//! The staff case view (§4.10, §14.3 `Case`): every field, for staff with
//! `moderate`.

use quack_core::DomainError;
use quack_core::appeal::AppealState;
use quack_core::case::{CaseSource, ContextValues, VoidCause};
use quack_core::dm::{DmFailure, DmState};
use quack_core::ids::{AccountId, AppealId, CaseId, MemberId, RuleId};
use quack_core::modlog::{ModLogKind, ModLogResult};
use quack_core::permissions::Capability;
use quack_core::rule::Field;
use quack_text::render::plain;
use serde::Serialize;
use time::OffsetDateTime;

use super::records::CaseRecords;
use super::timeline::member_timeline;
use super::view_items::{ActionItem, EvidenceItem, action_item, evidence_item};
use super::{CaseError, CaseRef};
use crate::names::Names;
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
    pub level_index: Option<u16>,
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
    /// The display names the sentences used, for the caller's own
    /// sentences about the case.
    #[serde(skip)]
    pub names: Names,
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
        let mut records = CaseRecords::of(&mut conn, found).await?;
        let entries =
            quack_db::modlog::for_case(&mut conn, principal.guild, records.case.id, TIMELINE_LIMIT)
                .await?;
        let people = records
            .people()
            .chain(
                entries
                    .iter()
                    .flat_map(|e| [e.actor, e.links.member_id.map(MemberId::account)])
                    .flatten(),
            )
            .collect();
        records.names = Names::read(engine, people).await;
        let mut timeline = Vec::new();
        let (member_timeline, timeline_truncated) = member_timeline(&records, &entries);
        for entry in &entries {
            timeline.push(TimelineItem {
                kind: entry.kind,
                result: entry.result,
                actor_id: entry.actor,
                text: modlog::entry_text(engine, &mut conn, entry, &records.names).await?,
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

/// The staff case card (§11.7) for `moderate` + 2FA: the one card builder
/// the receipts' sync, `/case view`, and every "Open case" share. Reads
/// only the case's records (no moderation log).
pub async fn card(
    engine: &Engine,
    principal: &Principal,
    case: CaseRef,
) -> Result<quack_text::case::CaseCard, CaseError> {
    obs::use_case("get_case_card", Some(principal.guild), async {
        let mut conn = engine.db().conn().await?;
        principal.require(&mut conn, Capability::Moderate).await??;
        let found = case
            .find(&mut conn, principal.guild)
            .await?
            .ok_or(DomainError::CaseNotFound)?;
        let records = CaseRecords::of(&mut conn, found).await?;
        Ok(super::card::card(engine, &records))
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
    let member = r.names.person(case.member.account());
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
            outcome_text: plain(&quack_text::dm::sentence(
                r.dm_view().as_ref(),
                &member,
                case.number,
            )),
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
        names: r.names.clone(),
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
