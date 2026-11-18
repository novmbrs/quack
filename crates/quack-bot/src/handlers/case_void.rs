//! Voiding a case (§4.7): `/case void number reason`, Void case (`vd`)
//! and its modal (`vdm`).

use quack_core::action::{ActionKind, ActionState};
use quack_core::appeal::AppealState;
use quack_core::ids::CaseId;
use quack_core::permissions::Capability;
use quack_discord::interaction::{FieldValue, Source};
use quack_engine::CaseRef;
use quack_engine::cases::{CaseView, VoidEffects, get, void_case};
use quack_text::actions::InputId;
use quack_text::case::{VoidResult, void_done, void_modal};
use quack_text::error::ErrorFacts;
use quack_text::{Msg, Person, Tone};

use super::op_key;
use crate::ack::{Acked, Failure, Finish, Pending};
use crate::auth::{self, payload_capabilities};
use crate::errors;
use crate::router::text_of;

/// `/case void number reason`.
pub(crate) async fn command(p: Pending<'_>, number: u64, reason: &str) -> Finish {
    let a = match p.defer().await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = void(&a, CaseRef::Number(number), reason).await;
    a.finish(result).await
}

/// Void case (`vd`): the reason modal (no stored values). Someone the
/// payload shows without `moderate` is refused instead.
pub(crate) async fn open(p: Pending<'_>, case: CaseId) -> Finish {
    if !payload_capabilities(p.ix, p.config().as_deref()).moderate {
        return p.fail(auth::missing(Capability::Moderate)).await;
    }
    p.modal(&void_modal(case, None)).await
}

/// The void modal (`vdm`).
pub(crate) async fn submit(
    p: Pending<'_>,
    case: CaseId,
    fields: &[(InputId, FieldValue)],
    source: Source,
) -> Finish {
    let reason = text_of(fields, &InputId::Reason);
    let a = match p.defer_from(source).await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = void(&a, CaseRef::Id(case), &reason).await;
    a.finish(result).await
}

async fn void(a: &Acked<'_>, case: CaseRef, reason: &str) -> Result<Msg, Failure> {
    let engine = a.engine();
    let principal = auth::principal(engine, a.ix).await?;
    let number = match case {
        CaseRef::Number(n) => Some(n),
        CaseRef::Id(_) => None,
    };
    let voided = void_case(engine, &principal, case, reason, &op_key(a.ix, "case_void"))
        .await
        .map_err(|e| {
            if matches!(e, quack_engine::cases::CaseError::ReasonLength) {
                return errors::reason(reason, quack_core::limits::VOID_REASON.max);
            }
            errors::case(
                &e,
                &ErrorFacts {
                    case_number: number,
                    ..ErrorFacts::default()
                },
            )
        })?;
    let member = Person::unnamed(voided.member.account());
    let (results, appeal_closed) = if let Some(effects) = &voided.effects {
        (from_effects(effects), effects.closed_appeal.is_some())
    } else {
        // A replay: what the first void did, read back from the case.
        let view = get(engine, &principal, CaseRef::Id(voided.case))
            .await
            .map_err(|e| errors::case(&e, &ErrorFacts::default()))?;
        let closed = view
            .appeal
            .as_ref()
            .is_some_and(|ap| ap.state == AppealState::Closed);
        (results(&view), closed)
    };
    Ok(Msg::text(
        Tone::Success,
        void_done(
            voided.number,
            &voided.rule_name,
            &member,
            &results,
            appeal_closed,
        ),
    ))
}

/// What the void did to the case's action, from the engine's answer.
fn from_effects(effects: &VoidEffects) -> Vec<VoidResult> {
    let mut out = Vec::new();
    if let Some(kind) = effects.cancelled_kind {
        out.push(VoidResult::Cancelled(kind));
    } else if let Some((kind, state)) = effects.original {
        match (kind, state) {
            (ActionKind::Kick, ActionState::Running) => out.push(VoidResult::RunningKick),
            (ActionKind::Kick, ActionState::Done) => out.push(VoidResult::DoneKick),
            (kind, ActionState::Running) => out.push(VoidResult::Running(kind)),
            _ => {}
        }
    }
    for kind in &effects.undo_kinds {
        out.push(match kind {
            ActionKind::Unban => VoidResult::Unbanning,
            _ => VoidResult::RemovingTimeout,
        });
    }
    out
}

/// What the void did to the case's action, read back from the case.
fn results(view: &CaseView) -> Vec<VoidResult> {
    let mut out = Vec::new();
    if let Some(original) = view.actions.iter().find(|a| a.undo_of_action_id.is_none()) {
        match (original.kind, original.state) {
            (kind, ActionState::Cancelled) => out.push(VoidResult::Cancelled(kind)),
            (ActionKind::Kick, ActionState::Running) => out.push(VoidResult::RunningKick),
            (ActionKind::Kick, ActionState::Done) => out.push(VoidResult::DoneKick),
            (kind, ActionState::Running) => out.push(VoidResult::Running(kind)),
            _ => {}
        }
    }
    for undo in view
        .actions
        .iter()
        .filter(|a| a.undo_of_action_id.is_some())
    {
        if matches!(undo.state, ActionState::Pending | ActionState::Running) {
            out.push(match undo.kind {
                ActionKind::Unban => VoidResult::Unbanning,
                _ => VoidResult::RemovingTimeout,
            });
        }
    }
    out
}
