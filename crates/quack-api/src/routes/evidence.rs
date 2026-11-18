//! Evidence files (routes 26 and 53, §7.4): a `302` to a fresh Discord CDN
//! URL, found by fetching the copy message (when the file was copied) or
//! the source message and locating the attachment by its ID. The staff
//! route finds the case in the path's server; the member route finds it
//! only among the member's own cases.

use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use quack_core::evidence::FileCopyState;
use quack_core::ids::{AttachmentId, ChannelId, FileId, GuildId, MessageId};
use quack_db::cases::Case;
use quack_engine::{CaseRef, Outcome};
use quack_text::api::ApiCode;
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

/// Where an attachment was; `Ok(None)` when the message or the attachment
/// is gone.
async fn locate(
    state: &ApiState,
    channel: ChannelId,
    message: MessageId,
    attachment: AttachmentId,
) -> ApiResult<Option<String>> {
    match state
        .engine()
        .discord()
        .fetch_message(channel, message)
        .await
    {
        Outcome::Done(m) => Ok(m
            .attachments
            .into_iter()
            .find(|a| a.id == attachment)
            .map(|a| a.url)),
        Outcome::Refused(_) => Ok(None),
        Outcome::RetryAfter(_) | Outcome::NotSent(_) | Outcome::Unknown(_) => {
            Err(ApiError::api(ApiCode::DiscordUnavailable))
        }
    }
}

/// The redirect for one file of `case`.
async fn redirect(
    state: &ApiState,
    guild: GuildId,
    case: &Case,
    file: FileId,
) -> ApiResult<Response> {
    let found = {
        let mut conn = state.engine().db().conn().await?;
        quack_db::evidence::file(&mut conn, guild, case.id, file).await?
    };
    let Some((file, source_channel, source_message)) = found else {
        return Err(ApiError::not_found());
    };
    let mut url = None;
    if file.copy_state == FileCopyState::Copied
        && let Some((channel, message, attachment)) = file.copy
    {
        url = locate(state, channel, message, attachment).await?;
    }
    if url.is_none()
        && let Some(attachment) = file.attachment_id
    {
        url = locate(state, source_channel, source_message, attachment).await?;
    }
    let url = url.ok_or_else(|| ApiError::api(ApiCode::FileUnavailable))?;
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
    let guild = staff.guild();
    let file: FileId = uuid_id(&path.2)?;
    let case = {
        let mut conn = state.engine().db().conn().await?;
        match case_ref(&path.1)? {
            CaseRef::Number(n) => quack_db::cases::get_by_number(&mut conn, guild, n).await?,
            CaseRef::Id(id) => quack_db::cases::get(&mut conn, guild, id).await?,
        }
    };
    let case = case.ok_or_else(ApiError::not_found)?;
    redirect(&state, guild, &case, file).await
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
    let case_id = member_case_id(&path.0)?;
    let file: FileId = uuid_id(&path.1)?;
    let case = {
        let mut conn = state.engine().db().conn().await?;
        quack_db::cases::get_for_member(&mut conn, member.account.member(), case_id).await?
    };
    let case = case.ok_or_else(ApiError::not_found)?;
    redirect(&state, case.guild, &case, file).await
}
