//! Uploading files as evidence (route 60, §7.1, §14.8). Everything that
//! can refuse the upload without its bytes runs first (the session, CSRF,
//! the capability, the case, the evidence channel unless the request
//! already ran); then the body is read part by part within the upload
//! memory budget and an overall deadline, the session and capability are
//! checked again, and the files go to the same `add_evidence` Discord
//! uses, as one uploaded-files item.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Request, State};
use axum::http::header;
use axum::response::Response;
use quack_core::DomainError;
use quack_core::limits::MAX_UPLOAD_BODY_BYTES;
use quack_engine::CaseRef;
use quack_engine::OpKey;
use quack_engine::cases::{self, CaseError};
use quack_engine::evidence::check_destination;
use quack_text::api::ApiCode;
use quack_text::render::plain;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use utoipa::ToSchema;

use super::evidence::{added, refused};
use super::staff_facts;
use super::types::EvidenceAdded;
use super::upload_form::{self, FormError};
use crate::error::{ApiError, ApiResult, ErrorEnvelope, Facts, map};
use crate::extract::{IdempotencyKey, Moderate, Staff, case_ref};
use crate::state::ApiState;

/// Upload bytes the API may hold at once, across every upload in flight:
/// two full-size requests. Each upload reserves its declared length (the
/// whole cap without one) before reading; the rest wait.
pub(crate) const UPLOAD_BUDGET_BYTES: usize = 2 * MAX_UPLOAD_BODY_BYTES;

/// How long an upload waits for room in the budget before `503
/// unavailable` (nothing saved; the browser may send it again).
const BUDGET_WAIT: Duration = Duration::from_secs(30);

/// The budget's unit: permits are KiB.
const UNIT: usize = 1024;

/// The upload memory budget (§14.8): a semaphore of KiB.
#[derive(Debug)]
pub(crate) struct UploadBudget(Arc<Semaphore>);

impl UploadBudget {
    /// A budget of `bytes`.
    pub(crate) fn new(bytes: usize) -> Self {
        Self(Arc::new(Semaphore::new(bytes.div_ceil(UNIT))))
    }

    /// Room for a body of `declared` bytes (the cap when unknown or
    /// larger), waiting at most [`BUDGET_WAIT`].
    async fn reserve(&self, declared: Option<u64>) -> Result<OwnedSemaphorePermit, ApiError> {
        let bytes = declared
            .and_then(|n| usize::try_from(n).ok())
            .map_or(MAX_UPLOAD_BODY_BYTES, |n| n.min(MAX_UPLOAD_BODY_BYTES));
        let units = u32::try_from(bytes.div_ceil(UNIT).max(1)).unwrap_or(u32::MAX);
        match tokio::time::timeout(BUDGET_WAIT, self.0.clone().acquire_many_owned(units)).await {
            Ok(Ok(permit)) => Ok(permit),
            Ok(Err(_)) | Err(_) => Err(ApiError::api(ApiCode::Unavailable)),
        }
    }
}

/// Route 60's `multipart/form-data` body: files and an optional note.
// Only documents the form: the handler reads the parts as they arrive.
#[derive(ToSchema)]
#[allow(dead_code, reason = "documents the multipart form; never built")]
pub(crate) struct UploadEvidence {
    /// 1–10 files: images, videos, audio, plain text, or PDFs, up to
    /// 10 MiB each. Each part's file name and `Content-Type` are used.
    #[schema(value_type = Vec<String>, format = Binary)]
    files: Vec<Vec<u8>>,
    /// A staff note (0–500 characters) for the uploaded files.
    note: Option<String>,
}

