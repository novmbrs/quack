//! Case lists: `/case list [member] [rule]` and its pager (`cl`),
//! `/case history member` and its pager (`hi`). Ten cases per page, newest
//! first, each list with an "Open a case" select.

use quack_core::ids::{GuildId, MemberId, RuleId};
use quack_core::permissions::Capability;
use quack_db::cases::{CaseFilter, CaseSummary};
use quack_discord::interaction::{Person, RuleArg, Source};
use quack_engine::Principal;
use quack_engine::cases::{list, member_summary};
use quack_text::Msg;
use quack_text::case::{CaseRow, HistoryTotals, ListFilter, ListPage, history};
use quack_text::error::ErrorFacts;

use super::person;
use crate::ack::{Acked, Failure, Finish, Pending};
use crate::{auth, errors};

/// Cases per page.
const PER_PAGE: u32 = 10;
/// Most cases a list pages through in Discord (50 pages); the dashboard
/// shows the rest.
const LIST_CAP: u32 = 500;

fn row(s: &CaseSummary) -> CaseRow {
    CaseRow {
        case: s.id,
        number: s.number,
        member: quack_text::Person::unnamed(s.member.account()),
        rule: s.rule_name.clone().unwrap_or_default(),
        created_at: s.created_at,
        voided: s.voided_at.is_some(),
        imported: s.source == quack_core::case::CaseSource::Imported,
    }
}

/// Page `page` of up to [`LIST_CAP`] matching cases.
async fn cases_page(
    a: &Acked<'_>,
    principal: &Principal,
    filter: &CaseFilter,
    page: u32,
) -> Result<ListPage, Failure> {
    let all = list(a.engine(), principal, filter, None, LIST_CAP)
        .await
        .map_err(|e| errors::case(&e, &ErrorFacts::default()))?
        .rows;
    let pages = u32::try_from(all.len())
        .unwrap_or(u32::MAX)
        .div_ceil(PER_PAGE)
        .max(1);
    let page = page.clamp(1, pages);
    let rows = all
        .iter()
        .skip(usize::try_from((page - 1) * PER_PAGE).unwrap_or(0))
        .take(PER_PAGE as usize)
        .map(row)
        .collect();
    Ok(ListPage { rows, page, pages })
}

/// The rule a `rule` filter names, active or archived, with its name.
async fn rule_filter(
    a: &Acked<'_>,
    guild: GuildId,
    rule: &RuleArg,
) -> Result<Option<(RuleId, String)>, Failure> {
    let engine = a.engine();
    let rules = engine
        .rules_cache()
        .get(engine.db(), guild)
        .await
        .map_err(|e| errors::db(&e))?;
    let named = |id: RuleId| {
        rules
            .active(id)
            .map(|r| r.definition.name.clone())
            .or_else(|| {
                rules
                    .archived
                    .iter()
                    .find(|r| r.id == id)
                    .map(|r| r.name.clone())
            })
    };
    Ok(match rule {
        RuleArg::Id(id) => named(*id).map(|name| (*id, name)),
        RuleArg::Name(name) => rules
            .active_by_name(name)
            .map(|r| (r.id, r.definition.name.clone()))
            .or_else(|| {
                rules
                    .archived
                    .iter()
                    .find(|r| r.name.eq_ignore_ascii_case(name))
                    .map(|r| (r.id, r.name.clone()))
            }),
    })
}

/// `/case list [member] [rule]`.
pub(crate) async fn command(
    p: Pending<'_>,
    member: Option<&Person>,
    rule: Option<&RuleArg>,
) -> Finish {
    let a = match p.defer().await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = list_command(&a, member, rule).await;
    a.finish(result).await
}

async fn list_command(
    a: &Acked<'_>,
    member: Option<&Person>,
    rule: Option<&RuleArg>,
) -> Result<Msg, Failure> {
    let principal = auth::principal(a.engine(), a.ix).await?;
    // Before the rule lookup, so a refusal never tells which names exist.
    auth::require(a.engine(), &principal, Capability::Moderate).await?;
    let rule = match rule {
        Some(arg) => match rule_filter(a, principal.guild, arg).await? {
            Some(found) => Some(found),
            None => {
                return Err(errors::domain(
                    quack_core::DomainError::RuleNotFound,
                    &ErrorFacts::default(),
                ));
            }
        },
        None => None,
    };
    let filter = ListFilter {
        member: member.map(|m| (m.account.id.member(), person(m))),
        rule,
    };
    show_list(a, &principal, &filter, 1).await
}

async fn show_list(
    a: &Acked<'_>,
    principal: &Principal,
    filter: &ListFilter,
    page: u32,
) -> Result<Msg, Failure> {
    let query = CaseFilter {
        member: filter.member.as_ref().map(|(id, _)| *id),
        rule: filter.rule.as_ref().map(|(id, _)| *id),
        ..CaseFilter::default()
    };
    let page = cases_page(a, principal, &query, page).await?;
    Ok(quack_text::case::list(filter, &page))
}

/// The `/case list` pager (`cl`).
pub(crate) async fn page(
    p: Pending<'_>,
    page: u32,
    member: Option<MemberId>,
    rule: Option<RuleId>,
    source: Source,
) -> Finish {
    let a = match p.defer_from(source).await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = list_page(&a, page, member, rule).await;
    a.finish(result).await
}

async fn list_page(
    a: &Acked<'_>,
    page: u32,
    member: Option<MemberId>,
    rule: Option<RuleId>,
) -> Result<Msg, Failure> {
    let principal = auth::principal(a.engine(), a.ix).await?;
    // Before the rule lookup, so a refusal never tells which names exist.
    auth::require(a.engine(), &principal, Capability::Moderate).await?;
    let rule = match rule {
        Some(id) => rule_filter(a, principal.guild, &RuleArg::Id(id)).await?,
        None => None,
    };
    let filter = ListFilter {
        member: member.map(|m| (m, quack_text::Person::unnamed(m.account()))),
        rule,
    };
    show_list(a, &principal, &filter, page).await
}

/// `/case history member`.
pub(crate) async fn history_command(p: Pending<'_>, member: &Person) -> Finish {
    let a = match p.defer().await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = show_history(&a, member.account.id.member(), &person(member), 1).await;
    a.finish(result).await
}

/// Member's case history and its pager (`hi`).
pub(crate) async fn history_page(
    p: Pending<'_>,
    member: MemberId,
    page: u32,
    source: Source,
) -> Finish {
    let a = match p.defer_from(source).await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let shown = quack_text::Person::unnamed(member.account());
    let result = show_history(&a, member, &shown, page).await;
    a.finish(result).await
}

async fn show_history(
    a: &Acked<'_>,
    member: MemberId,
    shown: &quack_text::Person,
    page: u32,
) -> Result<Msg, Failure> {
    let engine = a.engine();
    let principal = auth::principal(engine, a.ix).await?;
    let tallies = member_summary(engine, &principal, member)
        .await
        .map_err(|e| errors::case(&e, &ErrorFacts::default()))?;
    let mut totals = HistoryTotals::default();
    for t in &tallies {
        totals.valid += t.valid;
        totals.voided += t.voided;
    }
    totals.total = totals.valid + totals.voided;
    let filter = CaseFilter {
        member: Some(member),
        ..CaseFilter::default()
    };
    let page = cases_page(a, &principal, &filter, page).await?;
    Ok(history(member, shown, totals, &page))
}
