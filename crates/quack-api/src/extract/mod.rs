//! Extractors: who is asking ([`SessionAccount`], [`Staff`],
//! [`MemberSelf`]), the [`IdempotencyKey`] of a write, path IDs, and the
//! strict JSON body. Each fails with an [`ApiError`].

mod body;
mod ids;
mod staff;

use axum::extract::{FromRequestParts, MatchedPath};
use axum::http::request::Parts;
use quack_core::ids::AccountId;
use quack_engine::OpKey;
use quack_text::api::ApiCode;
use time::OffsetDateTime;

use crate::auth::session::{LiveSession, SessionLookup};
use crate::error::ApiError;
use crate::state::ApiState;

pub(crate) use body::Body;
pub(crate) use ids::{case_ref, member_case_id, snowflake, uuid_id};
pub use staff::Staff;
pub use staff::{InServer, ManageRules, ManageSettings, Moderate, ReadRules, Requirement};

/// The signed-in account (any route that needs a session).
#[derive(Clone, Debug)]
pub struct SessionAccount {
    /// The account.
    pub account: AccountId,
    /// When the session ends.
    pub expires_at: OffsetDateTime,
    pub(crate) session: LiveSession,
}

impl FromRequestParts<ApiState> for SessionAccount {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &ApiState) -> Result<Self, ApiError> {
        let lookup = match parts.extensions.get::<SessionLookup>() {
            Some(lookup) => lookup.clone(),
            None => crate::auth::session::lookup(state, &parts.headers).await?,
        };
        let session = lookup.require()?.clone();
        Ok(Self {
            account: session.account,
            expires_at: session.expires_at,
            session,
        })
    }
}

/// The signed-in account acting as the member a case is about (routes
/// 51–55). Any signed-in account may ask; each route finds only that
/// account's own cases.
#[derive(Clone, Debug)]
pub struct MemberSelf {
    /// The account.
    pub account: AccountId,
}

impl FromRequestParts<ApiState> for MemberSelf {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &ApiState) -> Result<Self, ApiError> {
        let session = SessionAccount::from_request_parts(parts, state).await?;
        Ok(Self {
            account: session.account,
        })
    }
}

/// The `Idempotency-Key` of a write (§4.3, §14.1): 8–200 printable ASCII
/// characters, scoped by the engine to the server, the account, and the
/// operation (`METHOD /route`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdempotencyKey(pub OpKey);

/// The header's name.
pub(crate) const IDEMPOTENCY_HEADER: &str = "idempotency-key";

impl IdempotencyKey {
    /// Reads and checks the header.
    fn read(parts: &Parts) -> Result<Self, ApiError> {
        let Some(value) = parts.headers.get(IDEMPOTENCY_HEADER) else {
            return Err(ApiError::api(ApiCode::IdempotencyKeyRequired));
        };
        let limit = quack_core::limits::IDEMPOTENCY_KEY;
        let key = value
            .to_str()
            .ok()
            .filter(|k| (limit.min..=limit.max).contains(&k.len()))
            .filter(|k| k.bytes().all(|b| (0x20..=0x7e).contains(&b)))
            .ok_or_else(|| ApiError::api(ApiCode::IdempotencyKeyRequired))?;
        let route = parts
            .extensions
            .get::<MatchedPath>()
            .map_or("", MatchedPath::as_str);
        Ok(Self(OpKey::new(format!("{} {route}", parts.method), key)))
    }
}

impl<S: Send + Sync> FromRequestParts<S> for IdempotencyKey {
    type Rejection = ApiError;

    fn from_request_parts(
        parts: &mut Parts,
        _: &S,
    ) -> impl Future<Output = Result<Self, ApiError>> + Send {
        std::future::ready(Self::read(parts))
    }
}
