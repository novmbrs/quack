//! Actions (routes 28–31): the failed-actions list and the staff
//! controls retry, dismiss, and undo.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use quack_core::action::{
    ActionFailure, ActionKind, ActionSkipReason, ActionState, AttemptOutcome,
};
use quack_core::ids::{AccountId, ActionId, CaseId, MemberId};
use quack_engine::actions::{self, ActionError, ControlDone};
use quack_engine::cases::{self, ActionItem};
use quack_text::error::Scene;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::error::{ApiError, ApiResult, ErrorEnvelope, Facts, map};
use crate::extract::{IdempotencyKey, Moderate, Staff, uuid_id};
use crate::pagination;
use crate::state::ApiState;
use crate::wire::{schema, ts};

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(list_actions))
        .routes(routes!(retry))
        .routes(routes!(dismiss))
        .routes(routes!(undo))
}

/// One attempt at an action.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct Attempt {
    number: u32,
    #[serde(with = "ts")]
    #[schema(value_type = String, format = DateTime)]
    started_at: OffsetDateTime,
    #[serde(with = "ts::option")]
    #[schema(value_type = Option<String>, format = DateTime)]
    finished_at: Option<OffsetDateTime>,
    #[schema(value_type = Option<schema::AttemptOutcome>)]
    outcome: Option<AttemptOutcome>,
    http_status: Option<u16>,
    discord_code: Option<u32>,
}

/// Whether an action is a case's original or the undo of one.
#[derive(Clone, Copy, Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Purpose {
    /// The case's own action.
    Original,
    /// Removes a timeout or ban.
    Undo,
}

/// An action as staff see it.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct Action {
    #[schema(value_type = uuid::Uuid)]
    id: ActionId,
    #[schema(value_type = uuid::Uuid)]
    case_id: CaseId,
    case_number: u64,
    #[schema(value_type = schema::ActionKind)]
    kind: ActionKind,
    purpose: Purpose,
    #[schema(value_type = Option<uuid::Uuid>)]
    undo_of_action_id: Option<ActionId>,
    #[schema(value_type = schema::ActionState)]
    state: ActionState,
    /// Where it stands, in words.
    outcome_text: String,
    #[serde(with = "ts::option")]
    #[schema(value_type = Option<String>, format = DateTime)]
    timeout_ends_at: Option<OffsetDateTime>,
    delete_message_seconds: Option<u32>,
    attempts: u32,
    #[serde(with = "ts::option")]
    #[schema(value_type = Option<String>, format = DateTime)]
    next_attempt_at: Option<OffsetDateTime>,
    #[schema(value_type = Option<schema::ActionFailure>)]
    failure_reason: Option<ActionFailure>,
    http_status: Option<u16>,
    discord_code: Option<u32>,
    #[schema(value_type = Option<schema::ActionSkipReason>)]
    skip_reason: Option<ActionSkipReason>,
    skipped_because_case_number: Option<u64>,
    #[schema(value_type = Option<schema::Snowflake>)]
    requested_by: Option<AccountId>,
    #[serde(with = "ts::option")]
    #[schema(value_type = Option<String>, format = DateTime)]
    dismissed_at: Option<OffsetDateTime>,
    #[schema(value_type = Option<schema::Snowflake>)]
    dismissed_by: Option<AccountId>,
    #[serde(with = "ts::option")]
    #[schema(value_type = Option<String>, format = DateTime)]
    done_at: Option<OffsetDateTime>,
    #[serde(with = "ts")]
    #[schema(value_type = String, format = DateTime)]
    created_at: OffsetDateTime,
    attempt_log: Vec<Attempt>,
    can_retry: bool,
    can_dismiss: bool,
    can_undo: bool,
}

