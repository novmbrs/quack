//! Operation keys (§4.3): idempotency stored with the write it protects.
//!
//! A state-changing command reserves its key first in its transaction
//! ([`reserve`]), stores its result reference before commit ([`complete`]),
//! and a replay finds it with [`lookup`]. A concurrent request with the same
//! key blocks on the reservation until the first commits.

use std::time::Duration;

use quack_core::ids::{AccountId, GuildId};
use sqlx::PgConnection;
use time::OffsetDateTime;

use crate::DbError;
use crate::obs::{self, Insert, Select, Update};
use crate::pg::count;

/// How long a key is kept (housekeeping deletes it after).
pub const OPERATION_KEY_TTL: Duration = Duration::from_hours(24);

/// A scoped operation key.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct OperationKey {
    /// The server, or `None` for DMs and `/api/me` routes (stored as 0).
    pub guild: Option<GuildId>,
    /// The account acting, or `None` for Quack (stored as 0).
    pub account: Option<AccountId>,
    /// The operation (command name, custom-ID code, or `METHOD /route`).
    pub operation: String,
    /// The key (interaction ID, `draft:<id>`, `Idempotency-Key`, …).
    pub key: String,
}

impl OperationKey {
    fn parts(&self) -> (i64, i64) {
        (
            self.guild.map_or(0, i64::from),
            self.account.map_or(0, i64::from),
        )
    }

    /// The key as stored on a case (`<account_id>:<operation>:<key>`),
    /// which protects case creation forever.
    pub fn case_scoped(&self) -> String {
        format!(
            "{}:{}:{}",
            self.account.map_or(0, AccountId::get),
            self.operation,
            self.key
        )
    }
}

/// A stored operation and its result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredOperation {
    /// SHA-256 of the canonical request.
    pub request_hash: [u8; 32],
    /// The original status code; 0 while the reserving transaction has not
    /// stored its result (never visible after its commit).
    pub status: u16,
    /// The result reference, e.g. `{"case_id": "…"}`.
    pub response: serde_json::Value,
    /// When the key expires.
    pub expires_at: OffsetDateTime,
}

/// Reserves `key` in the caller's transaction (§4.2 step 7). `false` means
/// another request with the key committed first: roll back and replay.
pub async fn reserve(
    conn: &mut PgConnection,
    key: &OperationKey,
    request_hash: &[u8; 32],
) -> Result<bool, DbError> {
    let (guild, account) = key.parts();
    let ttl = OPERATION_KEY_TTL.as_secs_f64();
    let done = obs::run(
        Insert,
        "operation_keys",
        sqlx::query!(
            "INSERT INTO operation_keys (guild_id, account_id, operation, key, request_hash, response_status,
                                         response, expires_at)
             VALUES ($1, $2, $3, $4, $5, 0, '{}', now() + make_interval(secs => $6))
             ON CONFLICT DO NOTHING",
            guild,
            account,
            key.operation,
            key.key,
            &request_hash[..],
            ttl,
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Stores the result of the reserved operation before commit.
pub async fn complete(
    conn: &mut PgConnection,
    key: &OperationKey,
    status: u16,
    response: &serde_json::Value,
) -> Result<(), DbError> {
    let (guild, account) = key.parts();
    let done = obs::run(
        Update,
        "operation_keys",
        sqlx::query!(
            "UPDATE operation_keys SET response_status = $5, response = $6
             WHERE guild_id = $1 AND account_id = $2 AND operation = $3 AND key = $4",
            guild,
            account,
            key.operation,
            key.key,
            i16::try_from(status).map_err(|_| DbError::Invalid(format!("status {status}")))?,
            response,
        )
        .execute(conn),
    )
    .await?;
    if done.rows_affected() == 1 {
        Ok(())
    } else {
        Err(DbError::NotFound)
    }
}

/// Finds an unexpired stored operation (§4.3 replay check).
pub async fn lookup(
    conn: &mut PgConnection,
    key: &OperationKey,
) -> Result<Option<StoredOperation>, DbError> {
    let (guild, account) = key.parts();
    let row = obs::run(
        Select,
        "operation_keys",
        sqlx::query!(
            "SELECT request_hash, response_status, response, expires_at FROM operation_keys
             WHERE guild_id = $1 AND account_id = $2 AND operation = $3 AND key = $4 AND expires_at > now()",
            guild,
            account,
            key.operation,
            key.key,
        )
        .fetch_optional(conn),
    )
    .await?;
    row.map(|r| {
        Ok(StoredOperation {
            request_hash: r
                .request_hash
                .try_into()
                .map_err(|_| DbError::Decode("request_hash".to_owned()))?,
            status: count(r.response_status)?,
            response: r.response,
            expires_at: r.expires_at,
        })
    })
    .transpose()
}
