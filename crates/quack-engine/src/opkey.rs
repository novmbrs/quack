//! Operation keys (§4.3): idempotency stored with the write it protects.
//!
//! A use case authorizes first, then [`begin_keyed`] either replays a
//! stored result (same request hash), refuses a reused key (different
//! hash), or opens the write's transaction with the key reserved in it.
//! A concurrent request with the same key blocks on the reservation until
//! the first commits, then replays its result.

use quack_core::ids::{AccountId, GuildId};
use quack_db::operation_keys::{self, OperationKey};
use quack_db::{Db, DbError, Tx};
use serde::Serialize;

/// The caller's half of an operation key: what it is and the key itself.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct OpKey {
    /// The operation: a command name, custom-ID code, or `METHOD /route`.
    pub operation: String,
    /// The interaction ID, `draft:<id>`, `honeypot:<message>`, or the
    /// `Idempotency-Key` header.
    pub key: String,
}

impl OpKey {
    /// An operation key.
    pub fn new(operation: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            operation: operation.into(),
            key: key.into(),
        }
    }

    /// The key scoped to a server and account (`None` = Quack).
    pub fn scoped(&self, guild: Option<GuildId>, account: Option<AccountId>) -> OperationKey {
        OperationKey {
            guild,
            account,
            operation: self.operation.clone(),
            key: self.key.clone(),
        }
    }
}

/// SHA-256 of a request's canonical JSON (§4.3). Requests hash typed,
/// resolved values (ids, validated bodies), so the same request always
/// hashes the same.
pub fn request_hash<T: Serialize>(request: &T) -> [u8; 32] {
    let bytes = serde_json::to_vec(request).unwrap_or_default();
    let digest = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, &bytes);
    let mut out = [0_u8; 32];
    out.copy_from_slice(digest.as_ref());
    out
}

/// What [`begin_keyed`] found.
pub enum Keyed {
    /// The key is new: the transaction holds its reservation.
    Fresh(Tx),
    /// The same request already ran: its stored result reference.
    Replay(serde_json::Value),
}

/// Why an operation key was refused.
#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    /// The key was used for a different request.
    #[error("operation key reused")]
    Reused,
    /// The database failed.
    #[error(transparent)]
    Db(#[from] DbError),
}

/// Looks the key up, and when it is new, opens a transaction holding its
/// reservation (§4.2 steps 2 and 7).
pub async fn begin_keyed(db: &Db, key: &OperationKey, hash: &[u8; 32]) -> Result<Keyed, KeyError> {
    // A reservation lost to a concurrent request means that request
    // committed; the second lookup finds it.
    for _ in 0..3 {
        if let Some(stored) = lookup(db, key).await? {
            return if stored.request_hash == *hash {
                Ok(Keyed::Replay(stored.response))
            } else {
                Err(KeyError::Reused)
            };
        }
        let mut tx = db.begin().await?;
        if operation_keys::reserve(&mut tx, key, hash).await? {
            return Ok(Keyed::Fresh(tx));
        }
        tx.rollback().await?;
    }
    Err(KeyError::Db(DbError::Stale))
}

async fn lookup(
    db: &Db,
    key: &OperationKey,
) -> Result<Option<operation_keys::StoredOperation>, DbError> {
    let mut conn = db.conn().await?;
    operation_keys::lookup(&mut conn, key).await
}

/// Stores the result reference before commit.
pub async fn complete(
    tx: &mut Tx,
    key: &OperationKey,
    status: u16,
    response: serde_json::Value,
) -> Result<(), DbError> {
    operation_keys::complete(tx, key, status, &response).await
}

/// Reads an id field from a stored result reference.
pub(crate) fn stored_id<T: serde::de::DeserializeOwned>(
    response: &serde_json::Value,
    field: &str,
) -> Result<T, DbError> {
    response
        .get(field)
        .cloned()
        .and_then(|v| serde_json::from_value(v).ok())
        .ok_or_else(|| DbError::Decode(format!("operation result without {field}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_are_stable_and_distinct() {
        let a = request_hash(&serde_json::json!({"case": 1, "reason": "x"}));
        assert_eq!(
            a,
            request_hash(&serde_json::json!({"case": 1, "reason": "x"}))
        );
        assert_ne!(
            a,
            request_hash(&serde_json::json!({"case": 1, "reason": "y"}))
        );
    }
}
