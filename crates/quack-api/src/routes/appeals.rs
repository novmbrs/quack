//! Appeals as staff see them (routes 32–34): the list, one appeal with
//! its timeline and case, and the one decision route.

use axum::Json;
use axum::extract::{Path, Query, State};
use quack_core::appeal::{AppealDecision, AppealEventKind, AppealState};
use quack_core::ids::{AccountId, AppealEventId, AppealId, CaseId, MemberId};
use quack_engine::CaseRef;
use quack_engine::appeals::{self, AppealView};
use quack_engine::cases;
use quack_text::error::Scene;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::cases::CaseSummary;
use crate::error::{ApiError, ApiResult, ErrorEnvelope, Facts, map};
use crate::extract::{Body, IdempotencyKey, Moderate, Staff, uuid_id};
use crate::pagination;
use crate::state::ApiState;
use crate::wire::{schema, ts};

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(list_appeals))
        .routes(routes!(get_appeal))
        .routes(routes!(decide))
}

/// An appeal in a list.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AppealSummary {
    #[schema(value_type = uuid::Uuid)]
    id: AppealId,
    #[schema(value_type = uuid::Uuid)]
    case_id: CaseId,
    case_number: u64,
    #[schema(value_type = schema::Snowflake)]
    member_id: MemberId,
    rule_name: String,
    #[schema(value_type = schema::AppealState)]
    state: AppealState,
    /// Send back as `expected_version` with a decision.
    version: u32,
    #[serde(with = "ts")]
    #[schema(value_type = String, format = DateTime)]
    created_at: OffsetDateTime,
    #[serde(with = "ts")]
    #[schema(value_type = String, format = DateTime)]
    updated_at: OffsetDateTime,
}

impl From<&AppealView> for AppealSummary {
    fn from(a: &AppealView) -> Self {
        Self {
            id: a.id,
            case_id: a.case_id,
            case_number: a.case_number,
            member_id: a.member_id,
            rule_name: a.rule_name.clone(),
            state: a.state,
            version: a.version,
            created_at: a.created_at,
            updated_at: a.updated_at,
        }
    }
}

/// One event on an appeal's timeline.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AppealEvent {
    #[schema(value_type = uuid::Uuid)]
    id: AppealEventId,
    #[schema(value_type = schema::AppealEventKind)]
    kind: AppealEventKind,
    /// Who; null = Quack.
    #[schema(value_type = Option<schema::Snowflake>)]
    actor_id: Option<AccountId>,
    /// The statement, reply, or reason.
    text: String,
    /// Quack closed it because the case was voided.
    closed_by_void: bool,
    #[serde(with = "ts")]
    #[schema(value_type = String, format = DateTime)]
    created_at: OffsetDateTime,
}

/// An appeal with its timeline and case.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct Appeal {
    #[serde(flatten)]
    summary: AppealSummary,
    statement: String,
    #[schema(value_type = Option<schema::Snowflake>)]
    decided_by: Option<AccountId>,
    #[serde(with = "ts::option")]
    #[schema(value_type = Option<String>, format = DateTime)]
    decided_at: Option<OffsetDateTime>,
    decision_reason: Option<String>,
    events: Vec<AppealEvent>,
    case: CaseSummary,
}

/// Route 32's body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AppealPage {
    appeals: Vec<AppealSummary>,
    next_cursor: Option<String>,
}

/// Route 32's query.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct AppealQuery {
    /// Only appeals in this state.
    state: Option<String>,
    /// 1–100, default 50.
    limit: Option<u32>,
    /// From the previous page's `next_cursor`.
    cursor: Option<String>,
}

fn staff_facts() -> Facts {
    Facts::scene(Scene::Staff)
}