impl Action {
    /// The wire form of one of a case's actions.
    pub(crate) fn from_item(case_id: CaseId, case_number: u64, a: ActionItem) -> Self {
        Self {
            id: a.id,
            case_id,
            case_number,
            kind: a.kind,
            purpose: if a.undo_of_action_id.is_some() {
                Purpose::Undo
            } else {
                Purpose::Original
            },
            undo_of_action_id: a.undo_of_action_id,
            state: a.state,
            outcome_text: a.outcome_text,
            timeout_ends_at: a.timeout_ends_at,
            delete_message_seconds: a.delete_message_seconds,
            attempts: a.attempts,
            next_attempt_at: a.next_attempt_at,
            failure_reason: a.failure_reason,
            http_status: a.http_status,
            discord_code: a.discord_code,
            skip_reason: a.skip_reason,
            skipped_because_case_number: a.skipped_because_case_number,
            requested_by: a.requested_by,
            dismissed_at: a.dismissed_at,
            dismissed_by: a.dismissed_by,
            done_at: a.done_at,
            created_at: a.created_at,
            attempt_log: a
                .attempt_log
                .into_iter()
                .map(|t| Attempt {
                    number: t.number,
                    started_at: t.started_at,
                    finished_at: t.finished_at,
                    outcome: t.outcome,
                    http_status: t.http_status,
                    discord_code: t.discord_code,
                })
                .collect(),
            can_retry: a.can_retry,
            can_dismiss: a.can_dismiss,
            can_undo: a.can_undo,
        }
    }
}

/// One row of the failed-actions page.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ActionListItem {
    #[serde(flatten)]
    action: Action,
    #[schema(value_type = schema::Snowflake)]
    member_id: MemberId,
    rule_name: String,
}

/// Route 28's body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ActionPage {
    actions: Vec<ActionListItem>,
    next_cursor: Option<String>,
}

/// Route 28's query.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct ActionQuery {
    /// The state to list (default `failed`).
    state: Option<String>,
    /// For `failed`: dismissed ones instead (default false).
    #[serde(default)]
    dismissed: bool,
    /// 1–100, default 50.
    limit: Option<u32>,
    /// From the previous page's `next_cursor`.
    cursor: Option<String>,
}

/// The server's actions in a state, newest first (failed and undismissed
/// by default).
#[utoipa::path(get, path = "/api/guilds/{guild_id}/actions", tag = "actions",
    params(("guild_id" = String, Path), ActionQuery),
    responses((status = 200, body = ActionPage), (status = 400, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn list_actions(
    State(state): State<ApiState>,
    staff: Staff<Moderate>,
    Query(q): Query<ActionQuery>,
) -> ApiResult<Json<ActionPage>> {
    let action_state: ActionState = match q.state.as_deref() {
        None => ActionState::Failed,
        Some(s) => s.parse().map_err(|_| {
            ApiError::field(
                "state",
                "invalid",
                quack_text::render::plain(&quack_text::api::validation_failed()),
            )
        })?,
    };
    let limit = pagination::limit(q.limit)?;
    let filters = format!("state={}&dismissed={}", action_state.as_str(), q.dismissed);
    let cursor = pagination::decode_time(q.cursor.as_deref(), &filters)?;
    let rows = cases::actions(
        state.engine(),
        &staff.principal,
        action_state,
        q.dismissed,
        cursor,
        limit,
    )
    .await
    .map_err(|e| map::case(e, &Facts::scene(Scene::Staff), ""))?;
    let (rows, next_cursor) = pagination::split(rows, |r| {
        pagination::time_cursor(r.item.created_at, r.item.id.get(), &filters)
    });
    let actions = rows
        .into_iter()
        .map(|r| ActionListItem {
            action: Action::from_item(r.case, r.case_number, r.item),
            member_id: r.member,
            rule_name: r.rule_name,
        })
        .collect();
    Ok(Json(ActionPage {
        actions,
        next_cursor,
    }))
}

/// The action a control left, read back from its case's records.
async fn answer(
    state: &ApiState,
    staff: &Staff<Moderate>,
    result: Result<ControlDone, ActionError>,
) -> ApiResult<Json<Action>> {
    let done = result.map_err(|e| map::action(e, &Facts::scene(Scene::Staff)))?;
    let found = cases::action(state.engine(), &staff.principal, done.case, done.action)
        .await
        .map_err(|e| match e {
            cases::CaseError::Refused(quack_core::DomainError::CaseNotFound) => {
                ApiError::api(quack_text::api::ApiCode::ActionNotFound)
            }
            other => map::case(other, &Facts::scene(Scene::Staff), ""),
        })?;
    Ok(Json(Action::from_item(
        found.case,
        found.case_number,
        found.item,
    )))
}

/// Facts that name the action in a refusal.
async fn action_facts(state: &ApiState, staff: &Staff<Moderate>, action: ActionId) -> Facts {
    let mut facts = Facts::scene(Scene::Staff);
    if let Ok(mut conn) = state.engine().db().conn().await
        && let Ok(Some(found)) = quack_db::actions::get(&mut conn, staff.guild(), action).await
    {
        facts.action = Some(found.kind);
    }
    facts
}

async fn control(
    state: &ApiState,
    staff: &Staff<Moderate>,
    action: &str,
    run: impl AsyncFnOnce(ActionId) -> Result<ControlDone, ActionError>,
) -> ApiResult<Json<Action>> {
    let action: ActionId =
        uuid_id(action).map_err(|_| ApiError::api(quack_text::api::ApiCode::ActionNotFound))?;
    match run(action).await {
        Err(ActionError::Refused(err)) => Err(action_facts(state, staff, action).await.domain(err)),
        result => answer(state, staff, result).await,
    }
}

/// Retries a failed action.
#[utoipa::path(post, path = "/api/guilds/{guild_id}/actions/{action_id}/retry", tag = "actions",
    params(("guild_id" = String, Path), ("action_id" = String, Path)),
    responses((status = 200, body = Action), (status = 404, body = ErrorEnvelope),
              (status = 409, body = ErrorEnvelope)),
    security(("session" = [], "csrf" = [])))]
