//! The signed-in member's own cases (routes 51, 52, 54, 55). Each case is
//! found by its ID and its member in one query, so anyone else's case is
//! `404 not_found`; responses use only the member projection.

mod types;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use quack_core::ids::{CaseId, GuildId};
use quack_engine::Origin;
use quack_engine::appeals;
use quack_engine::cases;
use quack_text::error::Scene;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use types::{MemberCase, MemberCaseSummary};

use crate::error::{ApiResult, ErrorEnvelope, Facts, map};
use crate::extract::{Body, IdempotencyKey, MemberSelf, member_case_id, snowflake};
use crate::pagination;
use crate::state::ApiState;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(my_cases))
        .routes(routes!(my_case))
        .routes(routes!(submit_appeal))
        .routes(routes!(add_information))
}

fn facts() -> Facts {
    Facts::scene(Scene::Member)
}

/// Route 51's query.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct MyCasesQuery {
    /// Only cases in this server.
    guild_id: Option<String>,
    /// 1–100, default 50.
    limit: Option<u32>,
    /// From the previous page's `next_cursor`.
    cursor: Option<String>,
}

/// Route 51's body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct MemberCasePage {
    cases: Vec<MemberCaseSummary>,
    next_cursor: Option<String>,
}

/// The signed-in member's own cases, newest first.
#[utoipa::path(get, path = "/api/me/cases", tag = "me", params(MyCasesQuery),
    responses((status = 200, body = MemberCasePage), (status = 401, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn my_cases(
    State(state): State<ApiState>,
    member: MemberSelf,
    Query(q): Query<MyCasesQuery>,
) -> ApiResult<Json<MemberCasePage>> {
    let guild: Option<GuildId> = q
        .guild_id
        .as_deref()
        .map(|g| snowflake(g, "guild_id"))
        .transpose()?;
    let limit = pagination::limit(q.limit)?;
    let filters = format!("guild={}", guild.map_or(0, GuildId::get));
    let cursor = pagination::decode_time(q.cursor.as_deref(), &filters)?;
    let rows = cases::member_cases(state.engine(), member.account, guild, cursor, limit)
        .await
        .map_err(|e| map::case(e, &facts(), ""))?;
    let (rows, next_cursor) = pagination::split(rows, |r| {
        pagination::time_cursor(r.created_at, r.id.get(), &filters)
    });
    Ok(Json(MemberCasePage {
        cases: rows.into_iter().map(Into::into).collect(),
        next_cursor,
    }))
}

async fn view(state: &ApiState, member: &MemberSelf, case: CaseId) -> ApiResult<MemberCase> {
    let v = cases::member_case(state.engine(), member.account, case)
        .await
        .map_err(|e| map::case(e, &facts(), ""))?;
    let icon = {
        let mut conn = state.engine().db().conn().await?;
        quack_db::guilds::get(&mut conn, v.guild_id)
            .await?
            .and_then(|g| g.icon_hash)
    };
    Ok(MemberCase::new(v, icon.as_deref()))
}

/// One of the member's own cases.
#[utoipa::path(get, path = "/api/me/cases/{case_id}", tag = "me",
    params(("case_id" = String, Path, description = "The case's UUID or legacy ULID")),
    responses((status = 200, body = MemberCase), (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn my_case(
    State(state): State<ApiState>,
    member: MemberSelf,
    Path(case): Path<String>,
) -> ApiResult<Json<MemberCase>> {
    let case = member_case_id(&case)?;
    Ok(Json(view(&state, &member, case).await?))
}

/// Route 54's body.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SubmitAppeal {
    /// Why the case should be reconsidered (1–4,000 characters).
    statement: String,
}

/// Appeals one of the member's own cases.
#[utoipa::path(post, path = "/api/me/cases/{case_id}/appeal", tag = "me",
    params(("case_id" = String, Path)), request_body = SubmitAppeal,
    responses((status = 201, body = MemberCase), (status = 404, body = ErrorEnvelope),
              (status = 409, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope)),
    security(("session" = [], "csrf" = [])))]
async fn submit_appeal(
    State(state): State<ApiState>,
    member: MemberSelf,
    key: IdempotencyKey,
    Path(case): Path<String>,
    Body(body): Body<SubmitAppeal>,
) -> ApiResult<(StatusCode, Json<MemberCase>)> {
    let case = member_case_id(&case)?;
    appeals::submit(
        state.engine(),
        member.account,
        Origin::Dashboard,
        case,
        &body.statement,
        &key.0,
    )
    .await
    .map_err(|e| map::appeal(e, &facts(), "statement", &body.statement))?;
    Ok((
        StatusCode::CREATED,
        Json(view(&state, &member, case).await?),
    ))
}

/// Route 55's body.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct AddInformation {
    /// The reply staff asked for (1–4,000 characters).
    text: String,
    /// The appeal's `version` the member saw.
    expected_version: u32,
}

/// Replies to staff's request for more information.
#[utoipa::path(post, path = "/api/me/cases/{case_id}/appeal/information", tag = "me",
    params(("case_id" = String, Path)), request_body = AddInformation,
    responses((status = 200, body = MemberCase), (status = 404, body = ErrorEnvelope),
              (status = 409, body = ErrorEnvelope)),
    security(("session" = [], "csrf" = [])))]
async fn add_information(
    State(state): State<ApiState>,
    member: MemberSelf,
    key: IdempotencyKey,
    Path(case): Path<String>,
    Body(body): Body<AddInformation>,
) -> ApiResult<Json<MemberCase>> {
    let case = member_case_id(&case)?;
    appeals::add_information(
        state.engine(),
        member.account,
        Origin::Dashboard,
        case,
        &body.text,
        body.expected_version,
        &key.0,
    )
    .await
    .map_err(|e| map::appeal(e, &facts(), "text", &body.text))?;
    Ok(Json(view(&state, &member, case).await?))
}