/// The server's appeals, newest first.
#[utoipa::path(get, path = "/api/guilds/{guild_id}/appeals", tag = "appeals",
    params(("guild_id" = String, Path), AppealQuery),
    responses((status = 200, body = AppealPage), (status = 400, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn list_appeals(
    State(state): State<ApiState>,
    staff: Staff<Moderate>,
    Query(q): Query<AppealQuery>,
) -> ApiResult<Json<AppealPage>> {
    let appeal_state: Option<AppealState> = q
        .state
        .as_deref()
        .map(|s| {
            s.parse().map_err(|_| {
                ApiError::field(
                    "state",
                    "invalid",
                    quack_text::render::plain(&quack_text::api::validation_failed()),
                )
            })
        })
        .transpose()?;
    let limit = pagination::limit(q.limit)?;
    let filters = format!("state={}", appeal_state.map_or("", AppealState::as_str));
    let cursor = pagination::decode_time(q.cursor.as_deref(), &filters)?;
    let rows = appeals::list(
        state.engine(),
        &staff.principal,
        appeal_state,
        cursor,
        limit,
    )
    .await
    .map_err(|e| map::appeal(e, &staff_facts(), "", ""))?;
    let (rows, next_cursor) = pagination::split(rows, |a| {
        pagination::time_cursor(a.created_at, a.id.get(), &filters)
    });
    Ok(Json(AppealPage {
        appeals: rows.iter().map(Into::into).collect(),
        next_cursor,
    }))
}

async fn appeal(state: &ApiState, staff: &Staff<Moderate>, id: AppealId) -> ApiResult<Appeal> {
    let view = appeals::get(state.engine(), &staff.principal, id)
        .await
        .map_err(|e| map::appeal(e, &staff_facts(), "", ""))?;
    let case = cases::get(state.engine(), &staff.principal, CaseRef::Id(view.case_id))
        .await
        .map_err(|e| map::case(e, &staff_facts(), ""))?;
    Ok(Appeal {
        summary: (&view).into(),
        statement: view.statement,
        decided_by: view.decided_by,
        decided_at: view.decided_at,
        decision_reason: view.decision_reason,
        events: view
            .events
            .into_iter()
            .map(|e| AppealEvent {
                id: e.id,
                kind: e.kind,
                actor_id: e.actor_id,
                text: e.text,
                closed_by_void: e.closed_by_void,
                created_at: e.created_at,
            })
            .collect(),
        case: CaseSummary::from_view(&case),
    })
}

/// One appeal with its timeline.
#[utoipa::path(get, path = "/api/guilds/{guild_id}/appeals/{appeal_id}", tag = "appeals",
    params(("guild_id" = String, Path), ("appeal_id" = String, Path)),
    responses((status = 200, body = Appeal), (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn get_appeal(
    State(state): State<ApiState>,
    staff: Staff<Moderate>,
    Path(path): Path<(String, String)>,
) -> ApiResult<Json<Appeal>> {
    let id: AppealId = uuid_id(&path.1)?;
    Ok(Json(appeal(&state, &staff, id).await?))
}

/// A staff decision.
#[derive(Clone, Copy, Debug, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Decision {
    /// Ask the member for more information.
    RequestInformation,
    /// Accept: the case is voided.
    Accept,
    /// Reject: the case stands.
    Reject,
    /// Close without a decision.
    Close,
    /// Reopen a rejected or closed appeal.
    Reopen,
}

impl From<Decision> for AppealDecision {
    fn from(d: Decision) -> Self {
        match d {
            Decision::RequestInformation => Self::RequestInformation,
            Decision::Accept => Self::Accept,
            Decision::Reject => Self::Reject,
            Decision::Close => Self::Close,
            Decision::Reopen => Self::Reopen,
        }
    }
}

/// Route 34's body.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Decide {
    decision: Decision,
    /// Why (1–2,000 characters); the member reads it.
    reason: String,
    /// The version the reader saw; another is `409 appeal_changed`.
    expected_version: u32,
}

/// Decides on an appeal.
#[utoipa::path(post, path = "/api/guilds/{guild_id}/appeals/{appeal_id}/decision", tag = "appeals",
    params(("guild_id" = String, Path), ("appeal_id" = String, Path)), request_body = Decide,
    responses((status = 200, body = Appeal), (status = 404, body = ErrorEnvelope),
              (status = 409, body = ErrorEnvelope)),
    security(("session" = [], "csrf" = [])))]
async fn decide(
    State(state): State<ApiState>,
    staff: Staff<Moderate>,
    key: IdempotencyKey,
    Path(path): Path<(String, String)>,
    Body(body): Body<Decide>,
) -> ApiResult<Json<Appeal>> {
    let id: AppealId = uuid_id(&path.1)?;
    let decided = appeals::decide(
        state.engine(),
        &staff.principal,
        id,
        body.decision.into(),
        &body.reason,
        body.expected_version,
        &key.0,
    )
    .await;
    if let Err(e) = decided {
        return Err(map::appeal(e, &staff_facts(), "reason", &body.reason));
    }
    Ok(Json(appeal(&state, &staff, id).await?))
}
