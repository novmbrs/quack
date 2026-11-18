//! Evidence files (routes 26 and 53, §7.4): a `302` to a fresh Discord CDN
//! URL from `quack_engine::evidence::resolve_file`. The staff route finds
//! the case in the path's server; the member route finds it only among the
//! member's own cases.

use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use quack_core::ids::FileId;
use quack_engine::evidence::{FileError, FileViewer, resolve_file};
use quack_text::api::ApiCode;
use quack_text::error::{ErrorFacts, Scene};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::error::{ApiError, ApiResult, ErrorEnvelope};
use crate::extract::{MemberSelf, Moderate, Staff, case_ref, member_case_id, uuid_id};
use crate::state::ApiState;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(staff_file))
        .routes(routes!(member_file))
}

/// The redirect to a resolved file.
async fn redirect(state: &ApiState, viewer: FileViewer<'_>, file: FileId) -> ApiResult<Response> {
    let url = resolve_file(state.engine(), viewer, file)
        .await
        .map_err(|e| match e {
            FileError::NotFound => ApiError::not_found(),
            FileError::Unavailable => ApiError::api(ApiCode::FileUnavailable),
            FileError::DiscordUnavailable => ApiError::api(ApiCode::DiscordUnavailable),
            FileError::Refused(d) => ApiError::domain(
                &d,
                &ErrorFacts {
                    scene: Scene::Staff,
                    ..ErrorFacts::default()
                },
            ),
            FileError::Db(e) => e.into(),
        })?;
    let location =
        HeaderValue::from_str(&url).map_err(|_| ApiError::api(ApiCode::FileUnavailable))?;
    let mut response = StatusCode::FOUND.into_response();
    response.headers_mut().insert(header::LOCATION, location);
    Ok(response)
}

/// A file of a case's evidence.
#[utoipa::path(get, path = "/api/guilds/{guild_id}/cases/{case_ref}/evidence/{file_id}", tag = "cases",
    params(("guild_id" = String, Path), ("case_ref" = String, Path), ("file_id" = String, Path)),
    responses((status = 302, description = "To the file on Discord's CDN"),
              (status = 404, body = ErrorEnvelope), (status = 502, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn staff_file(
    State(state): State<ApiState>,
    staff: Staff<Moderate>,
    Path(path): Path<(String, String, String)>,
) -> ApiResult<Response> {
    let file: FileId = uuid_id(&path.2)?;
    let viewer = FileViewer::Staff {
        principal: &staff.principal,
        case: case_ref(&path.1)?,
    };
    redirect(&state, viewer, file).await
}

/// A file of the member's own case.
#[utoipa::path(get, path = "/api/me/cases/{case_id}/evidence/{file_id}", tag = "me",
    params(("case_id" = String, Path), ("file_id" = String, Path)),
    responses((status = 302, description = "To the file on Discord's CDN"),
              (status = 404, body = ErrorEnvelope), (status = 502, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn member_file(
    State(state): State<ApiState>,
    member: MemberSelf,
    Path(path): Path<(String, String)>,
) -> ApiResult<Response> {
    let viewer = FileViewer::Member {
        account: member.account,
        case: member_case_id(&path.0)?,
    };
    let file: FileId = uuid_id(&path.1)?;
    redirect(&state, viewer, file).await
}
