//! `/rules` and its pager (`rl`): the active rules with a dashboard link
//! each (§5.1 of the charter: rules are edited on the dashboard only).

use quack_core::permissions::Capability;
use quack_discord::interaction::Source;
use quack_text::Msg;
use quack_text::rules::{RuleRow, list};

use crate::ack::{Acked, Failure, Finish, Pending};
use crate::{auth, errors};

/// Rules per page.
const PER_PAGE: usize = 10;

/// `/rules`.
pub(crate) async fn command(p: Pending<'_>) -> Finish {
    let a = match p.defer().await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = show(&a, 1).await;
    a.finish(result).await
}

/// The `/rules` pager (`rl`).
pub(crate) async fn page(p: Pending<'_>, page: u32, source: Source) -> Finish {
    let a = match p.defer_from(source).await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = show(&a, page).await;
    a.finish(result).await
}

/// `moderate` or `manage_rules` (with 2FA) may list rules (§11.1).
async fn show(a: &Acked<'_>, page: u32) -> Result<Msg, Failure> {
    let engine = a.engine();
    let principal = auth::principal(engine, a.ix).await?;
    let caps = {
        let mut conn = engine.db().conn().await.map_err(|e| errors::db(&e))?;
        principal
            .current_capabilities(&mut conn)
            .await
            .map_err(|e| errors::db(&e))?
    };
    if caps.check(Capability::ManageRules).is_err()
        && let Err(refused) = caps.check(Capability::Moderate)
    {
        return Err(errors::domain(
            refused.into(),
            &quack_text::error::ErrorFacts::default(),
        ));
    }
    let rules = engine
        .rules_cache()
        .get(engine.db(), principal.guild)
        .await
        .map_err(|e| errors::db(&e))?;
    let mut active = rules.active.clone();
    active.sort_by_key(|r| r.definition.name.to_lowercase());
    let pages = u32::try_from(active.len().div_ceil(PER_PAGE).max(1)).unwrap_or(u32::MAX);
    let page = page.clamp(1, pages);
    let rows: Vec<RuleRow> = active
        .iter()
        .skip(usize::try_from(page - 1).unwrap_or(0) * PER_PAGE)
        .take(PER_PAGE)
        .map(|r| RuleRow {
            name: r.definition.name.clone(),
            levels: r
                .definition
                .levels
                .iter()
                .map(|l| (l.from_case_count, l.action))
                .collect(),
            url: engine.links().rule(principal.guild, r.id),
        })
        .collect();
    let archived = u64::try_from(rules.archived.len()).unwrap_or(u64::MAX);
    Ok(list(&rows, archived, page, pages))
}
