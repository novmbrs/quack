//! Staff appeal review in Discord (§8, §11.5): `/appeals` and its pager
//! (`al`), Open appeal (`ao`), and Accept / Reject (`aa`/`aj` open the
//! reason modal, `aam`/`ajm` decide). Other decisions are made on the
//! dashboard.

use quack_core::appeal::{AppealDecision, AppealEventKind, AppealState};
use quack_core::ids::AppealId;
use quack_core::permissions::Capability;
use quack_discord::interaction::{FieldValue, Source};
use quack_engine::appeals::{AppealView, decide as decide_appeal, get, list};
use quack_text::actions::InputId;
use quack_text::appeal::{AppealPost, AppealRow, ModalDecision, decided, decision_modal};
use quack_text::error::ErrorFacts;
use quack_text::{Actor, Msg, Person, Tone};

use super::op_key;
use crate::ack::{Acked, Failure, Finish, Pending};
use crate::auth::{self, payload_capabilities};
use crate::errors;
use crate::router::text_of;

/// Appeals per page.
const PER_PAGE: usize = 10;
/// Most pending appeals listed in Discord.
const LIST_CAP: u32 = 500;

/// `/appeals`: pending appeals, oldest first.
pub(crate) async fn command(p: Pending<'_>) -> Finish {
    let a = match p.defer().await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = list_page(&a, 1).await;
    a.finish(result).await
}

/// The `/appeals` pager (`al`).
pub(crate) async fn page(p: Pending<'_>, page: u32, source: Source) -> Finish {
    let a = match p.defer_from(source).await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = list_page(&a, page).await;
    a.finish(result).await
}

async fn list_page(a: &Acked<'_>, page: u32) -> Result<Msg, Failure> {
    let engine = a.engine();
    let principal = auth::principal(engine, a.ix).await?;
    let mut appeals = list(
        engine,
        &principal,
        Some(AppealState::Pending),
        None,
        LIST_CAP,
    )
    .await
    .map_err(|e| errors::appeal(&e, &ErrorFacts::default()))?
    .rows;
    appeals.sort_by_key(|v| v.created_at);
    let pages = u32::try_from(appeals.len().div_ceil(PER_PAGE).max(1)).unwrap_or(u32::MAX);
    let page = page.clamp(1, pages);
    let rows: Vec<AppealRow> = appeals
        .iter()
        .skip(usize::try_from(page - 1).unwrap_or(0) * PER_PAGE)
        .take(PER_PAGE)
        .map(|v| AppealRow {
            appeal: v.id,
            number: v.case_number,
            member: Person::unnamed(v.member_id.account()),
            rule: v.rule_name.clone(),
            submitted_at: v.created_at,
        })
        .collect();
    Ok(quack_text::appeal::list(&rows, page, pages))
}

/// The appeal card: the appeal post's content with Accept and Reject while
/// it is pending.
fn card(v: &AppealView, dashboard_url: String) -> Msg {
    let closed_by_void = v
        .events
        .iter()
        .rev()
        .find(|e| e.kind == AppealEventKind::Closed)
        .is_some_and(|e| e.closed_by_void);
    let information = v
        .events
        .iter()
        .rev()
        .find(|e| e.kind == AppealEventKind::InformationProvided)
        .map(|e| e.text.clone());
    quack_text::appeal::post(&AppealPost {
        appeal: v.id,
        version: v.version,
        number: v.case_number,
        rule: v.rule_name.clone(),
        member: Person::unnamed(v.member_id.account()),
        submitted_at: v.created_at,
        state: v.state,
        decided: v
            .decided_by
            .zip(v.decided_at)
            .map(|(by, at)| (Actor::Person(Person::unnamed(by)), at)),
        closed_by_void,
        statement: v.statement.clone(),
        information,
        decision_reason: v.decision_reason.clone(),
        dashboard_url,
    })
}

/// Open appeal (`ao`, also Go-era accept/reject buttons).
pub(crate) async fn open(p: Pending<'_>, appeal: AppealId, source: Source) -> Finish {
    let a = match p.defer_from(source).await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = show(&a, appeal).await;
    a.finish(result).await
}

async fn show(a: &Acked<'_>, appeal: AppealId) -> Result<Msg, Failure> {
    let engine = a.engine();
    let principal = auth::principal(engine, a.ix).await?;
    let v = get(engine, &principal, appeal)
        .await
        .map_err(|e| errors::appeal(&e, &ErrorFacts::default()))?;
    Ok(card(&v, engine.links().appeal(principal.guild, appeal)))
}

/// Accept / Reject (`aa`/`aj`): the reason modal with its default text.
pub(crate) async fn open_decision(
    p: Pending<'_>,
    decision: ModalDecision,
    appeal: AppealId,
    version: u32,
) -> Finish {
    if !payload_capabilities(p.ix, p.config().as_deref()).moderate {
        return p.fail(auth::missing(Capability::Moderate)).await;
    }
    p.modal(&decision_modal(decision, appeal, version)).await
}

/// The accept or reject modal (`aam`/`ajm`).
pub(crate) async fn decide(
    p: Pending<'_>,
    decision: ModalDecision,
    appeal: AppealId,
    version: u32,
    fields: &[(InputId, FieldValue)],
    source: Source,
) -> Finish {
    let reason = text_of(fields, &InputId::Reason);
    let a = match p.defer_from(source).await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = decide_now(&a, decision, appeal, version, &reason).await;
    a.finish(result).await
}

async fn decide_now(
    a: &Acked<'_>,
    decision: ModalDecision,
    appeal: AppealId,
    version: u32,
    reason: &str,
) -> Result<Msg, Failure> {
    let engine = a.engine();
    let principal = auth::principal(engine, a.ix).await?;
    let (kind, operation) = match decision {
        ModalDecision::Accept => (AppealDecision::Accept, "appeal_accept"),
        ModalDecision::Reject => (AppealDecision::Reject, "appeal_reject"),
    };
    decide_appeal(
        engine,
        &principal,
        appeal,
        kind,
        reason,
        version,
        &op_key(a.ix, operation),
    )
    .await
    .map_err(|e| errors::appeal(&e, &ErrorFacts::default()))?;
    let v = get(engine, &principal, appeal)
        .await
        .map_err(|e| errors::appeal(&e, &ErrorFacts::default()))?;
    let member = Person::unnamed(v.member_id.account());
    Ok(Msg::text(
        Tone::Success,
        decided(decision, v.case_number, &member),
    ))
}
