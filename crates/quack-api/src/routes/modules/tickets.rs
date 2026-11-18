//! Routes 47–50: the server's tickets, one ticket, its transcript, and
//! closing it, through `quack-tickets`. A ticket's member sees the member
//! projection (no staff IDs); staff with `moderate` see everything.
//! Missing and hidden tickets are both `404 not_found`.

use axum::Json;
use axum::extract::{Path, Query, State};
use quack_core::ids::{GuildId, TicketId};
use quack_core::modules::TicketState;
use quack_core::permissions::Capability;
use quack_engine::Principal;
use quack_text::api::ApiCode;
use quack_text::error::{ErrorFacts, Scene};
use quack_tickets::{TicketError, TicketView};
use serde::Deserialize;
use time::OffsetDateTime;
use utoipa::IntoParams;

use super::types::{Ticket, TicketPage, TicketTranscriptInfo, Transcript};
use crate::error::{ApiError, ApiResult, ErrorEnvelope};
use crate::extract::{IdempotencyKey, Moderate, SessionAccount, Staff, snowflake, uuid_id};
use crate::pagination;
use crate::state::ApiState;

/// A ticket error as the API's: hidden and missing look the same.
fn mapped(error: TicketError) -> ApiError {
    match error {
        TicketError::NotFound | TicketError::NotAllowed => ApiError::not_found(),
        TicketError::TranscriptUnavailable => ApiError::api(ApiCode::FileUnavailable),
        TicketError::Refused(d) => ApiError::domain(
            &d,
            &ErrorFacts {
                scene: Scene::Dashboard,
                ..ErrorFacts::default()
            },
        ),
        TicketError::DiscordUnavailable => ApiError::api(ApiCode::DiscordUnavailable),
        TicketError::Db(e) => e.into(),
    }
}

/// The wire ticket; `staff` picks the projection.
fn wire(t: &TicketView, staff: bool, viewer: quack_core::ids::AccountId) -> Ticket {
    let now = OffsetDateTime::now_utc();
    Ticket {
        id: t.id,
        member_id: t.member_id,
        state: t.state,
        thread_id: t.thread_id,
        created_at: t.created_at,
        opened_at: t.opened_at,
        close_requested_at: t.close_requested_at,
        closed_at: t.closed_at,
        closed_by: if staff { t.closed_by } else { None },
        closed_by_you: (!staff).then_some(t.closed_by == Some(viewer)),
        close_reason: t.close_reason,
        thread_delete_failed: t.thread_delete_failed,
        transcript: t.transcript.map(|s| TicketTranscriptInfo {
            state: if s.expires_at > now {
                "available"
            } else {
                "expired"
            }
            .to_owned(),
            complete: s.partial.is_none(),
            partial_cause: s.partial,
            expires_at: s.expires_at,
        }),
    }
}

/// Route 47's query.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct TicketListQuery {
    /// Only tickets in these states, comma-separated (`opening,open,closing`);
    /// absent: every state.
    state: Option<String>,
    /// 1–100, default 50.
    limit: Option<u32>,
    /// From the previous page's `next_cursor`.
    cursor: Option<String>,
}

/// The server's tickets, newest first.
#[utoipa::path(get, path = "/api/guilds/{guild_id}/tickets", tag = "modules",
    params(("guild_id" = String, Path), TicketListQuery),
    responses((status = 200, body = TicketPage)), security(("session" = [])))]
pub(super) async fn list_tickets(
    State(state): State<ApiState>,
    staff: Staff<Moderate>,
    Query(query): Query<TicketListQuery>,
) -> ApiResult<Json<TicketPage>> {
    let limit = pagination::limit(query.limit)?;
    let mut states = Vec::new();
    for s in query
        .state
        .as_deref()
        .unwrap_or("")
        .split(',')
        .filter(|s| !s.is_empty())
    {
        let parsed = s.parse::<TicketState>().map_err(|_| {
            ApiError::field(
                "state",
                "invalid",
                quack_text::render::plain(&quack_text::api::value_invalid()),
            )
        })?;
        if !states.contains(&parsed) {
            states.push(parsed);
        }
    }
    states.sort_by_key(|s| s.as_str());
    let filters = format!(
        "state={}",
        states
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(",")
    );
    let cursor = pagination::decode_time(query.cursor.as_deref(), &filters)?;
    let rows = quack_tickets::list(state.engine(), &staff.principal, &states, cursor, limit)
        .await
        .map_err(mapped)?;
    let (rows, next_cursor) = pagination::split(rows, |t| {
        pagination::time_cursor(t.created_at, t.id.get(), &filters)
    });
    let viewer = staff.principal.account;
    Ok(Json(TicketPage {
        tickets: rows.into_iter().map(|t| wire(&t, true, viewer)).collect(),
        next_cursor,
    }))
}

