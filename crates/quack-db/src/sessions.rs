//! Dashboard sessions and one-time OAuth login states (§14.4).
//!
//! Only hashes of session IDs, states, and nonces are stored; the caller
//! hashes. Expired rows are deleted by housekeeping.

use quack_core::ids::AccountId;
use sqlx::PgConnection;
use time::OffsetDateTime;

use crate::DbError;
use crate::obs::{self, Delete, Insert, Select, Update};
use crate::pg::id;

/// A new session row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewSession {
    /// SHA-256 of the session ID.
    pub id_hash: [u8; 32],
    /// The signed-in account.
    pub account: AccountId,
    /// The CSRF token (43 characters of base64url).
    pub csrf_token: String,
    /// The encrypted Discord access token.
    pub discord_token_ciphertext: Vec<u8>,
    /// Which session secret encrypted it (8 bytes).
    pub key_fingerprint: [u8; 8],
    /// When the session ends.
    pub expires_at: OffsetDateTime,
}

/// A live session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    /// The signed-in account.
    pub account: AccountId,
    /// The CSRF token.
    pub csrf_token: String,
    /// The encrypted Discord access token.
    pub discord_token_ciphertext: Vec<u8>,
    /// When it was created.
    pub created_at: OffsetDateTime,
    /// When it was last used (updated at most every 5 minutes).
    pub last_seen_at: OffsetDateTime,
    /// When it ends.
    pub expires_at: OffsetDateTime,
}

/// Creates a session.
pub async fn create(conn: &mut PgConnection, session: &NewSession) -> Result<(), DbError> {
    obs::run(
        Insert,
        "sessions",
        sqlx::query!(
            "INSERT INTO sessions (id_hash, account_id, csrf_token, discord_token_ciphertext, key_fingerprint, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6)",
            &session.id_hash[..],
            i64::from(session.account),
            session.csrf_token,
            session.discord_token_ciphertext,
            &session.key_fingerprint[..],
            session.expires_at,
        )
        .execute(conn),
    )
    .await?;
    Ok(())
}

/// The unexpired session with this hash under the current secret.
pub async fn get(
    conn: &mut PgConnection,
    id_hash: &[u8; 32],
    key_fingerprint: &[u8; 8],
) -> Result<Option<Session>, DbError> {
    let row = obs::run(
        Select,
        "sessions",
        sqlx::query!(
            "SELECT account_id, csrf_token, discord_token_ciphertext, created_at, last_seen_at, expires_at
             FROM sessions WHERE id_hash = $1 AND expires_at > now() AND key_fingerprint = $2",
            &id_hash[..],
            &key_fingerprint[..],
        )
        .fetch_optional(conn),
    )
    .await?;
    row.map(|r| {
        Ok(Session {
            account: id(r.account_id)?,
            csrf_token: r.csrf_token,
            discord_token_ciphertext: r.discord_token_ciphertext,
            created_at: r.created_at,
            last_seen_at: r.last_seen_at,
            expires_at: r.expires_at,
        })
    })
    .transpose()
}

/// Updates `last_seen_at` when it is more than 5 minutes old. Returns
/// whether it wrote.
pub async fn touch(conn: &mut PgConnection, id_hash: &[u8; 32]) -> Result<bool, DbError> {
    let done = obs::run(
        Update,
        "sessions",
        sqlx::query!(
            "UPDATE sessions SET last_seen_at = now()
             WHERE id_hash = $1 AND last_seen_at < now() - interval '5 minutes' AND expires_at > now()",
            &id_hash[..]
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Deletes a session (logout, or the browser's previous session at
/// sign-in). Returns whether it existed.
pub async fn delete(conn: &mut PgConnection, id_hash: &[u8; 32]) -> Result<bool, DbError> {
    let done = obs::run(
        Delete,
        "sessions",
        sqlx::query!("DELETE FROM sessions WHERE id_hash = $1", &id_hash[..]).execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Unexpired sessions, for the `quack.sessions.active` gauge.
pub async fn count_active(conn: &mut PgConnection) -> Result<u64, DbError> {
    let n = obs::run(
        Select,
        "sessions",
        sqlx::query_scalar!(r#"SELECT count(*) AS "n!" FROM sessions WHERE expires_at > now()"#)
            .fetch_one(conn),
    )
    .await?;
    Ok(u64::try_from(n).unwrap_or(0))
}

/// A one-time OAuth login state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OauthState {
    /// The PKCE verifier.
    pub code_verifier: String,
    /// Where to send the browser after sign-in (a path on Quack's origin).
    pub redirect_to: String,
}

/// Stores a login state for 10 minutes (`expires_at` from PostgreSQL).
pub async fn create_oauth_state(
    conn: &mut PgConnection,
    state_hash: &[u8; 32],
    nonce_hash: &[u8; 32],
    state: &OauthState,
) -> Result<(), DbError> {
    obs::run(
        Insert,
        "oauth_states",
        sqlx::query!(
            "INSERT INTO oauth_states (state_hash, code_verifier, redirect_to, nonce_hash, expires_at)
             VALUES ($1, $2, $3, $4, now() + interval '10 minutes')",
            &state_hash[..],
            state.code_verifier,
            state.redirect_to,
            &nonce_hash[..],
        )
        .execute(conn),
    )
    .await?;
    Ok(())
}

/// Consumes a login state, only for the browser that started it (matching
/// nonce) and only before it expires. `None` means sign-in must fail.
pub async fn consume_oauth_state(
    conn: &mut PgConnection,
    state_hash: &[u8; 32],
    nonce_hash: &[u8; 32],
) -> Result<Option<OauthState>, DbError> {
    let row = obs::run(
        Delete,
        "oauth_states",
        sqlx::query!(
            "DELETE FROM oauth_states WHERE state_hash = $1 AND nonce_hash = $2 AND expires_at > now()
             RETURNING code_verifier, redirect_to",
            &state_hash[..],
            &nonce_hash[..],
        )
        .fetch_optional(conn),
    )
    .await?;
    Ok(row.map(|r| OauthState {
        code_verifier: r.code_verifier,
        redirect_to: r.redirect_to,
    }))
}
