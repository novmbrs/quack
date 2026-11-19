//! Staff controls on a case's actions (§5.8): Retry (`ar`), Dismiss
//! (`ad`), and Remove timeout / Unban (`au` asks, `auc` does it). Each is
//! re-authorized and re-checked by the engine inside the write's
//! transaction.

use quack_core::ids::ActionId;
use quack_core::permissions::Capability;
use quack_discord::interaction::Source;
use quack_engine::Principal;
use quack_engine::actions::{self, ActionError, ControlDone};
use quack_telemetry::labels::Reason;
use quack_text::action::{
    Undo, dismiss_done, retry_done, undo_button, undo_confirm, undo_from_original, undo_queued,
};
use quack_text::actions::ButtonAction;
use quack_text::error::ErrorFacts;
use quack_text::model::{Button, ButtonStyle};
use quack_text::{Msg, Person, Tone};

use super::op_key;
use crate::ack::{Acked, Failure, Finish, Pending};
use crate::{auth, errors};

/// The action's own facts, read after authorizing the clicker.
struct Target {
    principal: Principal,
    action: quack_db::actions::Action,
    case_number: u64,
}

async fn target(a: &Acked<'_>, action: ActionId) -> Result<Target, Failure> {
    let engine = a.engine();
    let principal = auth::principal(engine, a.ix).await?;
    auth::require(engine, &principal, Capability::Moderate).await?;
    let mut conn = engine.db().conn().await.map_err(|e| errors::db(&e))?;
    let found = quack_db::actions::get(&mut conn, principal.guild, action)
        .await
        .map_err(|e| errors::db(&e))?
        .ok_or_else(|| errors::action(&ActionError::NotFound, &ErrorFacts::default()))?;
    let case_number = quack_db::cases::get(&mut conn, principal.guild, found.case)
        .await
        .map_err(|e| errors::db(&e))?
        .map_or(0, |c| c.number);
    Ok(Target {
        principal,
        action: found,
        case_number,
    })
}

fn failed(t: &Target, member: &Person, e: &ActionError) -> Failure {
    errors::action(
        e,
        &ErrorFacts {
            case_number: Some(t.case_number),
            action: Some(t.action.kind),
            member: Some(member),
            ..ErrorFacts::default()
        },
    )
}

async fn control(p: Pending<'_>, source: Source, work: Control, action: ActionId) -> Finish {
    let a = match p.defer_from(source).await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = run(&a, work, action).await;
    a.finish(result).await
}

#[derive(Clone, Copy)]
enum Control {
    Retry,
    Dismiss,
    Ask,
    Undo,
}

async fn run(a: &Acked<'_>, work: Control, action: ActionId) -> Result<Msg, Failure> {
    let t = target(a, action).await?;
    let engine = a.engine();
    let member = Person::unnamed(t.action.member.account());
    let undo = undo_from_original(t.action.kind);
    let done = |r: Result<ControlDone, ActionError>| r.map_err(|e| failed(&t, &member, &e));
    match work {
        Control::Retry => {
            done(
                actions::retry(engine, &t.principal, action, &op_key(a.ix, "action_retry")).await,
            )?;
            Ok(Msg::text(
                Tone::Success,
                retry_done(t.action.kind, t.case_number),
            ))
        }
        Control::Dismiss => {
            done(
                actions::dismiss(
                    engine,
                    &t.principal,
                    action,
                    &op_key(a.ix, "action_dismiss"),
                )
                .await,
            )?;
            Ok(Msg::text(
                Tone::Success,
                dismiss_done(t.action.kind, t.case_number),
            ))
        }
        Control::Ask => {
            let undo = undo.ok_or_else(|| not_undoable(&t, &member))?;
            Ok(
                Msg::text(Tone::Warning, undo_confirm(undo, &member, t.case_number)).buttons(vec![
                    Button::new(
                        undo_button(undo),
                        ButtonStyle::Danger,
                        ButtonAction::ConfirmUndo(action),
                    ),
                ]),
            )
        }
        Control::Undo => {
            let undo: Undo = undo.ok_or_else(|| not_undoable(&t, &member))?;
            done(actions::undo(engine, &t.principal, action, &op_key(a.ix, "action_undo")).await)?;
            Ok(Msg::text(
                Tone::Success,
                undo_queued(undo, &member, t.case_number),
            ))
        }
    }
}

fn not_undoable(t: &Target, member: &Person) -> Failure {
    let mut f = failed(
        t,
        member,
        &ActionError::Refused(quack_core::DomainError::ActionNotUndoable),
    );
    f.finish = crate::ack::Finish::Refused(Reason::Conflict);
    f
}

/// Retry {action} (`ar`).
pub(crate) async fn retry(p: Pending<'_>, action: ActionId, source: Source) -> Finish {
    control(p, source, Control::Retry, action).await
}

/// Dismiss failed {action} (`ad`).
pub(crate) async fn dismiss(p: Pending<'_>, action: ActionId, source: Source) -> Finish {
    control(p, source, Control::Dismiss, action).await
}

/// Remove timeout / Unban (`au`): ask to confirm.
pub(crate) async fn ask_undo(p: Pending<'_>, action: ActionId, source: Source) -> Finish {
    control(p, source, Control::Ask, action).await
}

/// Yes, remove timeout / Yes, unban (`auc`).
pub(crate) async fn undo(p: Pending<'_>, action: ActionId, source: Source) -> Finish {
    control(p, source, Control::Undo, action).await
}
