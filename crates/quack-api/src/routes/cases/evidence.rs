//! A case's evidence (routes 25, 61–63, §7.5): add message links, edit an
//! item's staff note, show uploaded files to the member or hide them, and
//! remove an item. Each answers the case and the bot's sentence.

use axum::Json;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use quack_core::evidence::clean_note;
use quack_core::ids::EvidenceId;
use quack_engine::CaseRef;
use quack_engine::cases::{self, CaseError};
use quack_engine::evidence::{self, EvidenceEdited as Edited};
use quack_text::render::plain;
use serde::Deserialize;
use utoipa::ToSchema;

use super::types::{EvidenceAdded, EvidenceEdited};
use super::{invalid, number_of, read, staff_facts};
use crate::error::{ApiError, ApiResult, ErrorEnvelope, Facts, map};
use crate::extract::{Body, IdempotencyKey, Moderate, Staff, case_ref, uuid_id};
use crate::state::ApiState;

/// Route 25's body.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct AddEvidence {
    /// 1–10 message links in this server.
    links: Vec<String>,
    /// A staff note (0–500 characters) for each item this adds.
    #[serde(default)]
    note: Option<String>,
}

/// A refusal of an evidence write, worded with the case's number.
pub(super) async fn refused(
    state: &ApiState,
    staff: &Staff<Moderate>,
    case: CaseRef,
    error: CaseError,
) -> ApiError {
    let facts = Facts {
        case_number: number_of(state, staff, case).await,
        ..staff_facts()
    };
    map::case(error, &facts, "")
}

/// The answer to an add: the case and the bot's sentence; a replay says so
/// (`Idempotency-Replayed: true`, §4.3).
pub(super) async fn added(
    state: &ApiState,
    staff: &Staff<Moderate>,
    added: cases::EvidenceAdded,
) -> ApiResult<Response> {
    let view = read(state, staff, CaseRef::Id(added.case)).await?;
    let message = plain(&quack_text::evidence::added(&quack_text::evidence::Added {
        number: added.number,
        new: added.new,
        retrying: added.retrying,
        restored: added.restored,
        already_saved: added.already_saved,
        files: added.files,
    }));
    let body = Json(EvidenceAdded {
        case: view.into(),
        new: added.new,
        retrying: added.retrying,
        restored: added.restored,
        already_saved: added.already_saved,
        files: added.files,
        message,
    });
    Ok(if added.replayed {
        ([(REPLAYED_HEADER, "true")], body).into_response()
    } else {
        body.into_response()
    })
}

/// Marks an answer that replays a stored result (§4.3).
pub(super) const REPLAYED_HEADER: &str = "idempotency-replayed";

/// Saves more messages as a case's evidence.
#[utoipa::path(post, path = "/api/guilds/{guild_id}/cases/{case_ref}/evidence", tag = "cases",
    params(("guild_id" = String, Path), ("case_ref" = String, Path)), request_body = AddEvidence,
    responses((status = 200, body = EvidenceAdded), (status = 400, body = ErrorEnvelope),
              (status = 422, body = ErrorEnvelope)),
    security(("session" = [], "csrf" = [])))]
pub(super) async fn add_evidence(
    State(state): State<ApiState>,
    staff: Staff<Moderate>,
    key: IdempotencyKey,
    Path(path): Path<(String, String)>,
    Body(body): Body<AddEvidence>,
) -> ApiResult<Response> {
    let case = case_ref(&path.1)?;
    if body.links.is_empty() {
        return Err(invalid("links"));
    }
    let add = cases::AddEvidence {
        links: body.links,
        files: Vec::new(),
        note: body.note,
    };
    match cases::add_evidence(state.engine(), &staff.principal, case, &add, &key.0).await {
        Ok(done) => added(&state, &staff, done).await,
        Err(e) => Err(refused(&state, &staff, case, e).await),
    }
}

/// The case and the item named in an evidence path.
fn evidence_path(path: &(String, String, String)) -> ApiResult<(CaseRef, EvidenceId)> {
    Ok((case_ref(&path.1)?, uuid_id(&path.2)?))
}

/// The answer to an edit: the case after it and the bot's sentence.
async fn edited(
    state: &ApiState,
    staff: &Staff<Moderate>,
    edited: Edited,
    message: String,
) -> ApiResult<Json<EvidenceEdited>> {
    let view = read(state, staff, CaseRef::Id(edited.case)).await?;
    Ok(Json(EvidenceEdited {
        case: view.into(),
        message,
    }))
}

