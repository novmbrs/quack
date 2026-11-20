//! Mapping between Postgres values and quack-core types.
//!
//! Every schema enum is read and written through [`Pg<T>`], which encodes a
//! quack-core enum by its stable `snake_case` name. `sqlx.toml` maps each
//! Postgres enum type to `Pg<T>` for the query macros, so a query that names
//! an enum column gets the typed value, and binding the wrong enum fails to
//! compile. Snowflakes are `bigint` in SQL and are converted with [`id`] and
//! [`opt_id`], which reject values that are not snowflakes instead of
//! wrapping them.

use std::error::Error as StdError;
use std::fmt::Display;
use std::str::FromStr;

use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::postgres::{PgArgumentBuffer, PgHasArrayType, PgTypeInfo, PgValueRef};
use sqlx::{Decode, Encode, Postgres, Type};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::DbError;

/// A quack-core enum stored as the Postgres enum `PG_TYPE`.
pub(crate) trait PgEnum: Copy + FromStr + Send + Sync + 'static {
    /// The Postgres type name.
    const PG_TYPE: &'static str;
    /// The Postgres array type name (`_` + the type name).
    const PG_ARRAY_TYPE: &'static str;
    /// The value's stable name, equal to its Postgres label.
    fn label(self) -> &'static str;
}

/// A quack-core enum as a Postgres enum value. Internal to this crate: rows
/// handed to callers hold the plain enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Pg<T>(pub T);

impl<T: PgEnum> Type<Postgres> for Pg<T> {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::with_name(T::PG_TYPE)
    }
}

impl<T: PgEnum> PgHasArrayType for Pg<T> {
    fn array_type_info() -> PgTypeInfo {
        PgTypeInfo::with_name(T::PG_ARRAY_TYPE)
    }
}

impl<T: PgEnum> Encode<'_, Postgres> for Pg<T> {
    fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> Result<IsNull, BoxDynError> {
        <&str as Encode<Postgres>>::encode(self.0.label(), buf)
    }
}

impl<'r, T> Decode<'r, Postgres> for Pg<T>
where
    T: PgEnum,
    T::Err: StdError + Send + Sync + 'static,
{
    fn decode(value: PgValueRef<'r>) -> Result<Self, BoxDynError> {
        let label = <&str as Decode<Postgres>>::decode(value)?;
        Ok(Self(label.parse()?))
    }
}

macro_rules! pg_enums {
    ($($ty:path => $name:literal,)+) => {
        $(
            impl PgEnum for $ty {
                const PG_TYPE: &'static str = $name;
                const PG_ARRAY_TYPE: &'static str = concat!("_", $name);
                fn label(self) -> &'static str {
                    self.as_str()
                }
            }
        )+
    };
}

pg_enums! {
    quack_core::case::CaseSource => "case_source",
    quack_core::case::VoidCause => "void_cause",
    quack_core::action::ActionKind => "action_kind",
    quack_core::action::ActionState => "action_state",
    quack_core::action::ActionFailure => "action_failure",
    quack_core::action::ActionSkipReason => "action_skip_reason",
    quack_core::action::AttemptOutcome => "attempt_outcome",
    quack_core::permissions::DiscordPermission => "discord_permission",
    quack_core::dm::DmKind => "dm_kind",
    quack_core::dm::DmState => "dm_state",
    quack_core::dm::DmFailure => "dm_failure",
    quack_core::evidence::EvidenceKind => "evidence_kind",
    quack_core::evidence::EvidenceState => "evidence_state",
    quack_core::evidence::EvidenceFailure => "evidence_failure",
    quack_core::evidence::FileCopyState => "file_copy_state",
    quack_core::appeal::AppealState => "appeal_state",
    quack_core::appeal::AppealEventKind => "appeal_event_kind",
    quack_core::modlog::ModLogKind => "mod_log_kind",
    quack_core::modlog::ModLogResult => "mod_log_result",
    quack_core::modlog::ModLogSource => "mod_log_source",
    quack_core::messages::MessageKind => "message_kind",
    quack_core::messages::MessageState => "message_state",
    crate::jobs::JobKind => "job_kind",
    quack_core::modules::TicketState => "ticket_state",
    quack_core::modules::TicketCloseReason => "ticket_close_reason",
    quack_core::modules::TranscriptPartial => "transcript_partial",
    quack_core::modules::ModuleDisabledReason => "module_disabled_reason",
    quack_core::modules::ActivityEvent => "activity_event",
    quack_core::modules::HoneypotHitState => "honeypot_hit_state",
}

