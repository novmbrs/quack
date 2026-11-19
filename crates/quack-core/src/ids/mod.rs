//! Typed ids.
//!
//! Discord snowflakes (`GuildId`, `AccountId`, `MemberId`, `ChannelId`,
//! `RoleId`, `MessageId`) are positive integers below 2^63: `bigint` in SQL,
//! decimal **strings** in JSON. Quack's own records use `UUIDv7` ids, generated
//! by the caller (this crate has no clock or randomness) and written in
//! custom ids and nonces as 22 characters of unpadded base64url (`u22`).

mod encoding;
mod snowflake;
mod uuids;

pub use encoding::{from_u22, to_u22, ulid_to_uuid};
pub use snowflake::{AccountId, AttachmentId, ChannelId, GuildId, MemberId, MessageId, RoleId};
pub use uuids::{
    ActionId, AppealEventId, AppealId, CaseId, DmId, EvidenceId, FileId, JobId, MessageRowId,
    ModLogId, RuleId, TicketId,
};

/// An id string or integer that is not a valid id.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum IdError {
    /// Not a decimal snowflake of 1–20 digits in `1..=i64::MAX`.
    #[error("not a Discord id")]
    NotASnowflake,
    /// Not 22 characters of canonical unpadded base64url.
    #[error("not a u22 id")]
    NotU22,
    /// Not a 26-character Crockford base32 ULID.
    #[error("not a ULID")]
    NotUlid,
}