/// Checks the case and the evidence channel before reading any bytes. A
/// request that already ran skips the channel: its replay must not depend
/// on it (§4.3); `add_evidence` checks the hash before replaying.
async fn precheck(
    state: &ApiState,
    staff: &Staff<Moderate>,
    case: CaseRef,
    key: &OpKey,
) -> ApiResult<()> {
    let found = {
        let mut conn = state.engine().db().conn().await?;
        match case {
            CaseRef::Number(n) => {
                quack_db::cases::get_by_number(&mut conn, staff.guild(), n).await?
            }
            CaseRef::Id(id) => quack_db::cases::get(&mut conn, staff.guild(), id).await?,
        }
    };
    let Some(found) = found else {
        let facts = Facts {
            case_number: match case {
                CaseRef::Number(n) => Some(n),
                CaseRef::Id(_) => None,
            },
            ..staff_facts()
        };
        return Err(facts.domain(DomainError::CaseNotFound));
    };
    let ran = cases::add_evidence_ran(state.engine(), &staff.principal, key)
        .await
        .map_err(|e| map::case(e, &staff_facts(), ""))?;
    if ran {
        return Ok(());
    }
    if let Err(e) = check_destination(state.engine(), staff.guild()).await {
        let facts = Facts {
            case_number: Some(found.number),
            ..staff_facts()
        };
        return Err(map::case(CaseError::from(e), &facts, ""));
    }
    Ok(())
}

/// Uploads 1–10 files (and a note) as one evidence item, copied to the
/// server's staff-only evidence channel. Only staff see them until someone
/// shows them to the member.
#[utoipa::path(post, path = "/api/guilds/{guild_id}/cases/{case_ref}/evidence/uploads",
    tag = "cases",
    params(("guild_id" = String, Path), ("case_ref" = String, Path)),
    request_body(content = inline(UploadEvidence), content_type = "multipart/form-data"),
    responses((status = 200, body = EvidenceAdded), (status = 400, body = ErrorEnvelope),
              (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope),
              (status = 413, body = ErrorEnvelope), (status = 415, body = ErrorEnvelope),
              (status = 422, body = ErrorEnvelope)),
    security(("session" = [], "csrf" = [])))]
pub(super) async fn upload_evidence(
    State(state): State<ApiState>,
    staff: Staff<Moderate>,
    key: IdempotencyKey,
    Path(path): Path<(String, String)>,
    request: Request,
) -> ApiResult<Response> {
    let case = case_ref(&path.1)?;
    let headers = request.headers().clone();
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok());
    let Some(boundary) = upload_form::boundary(content_type) else {
        return Err(ApiError::api(ApiCode::UnsupportedMediaType));
    };
    let declared = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    precheck(&state, &staff, case, &key.0).await?;
    let room = state.uploads().reserve(declared).await?;
    let form = match upload_form::read(request.into_body(), &boundary).await {
        Ok(form) => form,
        Err(e) => return Err(form_refused(&state, &staff, case, e).await),
    };
    // Reading may have taken minutes: whoever is signed in must still be
    // allowed to add evidence before anything is staged.
    let staff = staff.again(&state, &headers).await?;
    if form.files.is_empty() {
        return Err(ApiError::field(
            "files",
            "no_links",
            plain(&quack_text::error::no_links()),
        ));
    }
    let add = cases::AddEvidence {
        links: Vec::new(),
        files: form.files,
        note: form.note,
    };
    let done = cases::add_evidence(state.engine(), &staff.principal, case, &add, &key.0).await;
    // The bytes are in PostgreSQL now (or refused): free the budget before
    // reading the case back.
    drop(add);
    drop(room);
    match done {
        Ok(done) => added(&state, &staff, done).await,
        Err(e) => Err(refused(&state, &staff, case, e).await),
    }
}

/// A form that can't be used, answered as `add_evidence` would refuse it.
async fn form_refused(
    state: &ApiState,
    staff: &Staff<Moderate>,
    case: CaseRef,
    error: FormError,
) -> ApiError {
    let as_case = match error {
        FormError::TooManyFiles => CaseError::Refused(DomainError::TooManyFiles),
        FormError::Refused(files) => CaseError::FilesRefused(files),
        FormError::NoteTooLong => CaseError::NoteLength,
        other => return other.api().unwrap_or_else(ApiError::internal),
    };
    refused(state, staff, case, as_case).await
}