/// Converts a stored `bigint` into a snowflake newtype, failing with
/// [`DbError::Decode`] when the value is not a snowflake.
pub(crate) fn id<T>(value: i64) -> Result<T, DbError>
where
    T: TryFrom<i64>,
    T::Error: Display,
{
    T::try_from(value).map_err(|error| DbError::Decode(format!("bad snowflake: {error}")))
}

/// [`id`] for a nullable column.
pub(crate) fn opt_id<T>(value: Option<i64>) -> Result<Option<T>, DbError>
where
    T: TryFrom<i64>,
    T::Error: Display,
{
    value.map(id).transpose()
}

/// Converts a stored non-negative integer into an unsigned one.
pub(crate) fn count<T, U>(value: T) -> Result<U, DbError>
where
    U: TryFrom<T>,
    T: Copy + Display,
{
    U::try_from(value).map_err(|_| DbError::Decode(format!("out of range: {value}")))
}

/// Converts an unsigned value into the signed integer Postgres stores,
/// failing with [`DbError::Invalid`] when it doesn't fit.
pub(crate) fn int<T, U>(value: T) -> Result<U, DbError>
where
    U: TryFrom<T>,
    T: Copy + Display,
{
    U::try_from(value).map_err(|_| DbError::Invalid(format!("out of range: {value}")))
}

/// A keyset pagination cursor over `(created_at, id)`, newest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeCursor {
    /// The last row's creation time.
    pub created_at: OffsetDateTime,
    /// The last row's id.
    pub id: Uuid,
}

/// Unpacks an optional cursor into the two nullable bind values the list
/// queries take (`($n::timestamptz IS NULL OR (created_at, id) < ($n, $m))`).
pub(crate) fn cursor_parts(cursor: Option<TimeCursor>) -> (Option<OffsetDateTime>, Option<Uuid>) {
    cursor.map_or((None, None), |c| (Some(c.created_at), Some(c.id)))
}

/// The SQL `LIMIT` of a list page: the page size clamped to `1..=max`, plus
/// one row that tells [`Page::more`] whether another page follows.
pub(crate) fn fetch_limit(requested: u32, max: u32) -> i64 {
    i64::from(requested.clamp(1, max)) + 1
}

/// One page of a list, fetched with one extra row so that `more` is exact
/// and the last page is never followed by an empty one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page<T> {
    /// At most the requested number of rows.
    pub rows: Vec<T>,
    /// Another row follows the last one.
    pub more: bool,
}

impl<T> Page<T> {
    /// The page from rows fetched with [`fetch_limit`].
    pub(crate) fn of(mut rows: Vec<T>, requested: u32, max: u32) -> Self {
        let size = usize::try_from(requested.clamp(1, max)).unwrap_or(usize::MAX);
        let more = rows.len() > size;
        rows.truncate(size);
        Self { rows, more }
    }

    /// The same page with each row mapped.
    #[must_use]
    pub fn map<U>(self, f: impl FnMut(T) -> U) -> Page<U> {
        Page {
            rows: self.rows.into_iter().map(f).collect(),
            more: self.more,
        }
    }
}

/// Clamps a page size to `1..=max` and returns it as a SQL `LIMIT`.
pub(crate) fn limit(requested: u32, max: u32) -> i64 {
    i64::from(requested.clamp(1, max))
}
