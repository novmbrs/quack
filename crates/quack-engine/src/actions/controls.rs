//! Staff controls on an action (§5.8): retry, dismiss, and undo. Each
//! re-evaluates `quack_core::action::controls` inside its transaction
//! after locking the case and the action.

use quack_core::DomainError;
use quack_core::action::{ActionKind, ActionState, ActionView, Controls, StillApplies, controls};
use quack_core::ids::{ActionId, CaseId};
use quack_core::modlog::{ActionDetail, ModLogEntry, ModLogLinks};
use quack_core::permissions::{ApplyFacts, Capability, CheckPurpose, MemberSubject, check_member};
use quack_db::actions::{self, Action, NewAction};
use quack_db::cases::{self, Case};
use quack_db::jobs::{self, NewJob};
use quack_db::messages::{self, Owner};
use quack_db::{DbError, Tx};
use serde::Serialize;
use time::OffsetDateTime;

use crate::modlog::{self, By};
use crate::opkey::{KeyError, Keyed, begin_keyed, complete, request_hash, stored_id};
use crate::port::{Fresh, Membership, Outcome};
use crate::{Engine, OpKey, Principal, obs};

/// Why a staff control was refused.
#[derive(Debug, thiserror::Error)]
pub enum ActionError {
    /// No such action in this server (`404 action_not_found`).
    #[error("action not found")]
    NotFound,
    /// A refusal with a `quack-text` message (not retryable, voided case,
    /// timeout window passed, member checks, capability, 2FA).
    #[error("refused: {0:?}")]
    Refused(DomainError),
    /// The operation key was used for a different request.
    #[error("operation key reused")]
    OperationKeyReused,
    /// Discord didn't answer the actor checks' facts.
    #[error("Discord unavailable")]
    DiscordUnavailable,
    /// The database failed.
    #[error(transparent)]
    Db(#[from] DbError),
}

impl From<DomainError> for ActionError {
    fn from(error: DomainError) -> Self {
        Self::Refused(error)
    }
}

impl From<KeyError> for ActionError {
    fn from(error: KeyError) -> Self {
        match error {
            KeyError::Reused => Self::OperationKeyReused,
            KeyError::Db(db) => Self::Db(db),
        }
    }
}

/// What a control did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControlDone {
    /// The action retried or dismissed, or the new undo action.
    pub action: ActionId,
    /// Its case.
    pub case: CaseId,
    /// The same request already ran.
    pub replayed: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
enum Control {
    Retry,
    Dismiss,
    Undo,
}

#[derive(Serialize)]
struct ControlRequest {
    control: Control,
    action: ActionId,
}

/// Retries a failed action: a new round (`pending` + job) (§5.8).
pub async fn retry(
    engine: &Engine,
    principal: &Principal,
    action: ActionId,
    op_key: &OpKey,
) -> Result<ControlDone, ActionError> {
    obs::use_case("retry_action", Some(principal.guild), async {
        control(engine, principal, action, op_key, Control::Retry).await
    })
    .await
}

/// Dismisses a failed action: it leaves the failed-actions list; its
/// history is kept (§5.8).
pub async fn dismiss(
    engine: &Engine,
    principal: &Principal,
    action: ActionId,
    op_key: &OpKey,
) -> Result<ControlDone, ActionError> {
    obs::use_case("dismiss_action", Some(principal.guild), async {
        control(engine, principal, action, op_key, Control::Dismiss).await
    })
    .await
}

/// Undoes a done timeout or ban by hand: a new undo action (`pending` +
/// job). The undo guard and provenance still apply when it runs (§5.6).
pub async fn undo(
    engine: &Engine,
    principal: &Principal,
    original: ActionId,
    op_key: &OpKey,
) -> Result<ControlDone, ActionError> {
    obs::use_case("undo_action", Some(principal.guild), async {
        control(engine, principal, original, op_key, Control::Undo).await
    })
    .await
}

async fn control(
    engine: &Engine,
    principal: &Principal,
    id: ActionId,
    op_key: &OpKey,
    which: Control,
) -> Result<ControlDone, ActionError> {
    let guild = principal.guild;
    let (action, case) = {
        let mut conn = engine.db().conn().await?;
        principal.require(&mut conn, Capability::Moderate).await??;
        let action = actions::get(&mut conn, guild, id)
            .await?
            .ok_or(ActionError::NotFound)?;
        let case = cases::get(&mut conn, guild, action.case)
            .await?
            .ok_or(ActionError::NotFound)?;
        (action, case)
    };
    actor_checks(engine, principal, &action, &case, which).await?;
    let key = op_key.scoped(Some(guild), Some(principal.account));
    let hash = request_hash(&ControlRequest {
        control: which,
        action: id,
    });
    let mut tx = match begin_keyed(engine.db(), &key, &hash).await? {
        Keyed::Replay(response) => {
            return Ok(ControlDone {
                action: stored_id(&response, "action_id")?,
                case: case.id,
                replayed: true,
            });
        }
        Keyed::Fresh(tx) => tx,
    };
    principal.require(&mut tx, Capability::Moderate).await??;
    let case = cases::lock(&mut tx, guild, case.id)
        .await?
        .ok_or(ActionError::NotFound)?;
    let action = actions::lock(&mut tx, guild, id)
        .await?
        .ok_or(ActionError::NotFound)?;
    let allowed = allowed(&mut tx, &case, &action).await?;
    let by = By {
        actor: Some(principal.account),
        source: principal.origin.mod_log_source(),
    };
    let (target, status) = match which {
        Control::Retry => {
            allowed.retry?;
            if !actions::retry(&mut tx, guild, id, principal.account).await? {
                return Err(DomainError::ActionNotRetryable.into());
            }
            jobs::enqueue(&mut tx, &NewJob::run_action(guild, id)).await?;
            log(
                &mut tx,
                &case,
                &action,
                by,
                ModLogEntry::ActionRetryRequested,
            )
            .await?;
            (id, 200)
        }
        Control::Dismiss => {
            allowed.dismiss?;
            if !actions::dismiss(&mut tx, guild, id, principal.account).await? {
                return Err(DomainError::ActionNotDismissable.into());
            }
            log(&mut tx, &case, &action, by, ModLogEntry::ActionDismissed).await?;
            (id, 200)
        }
        Control::Undo => {
            let kind = allowed.undo?;
            let undo = insert_undo(&mut tx, &case, &action, kind, principal).await?;
            (undo.id, 201)
        }
    };
    messages::bump(&mut tx, guild, Owner::Case(case.id)).await?;
    complete(
        &mut tx,
        &key,
        status,
        serde_json::json!({ "action_id": target }),
    )
    .await?;
    tx.commit().await?;
    Ok(ControlDone {
        action: target,
        case: case.id,
        replayed: false,
    })
}

/// The controls `quack_core::action::controls` allows now, read under
/// the case and action locks.
pub(crate) async fn allowed(
    tx: &mut Tx,
    case: &Case,
    action: &Action,
) -> Result<Controls, DbError> {
    let has_undo = if action.kind.is_original() {
        actions::undo_of(tx, case.guild, action.id).await?.is_some()
    } else {
        false
    };
    let guard = undo_guard(tx, case, action, has_undo).await?;
    Ok(controls(
        &view(action, has_undo, guard),
        case.is_valid(),
        OffsetDateTime::now_utc(),
    ))
}

/// The undo guard (§5.6) for a done original's undo, read like the
/// undo's own start would; `None` when there is nothing to undo.
pub(crate) async fn undo_guard(
    conn: &mut quack_db::PgConnection,
    case: &Case,
    action: &Action,
    has_undo: bool,
) -> Result<Option<StillApplies>, DbError> {
    if has_undo || action.state != ActionState::Done || action.kind.undo_kind().is_none() {
        return Ok(None);
    }
    Ok(
        actions::guard(conn, case.guild, case.member, action.kind, case.id)
            .await?
            .map(|(case_id, case_number)| StillApplies {
                case_id,
                case_number,
            }),
    )
}

/// An action as the controls see it.
pub(crate) fn view(
    action: &Action,
    has_undo: bool,
    undo_guard: Option<StillApplies>,
) -> ActionView {
    ActionView {
        kind: action.kind,
        state: action.state,
        dismissed: action.dismissed.is_some(),
        failure: action.failure.map(|f| f.reason),
        timeout_ends_at: action.timeout_ends_at,
        has_undo,
        undo_guard,
    }
}

async fn log(
    tx: &mut Tx,
    case: &Case,
    action: &Action,
    by: By,
    entry: fn(ActionDetail) -> ModLogEntry,
) -> Result<(), DbError> {
    let detail = ActionDetail {
        case_number: case.number,
        kind: action.kind,
    };
    modlog::write(
        tx,
        case.guild,
        entry(detail),
        by,
        ModLogLinks {
            case_id: Some(case.id),
            member_id: Some(case.member),
            action_id: Some(action.id),
            ..ModLogLinks::default()
        },
    )
    .await?;
    Ok(())
}

async fn insert_undo(
    tx: &mut Tx,
    case: &Case,
    original: &Action,
    kind: ActionKind,
    principal: &Principal,
) -> Result<NewAction, ActionError> {
    let undo = NewAction {
        id: ActionId::from_uuid(uuid::Uuid::now_v7()),
        guild: case.guild,
        case: case.id,
        member: case.member,
        kind,
        undo_of: Some(original.id),
        timeout_ends_at: None,
        delete_message_seconds: None,
        requested_by: Some(principal.account),
        created_at: None,
    };
    actions::insert(tx, &undo)
        .await
        .map_err(|e| match e.conflict() {
            Some(quack_db::Conflict::ActionExists) => DomainError::ActionNotUndoable.into(),
            _ => ActionError::Db(e),
        })?;
    jobs::enqueue(tx, &NewJob::run_action(case.guild, undo.id)).await?;
    let by = By {
        actor: Some(principal.account),
        source: principal.origin.mod_log_source(),
    };
    let detail = ActionDetail {
        case_number: case.number,
        kind,
    };
    modlog::write(
        tx,
        case.guild,
        ModLogEntry::ActionUndoRequested(detail),
        by,
        ModLogLinks {
            case_id: Some(case.id),
            member_id: Some(case.member),
            action_id: Some(undo.id),
            ..ModLogLinks::default()
        },
    )
    .await?;
    Ok(undo)
}

/// The actor checks of §5.8, from fresh facts, through the shared member
/// checks: retrying an original runs checks 2, 4, 6, 7 (and 3 for a
/// timeout); retrying or requesting a remove timeout runs check 7 when the
/// member is in the server; an unban and dismiss run none.
async fn actor_checks(
    engine: &Engine,
    principal: &Principal,
    action: &Action,
    case: &Case,
    which: Control,
) -> Result<(), ActionError> {
    let purpose = match which {
        Control::Dismiss => return Ok(()),
        Control::Retry => CheckPurpose::RetryAction(action.kind),
        Control::Undo => match action.kind.undo_kind() {
            Some(undo) => CheckPurpose::RequestUndo(undo),
            // Not undoable: the write refuses it under lock.
            None => return Ok(()),
        },
    };
    if !purpose.needs_facts() {
        return Ok(());
    }
    let discord = engine.discord();
    let Outcome::Done(guild) = discord.guild_facts(case.guild).await else {
        return Err(ActionError::DiscordUnavailable);
    };
    let member = match discord.member(case.guild, case.member, Fresh::Cached).await {
        Outcome::Done(Membership::Present(m)) => m.subject(),
        Outcome::Done(Membership::Absent) => MemberSubject {
            id: case.member,
            bot: false,
            present: None,
        },
        _ => return Err(ActionError::DiscordUnavailable),
    };
    let facts = ApplyFacts {
        owner_id: guild.owner_id,
        quack: guild
            .quack
            .as_ref()
            .map(quack_core::permissions::MemberFacts::present),
        actor: principal.presence(),
        member,
    };
    check_member(&facts, purpose).map_err(DomainError::MemberRefused)?;
    Ok(())
}
