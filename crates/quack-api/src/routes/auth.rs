//! Sign-in, the signed-in account, and sign-out (routes 1–4, §14.4).

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use quack_core::ids::AccountId;
use quack_db::{accounts, sessions};
use quack_telemetry::labels::SignInOutcome;
use quack_telemetry::metrics;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::auth::cookies::Cookie;
use crate::auth::crypto;
use crate::auth::redirect;
use crate::error::{ApiError, ApiResult, ErrorEnvelope};
use crate::extract::SessionAccount;
use crate::state::ApiState;
use crate::wire::{avatar_url, schema, ts};

/// The longest session (§14.4 step 3).
const MAX_SESSION: time::Duration = time::Duration::days(7);
/// How long a sign-in may take.
const OAUTH_COOKIE_SECONDS: i64 = 600;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(login))
        .routes(routes!(callback))
        .routes(routes!(me))
        .routes(routes!(logout))
}

/// Route 1's query.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct LoginQuery {
    /// Where to land after signing in: a path on this dashboard (anything
    /// else lands on `/guilds`).
    redirect_to: Option<String>,
}

/// Route 2's query, from Discord.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct CallbackQuery {
    /// The authorization code.
    code: Option<String>,
    /// The sign-in's state.
    state: Option<String>,
    /// Set when the person cancelled.
    error: Option<String>,
}

/// The signed-in account (route 3).
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct Me {
    /// Who is signed in.
    account: MeAccount,
    /// Send as `X-CSRF-Token` on every change.
    csrf_token: String,
    /// The session.
    session: MeSession,
}

/// The signed-in Discord account.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct MeAccount {
    #[schema(value_type = schema::Snowflake)]
    id: AccountId,
    username: String,
    global_name: Option<String>,
    avatar_url: String,
}

/// The session's lifetime.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct MeSession {
    /// When it ends; sign in again after.
    #[serde(with = "ts")]
    #[schema(value_type = String, format = DateTime)]
    expires_at: OffsetDateTime,
}

fn redirect_to(location: &str) -> Response {
    let mut response = StatusCode::FOUND.into_response();
    if let Ok(value) = HeaderValue::from_str(location) {
        response.headers_mut().insert(header::LOCATION, value);
    }
    response
}

/// Starts Discord sign-in.
#[utoipa::path(get, path = "/api/auth/login", tag = "auth", params(LoginQuery),
    responses((status = 302, description = "To Discord's authorize page"),
              (status = 429, body = ErrorEnvelope)))]
async fn login(
    State(state): State<ApiState>,
    Query(query): Query<LoginQuery>,
) -> ApiResult<Response> {
    let public = &state.config().public_url;
    let landing = redirect::landing(public, query.redirect_to.as_deref());
    let (Some(login_state), Some(nonce), Some(verifier)) =
        (crypto::token(), crypto::token(), crypto::pkce_verifier())
    else {
        return Err(ApiError::internal());
    };
    let (Some(state_hash), Some(nonce_hash)) =
        (crypto::token_hash(&login_state), crypto::token_hash(&nonce))
    else {
        return Err(ApiError::internal());
    };
    let mut conn = state.engine().db().conn().await?;
    sessions::create_oauth_state(
        &mut conn,
        &state_hash,
        &nonce_hash,
        &sessions::OauthState {
            code_verifier: verifier.clone(),
            redirect_to: landing,
        },
    )
    .await?;
    let mut response = redirect_to(&state.oauth().authorize_url(&login_state, &verifier));
    if let Some(cookie) = Cookie::Oauth.set(public, &nonce, OAUTH_COOKIE_SECONDS) {
        response.headers_mut().append(header::SET_COOKIE, cookie);
    }
    Ok(response)
}

/// Where sign-in ends, with the outcome counted.
fn finish(state: &ApiState, outcome: SignInOutcome, location: &str) -> Response {
    metrics().http.sign_in(outcome);
    let mut response = redirect_to(location);
    if let Some(cookie) = Cookie::Oauth.clear(&state.config().public_url) {
        response.headers_mut().append(header::SET_COOKIE, cookie);
    }
    response
}

/// Discord's redirect back after sign-in.
#[utoipa::path(get, path = "/api/auth/callback", tag = "auth", params(CallbackQuery),
    responses((status = 302, description = "To the landing page, or `/login?error=failed|denied`")))]
