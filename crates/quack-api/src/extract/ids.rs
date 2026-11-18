//! Parsing IDs from paths and queries (§14.1): snowflakes by the one
//! checked parser (`400 validation_failed` when they don't fit), record
//! IDs as UUIDs or legacy ULIDs (`404 not_found` otherwise: an ID that
//! can't exist names nothing).

use std::str::FromStr;

use quack_core::ids::{CaseId, ulid_to_uuid};
use quack_engine::CaseRef;
use quack_text::render::plain;
use uuid::Uuid;

use crate::error::ApiError;

/// A Discord ID in `field`.
pub(crate) fn snowflake<T: FromStr>(text: &str, field: &str) -> Result<T, ApiError> {
    // The core parser trims; a path or query value with spaces is still
    // not a snowflake.
    if text.trim() != text {
        return Err(invalid(field));
    }
    text.parse().map_err(|_| invalid(field))
}

fn invalid(field: &str) -> ApiError {
    ApiError::field(
        field,
        "invalid_id",
        plain(&quack_text::api::validation_failed()),
    )
}

/// A record ID: a UUID, or a legacy ULID mapped as the importer did.
pub(crate) fn uuid_id<T: From<Uuid>>(text: &str) -> Result<T, ApiError> {
    Uuid::try_parse(text)
        .or_else(|_| ulid_to_uuid(text))
        .map(T::from)
        .map_err(|_| ApiError::not_found())
}

/// A staff `{case_ref}`: a case number, a case UUID, or a legacy ULID.
pub(crate) fn case_ref(text: &str) -> Result<CaseRef, ApiError> {
    if !text.is_empty() && text.len() <= 16 && text.bytes().all(|b| b.is_ascii_digit()) {
        return text
            .parse::<u64>()
            .ok()
            .filter(|n| (1..=quack_core::limits::MAX_CASE_NUMBER).contains(n))
            .map(CaseRef::Number)
            .ok_or_else(ApiError::not_found);
    }
    uuid_id::<CaseId>(text).map(CaseRef::Id)
}

/// A member route's `{case_id}`: a UUID or legacy ULID, never a number.
pub(crate) fn member_case_id(text: &str) -> Result<CaseId, ApiError> {
    uuid_id(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use quack_core::ids::GuildId;

    #[test]
    fn snowflakes_and_refs() {
        assert!(snowflake::<GuildId>("1001", "guild_id").is_ok());
        for bad in ["0", "-1", "9223372036854775808", " 1", "abc", ""] {
            assert_eq!(
                snowflake::<GuildId>(bad, "guild_id").map_err(|e| e.code()),
                Err("validation_failed"),
                "{bad}"
            );
        }
        assert_eq!(case_ref("12").ok(), Some(CaseRef::Number(12)));
        assert!(case_ref("0").is_err());
        assert!(case_ref("01ARZ3NDEKTSV4RRFFQ69G5FAV").is_ok());
        assert!(case_ref("0190f5b2-6c1e-7cc0-9a3e-000000000001").is_ok());
        assert!(member_case_id("12").is_err());
    }
}