async fn retry(
    State(state): State<ApiState>,
    staff: Staff<Moderate>,
    key: IdempotencyKey,
    Path(path): Path<(String, String)>,
) -> ApiResult<Json<Action>> {
    control(&state, &staff, &path.1, async |id| {
        actions::retry(state.engine(), &staff.principal, id, &key.0).await
    })
    .await
}

/// Dismisses a failed action: it leaves the failed-actions list.
#[utoipa::path(post, path = "/api/guilds/{guild_id}/actions/{action_id}/dismiss", tag = "actions",
    params(("guild_id" = String, Path), ("action_id" = String, Path)),
    responses((status = 200, body = Action), (status = 404, body = ErrorEnvelope),
              (status = 409, body = ErrorEnvelope)),
    security(("session" = [], "csrf" = [])))]
async fn dismiss(
    State(state): State<ApiState>,
    staff: Staff<Moderate>,
    key: IdempotencyKey,
    Path(path): Path<(String, String)>,
) -> ApiResult<Json<Action>> {
    control(&state, &staff, &path.1, async |id| {
        actions::dismiss(state.engine(), &staff.principal, id, &key.0).await
    })
    .await
}

/// Undoes a done timeout or ban; answers the new undo action.
#[utoipa::path(post, path = "/api/guilds/{guild_id}/actions/{action_id}/undo", tag = "actions",
    params(("guild_id" = String, Path), ("action_id" = String, Path)),
    responses((status = 201, body = Action), (status = 404, body = ErrorEnvelope),
              (status = 409, body = ErrorEnvelope)),
    security(("session" = [], "csrf" = [])))]
async fn undo(
    State(state): State<ApiState>,
    staff: Staff<Moderate>,
    key: IdempotencyKey,
    Path(path): Path<(String, String)>,
) -> ApiResult<(StatusCode, Json<Action>)> {
    let action = control(&state, &staff, &path.1, async |id| {
        actions::undo(state.engine(), &staff.principal, id, &key.0).await
    })
    .await?;
    Ok((StatusCode::CREATED, action))
}