async fn callback(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Query(query): Query<CallbackQuery>,
) -> Response {
    let public = &state.config().public_url;
    let nonce = Cookie::Oauth
        .read(public, &headers)
        .and_then(|n| crypto::token_hash(&n));
    let state_hash = query.state.as_deref().and_then(crypto::token_hash);
    let (Some(nonce), Some(state_hash)) = (nonce, state_hash) else {
        return finish(&state, SignInOutcome::Refused, "/login?error=failed");
    };
    let consumed = async {
        let mut conn = state.engine().db().conn().await?;
        sessions::consume_oauth_state(&mut conn, &state_hash, &nonce).await
    }
    .await;
    let login = match consumed {
        Ok(Some(login)) => login,
        Ok(None) => return finish(&state, SignInOutcome::Refused, "/login?error=failed"),
        Err(_) => return finish(&state, SignInOutcome::Failed, "/login?error=failed"),
    };
    if query.error.is_some() {
        return finish(&state, SignInOutcome::Cancelled, "/login?error=denied");
    }
    let Some(code) = query.code else {
        return finish(&state, SignInOutcome::Failed, "/login?error=failed");
    };
    match sign_in(&state, &headers, &code, &login.code_verifier).await {
        Some((cookie, max_age)) => {
            let mut response = finish(&state, SignInOutcome::Done, &login.redirect_to);
            if let Some(value) = Cookie::Session.set(public, &cookie, max_age) {
                response.headers_mut().append(header::SET_COOKIE, value);
            }
            response
        }
        None => finish(&state, SignInOutcome::Failed, "/login?error=failed"),
    }
}

/// Exchanges the code, reads the account, and creates the session in one
/// transaction with the 2FA record (replacing the browser's previous
/// session). Returns the session cookie and its max age.
async fn sign_in(
    state: &ApiState,
    headers: &HeaderMap,
    code: &str,
    verifier: &str,
) -> Option<(String, i64)> {
    let grant = state.oauth().exchange(code, verifier).await.ok()?;
    let user = state.oauth().current_user(&grant.access_token).await.ok()?;
    let account: AccountId = user.id.parse().ok()?;
    let lifetime = grant
        .expires_in
        .and_then(|d| time::Duration::try_from(d).ok())
        .map_or(MAX_SESSION, |d| d.min(MAX_SESSION));
    let expires_at = OffsetDateTime::now_utc() + lifetime;
    let session_id = crypto::token()?;
    let id_hash = crypto::token_hash(&session_id)?;
    let sealed = crypto::seal(
        &state.config().session_secret,
        &id_hash,
        account,
        &grant.access_token,
    )?;
    let previous = Cookie::Session
        .read(&state.config().public_url, headers)
        .and_then(|c| crypto::token_hash(&c));
    let mut tx = state.engine().db().begin().await.ok()?;
    accounts::upsert(&mut tx, account, user.mfa_enabled)
        .await
        .ok()?;
    sessions::create(
        &mut tx,
        &sessions::NewSession {
            id_hash,
            account,
            csrf_token: crypto::token()?,
            discord_token_ciphertext: sealed,
            key_fingerprint: *state.fingerprint(),
            expires_at,
        },
    )
    .await
    .ok()?;
    if let Some(previous) = previous {
        sessions::delete(&mut tx, &previous).await.ok()?;
    }
    tx.commit().await.ok()?;
    Some((session_id, lifetime.whole_seconds()))
}

/// The signed-in account and the CSRF token.
#[utoipa::path(get, path = "/api/auth/me", tag = "auth",
    responses((status = 200, body = Me), (status = 401, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn me(State(state): State<ApiState>, session: SessionAccount) -> ApiResult<Json<Me>> {
    let account = session.account;
    let facts = match state.engine().discord().accounts(&[account]).await {
        quack_engine::Outcome::Done(found) => found.into_iter().find(|a| a.id == account),
        _ => None,
    };
    let (username, global_name, avatar) = facts.map_or_else(
        || (account.to_string(), None, None),
        |f| (f.username, f.global_name, f.avatar_hash),
    );
    Ok(Json(Me {
        account: MeAccount {
            id: account,
            username,
            global_name,
            avatar_url: avatar_url(account.get(), avatar.as_deref()),
        },
        csrf_token: session.session.csrf_token.clone(),
        session: MeSession {
            expires_at: session.expires_at,
        },
    }))
}

/// Signs out: deletes the session and clears its cookie.
#[utoipa::path(post, path = "/api/auth/logout", tag = "auth",
    responses((status = 204, description = "Signed out"),
              (status = 401, body = ErrorEnvelope), (status = 403, body = ErrorEnvelope)),
    security(("session" = [], "csrf" = [])))]
async fn logout(State(state): State<ApiState>, session: SessionAccount) -> ApiResult<Response> {
    let mut conn = state.engine().db().conn().await?;
    sessions::delete(&mut conn, &session.session.id_hash).await?;
    state.account_guilds().forget(&session.session.id_hash);
    let mut response = StatusCode::NO_CONTENT.into_response();
    if let Some(cookie) = Cookie::Session.clear(&state.config().public_url) {
        response.headers_mut().append(header::SET_COOKIE, cookie);
    }
    Ok(response)
}
