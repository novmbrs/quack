//! Reading appeals as staff (§14.3 `Appeal`, `AppealSummary`).

use quack_core::DomainError;
use quack_core::appeal::{AppealEventKind, AppealState};
use quack_core::ids::{AccountId, AppealEventId, AppealId, CaseId, MemberId};
use quack_core::permissions::Capability;
use quack_db::{TimeCursor, appeals};
use serde::Serialize;
use time::OffsetDateTime;

use super::AppealError;
use crate::{Engine, Principal, obs};

/// An appeal as staff see it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AppealView {
    /// The appeal.
    pub id: AppealId,
    /// Its case.
    pub case_id: CaseId,
    /// The case's number.
    pub case_number: u64,
    /// The member.
    pub member_id: MemberId,
    /// The rule's name in the case's version.
    pub rule_name: String,
    /// Where it stands.
    pub state: AppealState,
    /// Its version (decisions name it).
    pub version: u32,
    /// The member's statement.
    pub statement: String,
    /// Who made the latest decision.
    pub decided_by: Option<AccountId>,
    /// When.
    #[serde(with = "time::serde::rfc3339::option")]
    pub decided_at: Option<OffsetDateTime>,
    /// Its reason.
    pub decision_reason: Option<String>,
    /// The timeline, oldest first (empty in lists).
    pub events: Vec<AppealEventView>,
    /// When it was submitted.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// When it last changed.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// One event on an appeal's timeline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AppealEventView {
    /// The event.
    pub id: AppealEventId,
    /// What happened.
    pub kind: AppealEventKind,
    /// Who; `None` = Quack.
    pub actor_id: Option<AccountId>,
    /// The statement, reply, or reason.
    pub text: String,
    /// Quack closed it because the case was voided.
    pub closed_by_void: bool,
    /// When.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// One appeal of the principal's server, with its timeline.
pub async fn get(
    engine: &Engine,
    principal: &Principal,
    appeal: AppealId,
) -> Result<AppealView, AppealError> {
    obs::use_case("get_appeal", Some(principal.guild), async {
        let mut conn = engine.db().conn().await?;
        principal.require(&mut conn, Capability::Moderate).await??;
        let guild = principal.guild;
        let found = appeals::get(&mut conn, guild, appeal)
            .await?
            .ok_or(DomainError::AppealNotFound)?;
        let case = quack_db::cases::get(&mut conn, guild, found.case)
            .await?
            .ok_or(DomainError::CaseNotFound)?;
        let rule_name = match case.rule {
            Some((rule, version)) => quack_db::rules::version(&mut conn, guild, rule, version)
                .await?
                .map_or_default(|v| v.definition.name),
            None => String::new(),
        };
        let events = appeals::events(&mut conn, guild, appeal).await?;
        let mut view = to_view(found, case.number, rule_name);
        view.events = events
            .into_iter()
            .map(|e| AppealEventView {
                id: e.id,
                kind: e.kind,
                actor_id: e.actor,
                text: e.text,
                closed_by_void: e.closed_by_void,
                created_at: e.created_at,
            })
            .collect();
        Ok(view)
    })
    .await
}

/// The principal's server's appeals (optionally in one state), newest
/// first, after `cursor`.
pub async fn list(
    engine: &Engine,
    principal: &Principal,
    state: Option<AppealState>,
    cursor: Option<TimeCursor>,
    limit: u32,
) -> Result<Vec<AppealView>, AppealError> {
    obs::use_case("list_appeals", Some(principal.guild), async {
        let mut conn = engine.db().conn().await?;
        principal.require(&mut conn, Capability::Moderate).await??;
        let rows = appeals::list(&mut conn, principal.guild, state, cursor, limit).await?;
        Ok(rows
            .into_iter()
            .map(|r| to_view(r.appeal, r.case_number, r.rule_name.unwrap_or_default()))
            .collect())
    })
    .await
}

fn to_view(a: quack_db::appeals::Appeal, case_number: u64, rule_name: String) -> AppealView {
    let (decided_by, decided_at, decision_reason) = match a.decision {
        Some((by, at, reason)) => (Some(by), Some(at), Some(reason)),
        None => (None, None, None),
    };
    AppealView {
        id: a.id,
        case_id: a.case,
        case_number,
        member_id: a.member,
        rule_name,
        state: a.state,
        version: a.version,
        statement: a.statement,
        decided_by,
        decided_at,
        decision_reason,
        events: Vec::new(),
        created_at: a.created_at,
        updated_at: a.updated_at,
    }
}
