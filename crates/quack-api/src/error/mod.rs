//! The one error type every handler and middleware returns, and the §14.6
//! envelope it renders. The `message` always comes from `quack-text`; no
//! database, Discord, or Rust error text ever reaches a response.

pub(crate) mod map;

use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use quack_core::DomainError;
use quack_core::modules::ModuleName;
use quack_text::api::ApiCode;
use quack_text::error::{ErrorFacts, for_domain};
use quack_text::render::plain;
use serde::Serialize;
use utoipa::ToSchema;

pub(crate) use map::{Facts, from_db};

/// One field problem of a `validation_failed` (or `member_refused`) error.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, ToSchema)]
pub struct FieldError {
    /// The JSON path of the field, such as `levels[1].from_case_count`.
    pub field: String,
    /// The problem's machine code (§3.3, §4.6, §13.1, or a refusal).
    pub code: String,
    /// What to fix, in the reader's words.
    pub message: String,
}

/// One reason a module can't be turned on (`module_not_ready`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, ToSchema)]
pub struct Problem {
    /// The problem's machine code (`module.problem.<code>`).
    pub code: String,
    /// What to fix.
    pub message: String,
}

/// The body of an error response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, ToSchema)]
pub struct ErrorBody {
    /// The machine code (§14.6).
    pub code: String,
    /// What happened and what to do, from `quack-text`.
    pub message: String,
    /// The request's 32-hex trace ID, or null when it has no trace.
    pub trace_id: Option<String>,
    /// Field problems (`validation_failed`, `member_refused`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<FieldError>,
    /// Module problems (`module_not_ready`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub problems: Vec<Problem>,
    /// Seconds to wait (`rate_limited`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_seconds: Option<u64>,
}

/// `{ "error": ErrorBody }`: every error response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, ToSchema)]
pub struct ErrorEnvelope {
    /// The error.
    pub error: ErrorBody,
}

/// An API failure: status, code, message, and the extras §14.6 lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
    fields: Vec<FieldError>,
    problems: Vec<Problem>,
    retry_after: Option<u64>,
}

/// The HTTP status of a transport-level code (§14.6).
pub(crate) const fn status_of(code: ApiCode) -> StatusCode {
    match code {
        ApiCode::ValidationFailed | ApiCode::InvalidJson => StatusCode::BAD_REQUEST,
        ApiCode::SignInRequired | ApiCode::SessionExpired => StatusCode::UNAUTHORIZED,
        ApiCode::OriginRejected | ApiCode::CsrfRejected => StatusCode::FORBIDDEN,
        ApiCode::NotFound
        | ApiCode::ActionNotFound
        | ApiCode::ServerNotFound
        | ApiCode::FileUnavailable => StatusCode::NOT_FOUND,
        ApiCode::IdempotencyKeyReused
        | ApiCode::ContextChanged
        | ApiCode::CaseNumbersExhausted
        | ApiCode::ModuleNotReady(_) => StatusCode::CONFLICT,
        ApiCode::BodyTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        ApiCode::UnsupportedMediaType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ApiCode::IdempotencyKeyRequired => StatusCode::PRECONDITION_REQUIRED,
        ApiCode::RateLimited(_) => StatusCode::TOO_MANY_REQUESTS,
        ApiCode::DiscordUnavailable => StatusCode::BAD_GATEWAY,
        ApiCode::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        ApiCode::InternalError => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

impl ApiError {
    /// A transport-level error with its `quack-text` message.
    pub fn api(code: ApiCode) -> Self {
        let retry_after = match code {
            ApiCode::RateLimited(seconds) => Some(seconds.max(1)),
            _ => None,
        };
        Self {
            status: status_of(code),
            code: code.code(),
            message: plain(&code.message()),
            fields: Vec::new(),
            problems: Vec::new(),
            retry_after,
        }
    }

    /// A domain refusal, worded with the facts the caller knows.
    pub fn domain(err: &DomainError, facts: &ErrorFacts<'_>) -> Self {
        let status = StatusCode::from_u16(err.http_status()).unwrap_or(StatusCode::CONFLICT);
        let message = plain(&for_domain(err, facts));
        let fields = match err {
            DomainError::MemberRefused(refusal) => vec![FieldError {
                field: "member_id".to_owned(),
                code: refusal.code().as_str().to_owned(),
                message: message.clone(),
            }],
            _ => Vec::new(),
        };
        Self {
            status,
            code: err.code(),
            message,
            fields,
            problems: Vec::new(),
            retry_after: None,
        }
    }

    /// `400 validation_failed` with every field problem.
    pub fn validation(fields: Vec<FieldError>) -> Self {
        Self {
            fields,
            ..Self::api(ApiCode::ValidationFailed)
        }
    }

    /// `400 validation_failed` for one field.
    pub fn field(field: impl Into<String>, code: impl Into<String>, message: String) -> Self {
        Self::validation(vec![FieldError {
            field: field.into(),
            code: code.into(),
            message,
        }])
    }

    /// `409 module_not_ready` with what needs fixing.
    pub fn module_not_ready(module: ModuleName, problems: Vec<Problem>) -> Self {
        Self {
            problems,
            ..Self::api(ApiCode::ModuleNotReady(module))
        }
    }

    /// `500 internal_error`.
    pub fn internal() -> Self {
        Self::api(ApiCode::InternalError)
    }

    /// `404 not_found`.
    pub fn not_found() -> Self {
        Self::api(ApiCode::NotFound)
    }

    /// The HTTP status.
    pub fn status(&self) -> StatusCode {
        self.status
    }

    /// The machine code.
    pub fn code(&self) -> &'static str {
        self.code
    }

    /// The envelope as it is sent, with the current trace's ID.
    pub fn envelope(&self) -> ErrorEnvelope {
        ErrorEnvelope {
            error: ErrorBody {
                code: self.code.to_owned(),
                message: self.message.clone(),
                trace_id: crate::middleware::telemetry::current_trace_id(),
                fields: self.fields.clone(),
                problems: self.problems.clone(),
                retry_after_seconds: self.retry_after,
            },
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = (self.status, Json(self.envelope())).into_response();
        if let Some(seconds) = self.retry_after {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from(seconds));
        }
        if self.status.is_server_error() {
            tracing::warn!(
                code = self.code,
                status = self.status.as_u16(),
                "request failed"
            );
        }
        response
    }
}

impl From<quack_db::DbError> for ApiError {
    fn from(error: quack_db::DbError) -> Self {
        from_db(&error)
    }
}

/// A handler result.
pub(crate) type ApiResult<T> = Result<T, ApiError>;
