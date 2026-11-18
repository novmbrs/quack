//! The member's own appeal from a case DM (§8.1): Appeal case (`ap`, also
//! the Go-era `appeal:submit:v1:` button) opens the statement modal, and
//! its submit (`apm`) files the appeal. These are member-self operations:
//! the engine checks the case is the clicker's own, and they work in DMs.

use quack_core::ids::CaseId;
use quack_discord::interaction::{FieldValue, Source};
use quack_engine::Origin;
use quack_engine::appeals::submit as submit_appeal;
use quack_text::actions::InputId;
use quack_text::appeal::{form, submitted};
use quack_text::error::{ErrorFacts, Scene};
use quack_text::{Msg, Tone};

use super::op_key;
use crate::ack::{Acked, Failure, Finish, Pending};
use crate::errors;
use crate::router::text_of;

/// Appeal case (`ap`): the statement modal (no stored values).
pub(crate) async fn open(p: Pending<'_>, case: CaseId) -> Finish {
    p.modal(&form(case)).await
}

/// The appeal modal (`apm`).
pub(crate) async fn submit(
    p: Pending<'_>,
    case: CaseId,
    fields: &[(InputId, FieldValue)],
    source: Source,
) -> Finish {
    let statement = text_of(fields, &InputId::Statement);
    let a = match p.defer_from(source).await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = file(&a, case, &statement).await;
    a.finish(result).await
}

async fn file(a: &Acked<'_>, case: CaseId, statement: &str) -> Result<Msg, Failure> {
    let member =
        a.ix.invoker
            .as_ref()
            .ok_or_else(crate::router::expired)?
            .account
            .id;
    let facts = ErrorFacts {
        scene: Scene::Member,
        ..ErrorFacts::default()
    };
    let done = submit_appeal(
        a.engine(),
        member,
        Origin::Discord,
        case,
        statement,
        &op_key(a.ix, "appeal_submit"),
    )
    .await
    .map_err(|e| errors::appeal(&e, &facts))?;
    let number = {
        let engine = a.engine();
        let mut conn = engine.db().conn().await.map_err(|e| errors::db(&e))?;
        quack_db::cases::get_for_member(&mut conn, member.member(), done.case)
            .await
            .map_err(|e| errors::db(&e))?
            .map_or(0, |c| c.number)
    };
    Ok(Msg::text(Tone::Success, submitted(number)))
}
