//! Keyset pagination (§14.1): `limit` 1–100 (default 50) and an opaque
//! `cursor`, the base64url of the last row's sort key plus a hash of the
//! filters it was made under. A cursor used with other filters is
//! `400 validation_failed`. A page is full when it has `limit` rows; then
//! `next_cursor` points after its last row (the next page may be empty).

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use quack_core::limits::{DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE};
use quack_db::TimeCursor;
use quack_text::render::plain;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::auth::crypto::sha256;
use crate::error::ApiError;

/// The page size a request asks for.
pub(crate) fn limit(requested: Option<u32>) -> Result<u32, ApiError> {
    limit_within(requested, DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE)
}

/// A page size within `1..=max`, `default` when absent.
pub(crate) fn limit_within(
    requested: Option<u32>,
    default: u32,
    max: u32,
) -> Result<u32, ApiError> {
    match requested {
        None => Ok(default),
        Some(n) if (1..=max).contains(&n) => Ok(n),
        Some(_) => Err(invalid("limit", "limit_range")),
    }
}

fn invalid(field: &str, code: &str) -> ApiError {
    ApiError::field(field, code, plain(&quack_text::api::validation_failed()))
}

#[derive(Serialize, Deserialize)]
struct Raw {
    /// The filters' hash.
    f: String,
    /// A case number key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    n: Option<u64>,
    /// A creation time key, in Unix nanoseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    t: Option<i128>,
    /// The row ID that breaks creation-time ties.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    i: Option<Uuid>,
}

fn filter_hash(filters: &str) -> String {
    let digest = sha256(filters.as_bytes());
    URL_SAFE_NO_PAD.encode(&digest[..9])
}

fn encode(raw: &Raw) -> String {
    URL_SAFE_NO_PAD.encode(serde_json::to_vec(raw).unwrap_or_default())
}

fn decode(cursor: &str, filters: &str) -> Result<Raw, ApiError> {
    let raw: Raw = URL_SAFE_NO_PAD
        .decode(cursor.as_bytes())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .ok_or_else(|| invalid("cursor", "cursor_invalid"))?;
    if raw.f == filter_hash(filters) {
        Ok(raw)
    } else {
        Err(invalid("cursor", "cursor_filters_changed"))
    }
}

/// A cursor after case number `number`, for these filters.
pub(crate) fn number_cursor(number: u64, filters: &str) -> String {
    encode(&Raw {
        f: filter_hash(filters),
        n: Some(number),
        t: None,
        i: None,
    })
}

/// The case number a cursor continues after.
pub(crate) fn decode_number(cursor: Option<&str>, filters: &str) -> Result<Option<u64>, ApiError> {
    let Some(cursor) = cursor else {
        return Ok(None);
    };
    decode(cursor, filters)?
        .n
        .map(Some)
        .ok_or_else(|| invalid("cursor", "cursor_invalid"))
}

/// A cursor after the row created at `created_at` with ID `id`.
pub(crate) fn time_cursor(created_at: OffsetDateTime, id: Uuid, filters: &str) -> String {
    encode(&Raw {
        f: filter_hash(filters),
        n: None,
        t: Some(created_at.unix_timestamp_nanos()),
        i: Some(id),
    })
}

/// The row a time cursor continues after.
pub(crate) fn decode_time(
    cursor: Option<&str>,
    filters: &str,
) -> Result<Option<TimeCursor>, ApiError> {
    let Some(cursor) = cursor else {
        return Ok(None);
    };
    let raw = decode(cursor, filters)?;
    let created_at = raw
        .t
        .and_then(|t| OffsetDateTime::from_unix_timestamp_nanos(t).ok());
    match (created_at, raw.i) {
        (Some(created_at), Some(id)) => Ok(Some(TimeCursor { created_at, id })),
        _ => Err(invalid("cursor", "cursor_invalid")),
    }
}

/// The next cursor when a page came back full.
pub(crate) fn next<T>(rows: &[T], limit: u32, cursor: impl Fn(&T) -> String) -> Option<String> {
    if rows.len() == usize::try_from(limit).unwrap_or(usize::MAX) {
        rows.last().map(cursor)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursors_round_trip_and_bind_their_filters() {
        let c = number_cursor(42, "state=valid");
        assert_eq!(decode_number(Some(&c), "state=valid").ok(), Some(Some(42)));
        assert_eq!(
            decode_number(Some(&c), "state=voided").map_err(|e| e.code()),
            Err("validation_failed")
        );
        assert!(decode_number(Some("garbage!"), "").is_err());
        let at = OffsetDateTime::from_unix_timestamp_nanos(1_700_000_000_123_456_000)
            .unwrap_or(OffsetDateTime::UNIX_EPOCH);
        let id = Uuid::now_v7();
        let t = time_cursor(at, id, "");
        assert_eq!(
            decode_time(Some(&t), "").ok().flatten(),
            Some(TimeCursor { created_at: at, id })
        );
    }

    #[test]
    fn limits_are_bounded() {
        assert_eq!(limit(None).ok(), Some(50));
        assert_eq!(limit(Some(100)).ok(), Some(100));
        assert!(limit(Some(0)).is_err());
        assert!(limit(Some(101)).is_err());
    }
}