/// The viewer's principal in the path's server and whether they see the
/// staff projection.
async fn viewer(
    state: &ApiState,
    session: &SessionAccount,
    path: &(String, String),
) -> ApiResult<(Principal, bool, TicketId)> {
    let guild: GuildId = snowflake(&path.0, "guild_id")?;
    let ticket: TicketId = uuid_id(&path.1)?;
    let principal = crate::extract::staff_principal(state, guild, session.account).await?;
    let staff = {
        let mut conn = state.engine().db().conn().await?;
        principal
            .current_capabilities(&mut conn)
            .await?
            .check(Capability::Moderate)
            .is_ok()
    };
    Ok((principal, staff, ticket))
}

/// One ticket: its member or staff.
#[utoipa::path(get, path = "/api/guilds/{guild_id}/tickets/{ticket_id}", tag = "modules",
    params(("guild_id" = String, Path), ("ticket_id" = String, Path)),
    responses((status = 200, body = Ticket), (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
pub(super) async fn get_ticket(
    State(state): State<ApiState>,
    session: SessionAccount,
    Path(path): Path<(String, String)>,
) -> ApiResult<Json<Ticket>> {
    let (principal, staff, ticket) = viewer(&state, &session, &path).await?;
    let view = quack_tickets::get(state.engine(), &principal, ticket)
        .await
        .map_err(mapped)?;
    Ok(Json(wire(&view, staff, session.account)))
}

/// A ticket's transcript until it expires (`404 file_unavailable` after).
#[utoipa::path(get, path = "/api/guilds/{guild_id}/tickets/{ticket_id}/transcript", tag = "modules",
    params(("guild_id" = String, Path), ("ticket_id" = String, Path)),
    responses((status = 200, body = Transcript), (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
pub(super) async fn ticket_transcript(
    State(state): State<ApiState>,
    session: SessionAccount,
    Path(path): Path<(String, String)>,
) -> ApiResult<Json<Transcript>> {
    let (principal, _, ticket) = viewer(&state, &session, &path).await?;
    let t = quack_tickets::transcript(state.engine(), &principal, ticket)
        .await
        .map_err(mapped)?;
    Ok(Json(Transcript {
        complete: t.partial.is_none(),
        content: t.content,
        expires_at: t.expires_at,
    }))
}

/// Closes an open ticket: its member or staff with `moderate`.
#[utoipa::path(post, path = "/api/guilds/{guild_id}/tickets/{ticket_id}/close", tag = "modules",
    params(("guild_id" = String, Path), ("ticket_id" = String, Path)),
    responses((status = 200, body = Ticket), (status = 409, body = ErrorEnvelope)),
    security(("session" = [], "csrf" = [])))]
pub(super) async fn close_ticket(
    State(state): State<ApiState>,
    session: SessionAccount,
    _key: IdempotencyKey,
    Path(path): Path<(String, String)>,
) -> ApiResult<Json<Ticket>> {
    let (principal, staff, ticket) = viewer(&state, &session, &path).await?;
    let closed = quack_tickets::close(state.engine(), &principal, ticket).await;
    let view = quack_tickets::get(state.engine(), &principal, ticket)
        .await
        .map_err(mapped)?;
    match closed {
        Ok(_) => {}
        // A replay of this person's own close answers like the first time.
        Err(TicketError::Refused(quack_core::DomainError::TicketNotOpen))
            if view.closed_by == Some(session.account)
                && matches!(view.state, TicketState::Closing | TicketState::Closed) => {}
        Err(e) => return Err(mapped(e)),
    }
    Ok(Json(wire(&view, staff, session.account)))
}
