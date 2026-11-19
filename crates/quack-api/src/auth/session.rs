//! Finding the request's session (§14.4 step 3): one cookie, hashed, one
//! indexed lookup under the current secret's fingerprint. The result is
//! resolved once per request and shared by the CSRF check, the rate
//! limiter, and the extractors, so they can never disagree about who is
//! asking.

use axum::http::HeaderMap;
use quack_core::ids::AccountId;
use quack_db::sessions;
use time::OffsetDateTime;

use super::cookies::Cookie;
use super::crypto;
use crate::error::ApiError;
use crate::state::ApiState;

/// A live session.
#[derive(Clone, Debug)]
pub(crate) struct LiveSession {
    /// SHA-256 of the session ID.
    pub id_hash: [u8; 32],
    /// The signed-in account.
    pub account: AccountId,
    /// The CSRF token.
    pub csrf_token: String,
    /// The sealed Discord access token.
    pub sealed_token: Vec<u8>,
    /// When the session ends.
    pub expires_at: OffsetDateTime,
}

/// What the request's cookie led to.
#[derive(Clone, Debug)]
pub(crate) enum SessionLookup {
    /// No session cookie.
    Anonymous,
    /// A cookie whose session ended, was signed out, or was sealed under
    /// another secret.
    Ended,
    /// A live session.
    Live(LiveSession),
}

impl SessionLookup {
    /// The live session, or the 401 a route needing one answers.
    pub(crate) fn require(&self) -> Result<&LiveSession, ApiError> {
        match self {
            Self::Anonymous => Err(ApiError::api(quack_text::api::ApiCode::SignInRequired)),
            Self::Ended => Err(ApiError::api(quack_text::api::ApiCode::SessionExpired)),
            Self::Live(session) => Ok(session),
        }
    }

    /// The signed-in account, if any.
    pub(crate) fn account(&self) -> Option<AccountId> {
        match self {
            Self::Live(session) => Some(session.account),
            Self::Anonymous | Self::Ended => None,
        }
    }
}

/// How often `last_seen_at` is refreshed.
const TOUCH_AFTER: time::Duration = time::Duration::minutes(5);

/// Resolves the session the request's cookie names.
pub(crate) async fn lookup(
    state: &ApiState,
    headers: &HeaderMap,
) -> Result<SessionLookup, ApiError> {
    let Some(cookie) = Cookie::Session.read(&state.config().public_url, headers) else {
        return Ok(SessionLookup::Anonymous);
    };
    let Some(id_hash) = crypto::token_hash(&cookie) else {
        return Ok(SessionLookup::Ended);
    };
    let mut conn = state.engine().db().conn().await?;
    let Some(session) = sessions::get(&mut conn, &id_hash, state.fingerprint()).await? else {
        return Ok(SessionLookup::Ended);
    };
    if session.last_seen_at < OffsetDateTime::now_utc() - TOUCH_AFTER {
        sessions::touch(&mut conn, &id_hash).await?;
    }
    Ok(SessionLookup::Live(LiveSession {
        id_hash,
        account: session.account,
        csrf_token: session.csrf_token,
        sealed_token: session.discord_token_ciphertext,
        expires_at: session.expires_at,
    }))
}

impl LiveSession {
    /// The session's Discord access token, opened with the current secret.
    pub(crate) fn access_token(&self, state: &ApiState) -> Option<String> {
        crypto::open(
            &state.config().session_secret,
            &self.id_hash,
            self.account,
            &self.sealed_token,
        )
    }
}
