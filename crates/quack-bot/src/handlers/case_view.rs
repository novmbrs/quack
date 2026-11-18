//! Showing a case: `/case view`, Open case (`cv`), the case select
//! (`cvs`), and Go-era retry/dismiss buttons that now only open the case.

use quack_core::DomainError;
use quack_core::ids::{ActionId, CaseId};
use quack_discord::interaction::{SelectChoice, Source};
use quack_engine::CaseRef;
use quack_engine::cases::get;
use quack_text::Msg;
use quack_text::error::ErrorFacts;

use crate::ack::{Acked, Failure, Finish, Pending};
use crate::{auth, card, errors};

/// The staff card for `case`, after authorizing the clicker (§11.7).
pub(crate) async fn card_for(a: &Acked<'_>, case: CaseRef) -> Result<Msg, Failure> {
    let engine = a.engine();
    let principal = auth::principal(engine, a.ix).await?;
    let number = match case {
        CaseRef::Number(n) => Some(n),
        CaseRef::Id(_) => None,
    };
    let view = get(engine, &principal, case).await.map_err(|e| {
        errors::case(
            &e,
            &ErrorFacts {
                case_number: number,
                ..ErrorFacts::default()
            },
        )
    })?;
    Ok(card::msg(&view, principal.guild, engine.links()))
}

/// `/case view number`.
pub(crate) async fn command(p: Pending<'_>, number: u64) -> Finish {
    let a = match p.defer().await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = card_for(&a, CaseRef::Number(number)).await;
    a.finish(result).await
}

/// Open case (`cv`): privately, from any message.
pub(crate) async fn open(p: Pending<'_>, case: CaseId, source: Source) -> Finish {
    let a = match p.defer_from(source).await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = card_for(&a, CaseRef::Id(case)).await;
    a.finish(result).await
}

/// The "Open a case" select (`cvs`) on lists and histories.
pub(crate) async fn picked(p: Pending<'_>, values: &[SelectChoice], source: Source) -> Finish {
    let case = values.iter().find_map(|v| match v {
        SelectChoice::Case(c) => Some(*c),
        _ => None,
    });
    let Some(case) = case else {
        return p.fail(crate::router::expired()).await;
    };
    open(p, case, source).await
}

/// A Go-era Retry or Dismiss button: open the mapped action's case (view
/// only, §11.5). An action of another server names nothing here.
pub(crate) async fn of_action(p: Pending<'_>, action: ActionId, source: Source) -> Finish {
    let a = match p.defer_from(source).await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = case_of_action(&a, action).await;
    a.finish(result).await
}

async fn case_of_action(a: &Acked<'_>, action: ActionId) -> Result<Msg, Failure> {
    let guild = auth::guild_of(a.ix)?;
    let found = {
        let mut conn = a.engine().db().conn().await.map_err(|e| errors::db(&e))?;
        quack_db::actions::get(&mut conn, guild, action)
            .await
            .map_err(|e| errors::db(&e))?
    };
    let Some(found) = found else {
        // Authorize first so an outsider learns nothing from the answer.
        let principal = auth::principal(a.engine(), a.ix).await?;
        auth::require(
            a.engine(),
            &principal,
            quack_core::permissions::Capability::Moderate,
        )
        .await?;
        return Err(legacy());
    };
    card_for(a, CaseRef::Id(found.case)).await
}

/// `interaction.legacy_button`: a Go-era id that names nothing here.
fn legacy() -> Failure {
    Failure::refused(
        quack_text::error::legacy_button(),
        quack_telemetry::labels::Reason::Invalid,
    )
}

/// A Go-era case button (`case:view:v1:…` and friends): Open case, with a
/// case that isn't in this server answered as a legacy button (§11.5).
pub(crate) async fn open_legacy(p: Pending<'_>, case: CaseId, source: Source) -> Finish {
    let a = match p.defer_from(source).await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = match card_for(&a, CaseRef::Id(case)).await {
        Err(f) if is_not_found(&f) => Err(legacy()),
        other => other,
    };
    a.finish(result).await
}

/// Whether a failure is "that case wasn't found".
fn is_not_found(f: &Failure) -> bool {
    f.text == errors::domain(DomainError::CaseNotFound, &ErrorFacts::default()).text
}