/// Route 61's body.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SetNote {
    /// The note (0–500 characters); null or blank clears it.
    note: Option<String>,
    /// The `note_revision` the editor loaded; another is `409 note_changed`.
    expected_note_revision: u32,
}

/// Sets or clears an evidence item's staff note.
#[utoipa::path(put, path = "/api/guilds/{guild_id}/cases/{case_ref}/evidence/{evidence_id}/note",
    tag = "cases",
    params(("guild_id" = String, Path), ("case_ref" = String, Path), ("evidence_id" = String, Path)),
    request_body = SetNote,
    responses((status = 200, body = EvidenceEdited), (status = 400, body = ErrorEnvelope),
              (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope)),
    security(("session" = [], "csrf" = [])))]
pub(super) async fn set_note(
    State(state): State<ApiState>,
    staff: Staff<Moderate>,
    key: IdempotencyKey,
    Path(path): Path<(String, String, String)>,
    Body(body): Body<SetNote>,
) -> ApiResult<Json<EvidenceEdited>> {
    let (case, item) = evidence_path(&path)?;
    let done = evidence::set_note(
        state.engine(),
        &staff.principal,
        Some(case),
        item,
        body.note.as_deref(),
        body.expected_note_revision,
        &key.0,
    )
    .await;
    let done = match done {
        Ok(d) => d,
        Err(e) => return Err(refused(&state, &staff, case, e).await),
    };
    let cleared = clean_note(body.note.as_deref()).ok().flatten().is_none();
    let message = plain(&quack_text::evidence::note_saved(done.number, cleared));
    edited(&state, &staff, done, message).await
}

/// Route 62's body.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SetVisibility {
    /// Whether the member sees these uploaded files on their case page.
    visible_to_member: bool,
}

/// Shows uploaded files to the case's member, or hides them again.
#[utoipa::path(put,
    path = "/api/guilds/{guild_id}/cases/{case_ref}/evidence/{evidence_id}/visibility",
    tag = "cases",
    params(("guild_id" = String, Path), ("case_ref" = String, Path), ("evidence_id" = String, Path)),
    request_body = SetVisibility,
    responses((status = 200, body = EvidenceEdited), (status = 404, body = ErrorEnvelope),
              (status = 409, body = ErrorEnvelope)),
    security(("session" = [], "csrf" = [])))]
pub(super) async fn set_visibility(
    State(state): State<ApiState>,
    staff: Staff<Moderate>,
    key: IdempotencyKey,
    Path(path): Path<(String, String, String)>,
    Body(body): Body<SetVisibility>,
) -> ApiResult<Json<EvidenceEdited>> {
    let (case, item) = evidence_path(&path)?;
    let visible = body.visible_to_member;
    let done = evidence::set_visibility(
        state.engine(),
        &staff.principal,
        Some(case),
        item,
        visible,
        &key.0,
    )
    .await;
    let done = match done {
        Ok(d) => d,
        Err(e) => return Err(refused(&state, &staff, case, e).await),
    };
    let message = plain(&quack_text::evidence::visibility_changed(
        done.number,
        visible,
    ));
    edited(&state, &staff, done, message).await
}

/// Removes an evidence item added by mistake: hidden from Discord and the
/// member, listed collapsed for staff, never deleted.
#[utoipa::path(post, path = "/api/guilds/{guild_id}/cases/{case_ref}/evidence/{evidence_id}/remove",
    tag = "cases",
    params(("guild_id" = String, Path), ("case_ref" = String, Path), ("evidence_id" = String, Path)),
    responses((status = 200, body = EvidenceEdited), (status = 404, body = ErrorEnvelope)),
    security(("session" = [], "csrf" = [])))]
pub(super) async fn remove(
    State(state): State<ApiState>,
    staff: Staff<Moderate>,
    key: IdempotencyKey,
    Path(path): Path<(String, String, String)>,
) -> ApiResult<Json<EvidenceEdited>> {
    let (case, item) = evidence_path(&path)?;
    let done =
        evidence::remove_evidence(state.engine(), &staff.principal, Some(case), item, &key.0).await;
    let done = match done {
        Ok(d) => d,
        Err(e) => return Err(refused(&state, &staff, case, e).await),
    };
    // A replay says what the first request did: the item is removed.
    let changed = done.changed || done.replayed;
    let message = plain(&quack_text::evidence::removed(done.number, changed));
    edited(&state, &staff, done, message).await
}
