//! Reading and locking case rows.

use quack_core::action::ActionKind;
use quack_core::case::{CaseDecision, CaseSource, ContextValues, VoidCause};
use quack_core::ids::{CaseId, GuildId, MemberId, RuleId};
use quack_core::rule::LevelAction;
use sqlx::PgConnection;
use sqlx::types::Json;
use time::OffsetDateTime;

use super::{Case, CaseVoid};
use crate::DbError;
use crate::obs::{self, Select};
use crate::pg::{Pg, count, id, int, opt_id};

pub(crate) struct CaseRow {
    id: uuid::Uuid,
    guild_id: i64,
    number: i64,
    member_id: i64,
    member_joined_at: Option<OffsetDateTime>,
    source: Pg<CaseSource>,
    moderator_id: Option<i64>,
    rule_id: Option<uuid::Uuid>,
    rule_version: Option<i32>,
    case_count: Option<i32>,
    level_index: Option<i16>,
    level_from_case_count: Option<i32>,
    action_kind: Option<Pg<ActionKind>>,
    timeout_seconds: Option<i32>,
    ban_delete_message_seconds: Option<i32>,
    sends_dm: bool,
    context: Json<ContextValues>,
    context_revision: i32,
    operation_key: Option<String>,
    request_hash: Option<Vec<u8>>,
    replaces_case_id: Option<uuid::Uuid>,
    voided_at: Option<OffsetDateTime>,
    voided_by: Option<i64>,
    void_reason: Option<String>,
    void_cause: Option<Pg<VoidCause>>,
    imported: Option<serde_json::Value>,
    created_at: OffsetDateTime,
}

impl TryFrom<CaseRow> for Case {
    type Error = DbError;

    fn try_from(r: CaseRow) -> Result<Self, DbError> {
        let rule = match (r.rule_id, r.rule_version) {
            (Some(rule), Some(version)) => Some((RuleId::from_uuid(rule), count(version)?)),
            _ => None,
        };
        let action = match (
            r.action_kind.map(|k| k.0),
            r.timeout_seconds,
            r.ban_delete_message_seconds,
        ) {
            (None, ..) => None,
            (Some(ActionKind::Timeout), Some(s), _) => Some(LevelAction::Timeout {
                duration_seconds: count(s)?,
            }),
            (Some(ActionKind::Kick), ..) => Some(LevelAction::Kick),
            (Some(ActionKind::Ban), _, Some(s)) => Some(LevelAction::Ban {
                delete_message_seconds: count(s)?,
            }),
            _ => return Err(DbError::Decode("case action".to_owned())),
        };
        let decision = match (rule, r.case_count, r.level_index, r.level_from_case_count) {
            (Some((rule_id, rule_version)), Some(case_count), Some(level), Some(from)) => {
                Some(CaseDecision {
                    rule_id,
                    rule_version,
                    case_count: count(case_count)?,
                    level_index: count(level)?,
                    level_from_case_count: count(from)?,
                    action,
                    sends_dm: r.sends_dm,
                })
            }
            _ => None,
        };
        let void = match (r.voided_at, r.void_reason, r.void_cause) {
            (Some(at), Some(reason), Some(cause)) => Some(CaseVoid {
                at,
                by: opt_id(r.voided_by)?,
                reason,
                cause: cause.0,
            }),
            _ => None,
        };
        Ok(Self {
            id: CaseId::from_uuid(r.id),
            guild: id(r.guild_id)?,
            number: count(r.number)?,
            member: id(r.member_id)?,
            member_joined_at: r.member_joined_at,
            source: r.source.0,
            moderator: opt_id(r.moderator_id)?,
            rule,
            decision,
            context: r.context.0,
            context_revision: count(r.context_revision)?,
            operation_key: r.operation_key,
            request_hash: r
                .request_hash
                .map(|h| {
                    h.try_into()
                        .map_err(|_| DbError::Decode("request_hash".to_owned()))
                })
                .transpose()?,
            replaces: r.replaces_case_id.map(CaseId::from_uuid),
            void,
            imported: r.imported,
            created_at: r.created_at,
        })
    }
}

/// `query_as!` for [`CaseRow`] with the shared column list; `$tail` is the
/// rest of the statement after `FROM cases`.
macro_rules! query_case {
    ($tail:literal $(, $arg:expr)* $(,)?) => {
        ::sqlx::sqlx_macros::expand_query!(
            record = CaseRow,
            source = r#"SELECT id, guild_id, number, member_id, member_joined_at, source, moderator_id, rule_id,
                         rule_version, case_count, level_index, level_from_case_count, action_kind,
                         timeout_seconds, ban_delete_message_seconds, sends_dm,
                         context AS "context: Json<ContextValues>", context_revision, operation_key,
                         request_hash, replaces_case_id, voided_at, voided_by, void_reason, void_cause,
                         imported, created_at
                  FROM cases "# + $tail,
            args = [$($arg),*]
        )
    };
}

/// A case of this server by ID.
pub async fn get(
    conn: &mut PgConnection,
    guild: GuildId,
    case: CaseId,
) -> Result<Option<Case>, DbError> {
    let row = obs::run(
        Select,
        "cases",
        query_case!(
            "WHERE guild_id = $1 AND id = $2",
            i64::from(guild),
            case.get()
        )
        .fetch_optional(conn),
    )
    .await?;
    row.map(Case::try_from).transpose()
}

/// A case of this server by number.
pub async fn get_by_number(
    conn: &mut PgConnection,
    guild: GuildId,
    number: u64,
) -> Result<Option<Case>, DbError> {
    let row = obs::run(
        Select,
        "cases",
        query_case!(
            "WHERE guild_id = $1 AND number = $2",
            i64::from(guild),
            int::<u64, i64>(number)?
        )
        .fetch_optional(conn),
    )
    .await?;
    row.map(Case::try_from).transpose()
}

/// A case by ID, only when it is about `member` (member routes, §4.10).
pub async fn get_for_member(
    conn: &mut PgConnection,
    member: MemberId,
    case: CaseId,
) -> Result<Option<Case>, DbError> {
    let row = obs::run(
        Select,
        "cases",
        query_case!(
            "WHERE id = $1 AND member_id = $2",
            case.get(),
            i64::from(member)
        )
        .fetch_optional(conn),
    )
    .await?;
    row.map(Case::try_from).transpose()
}

/// Locks a case `FOR UPDATE`: the single lock point for everything that
/// changes it or its children (§4.12).
pub async fn lock(
    conn: &mut PgConnection,
    guild: GuildId,
    case: CaseId,
) -> Result<Option<Case>, DbError> {
    let row = obs::run(
        Select,
        "cases",
        query_case!(
            "WHERE guild_id = $1 AND id = $2 FOR UPDATE",
            i64::from(guild),
            case.get()
        )
        .fetch_optional(conn),
    )
    .await?;
    row.map(Case::try_from).transpose()
}

/// The case created with this scoped operation key, with its request hash
/// (replays after the key's 24 hours, §4.3).
pub async fn find_by_operation_key(
    conn: &mut PgConnection,
    guild: GuildId,
    scoped_key: &str,
) -> Result<Option<(CaseId, Option<[u8; 32]>)>, DbError> {
    let row = obs::run(
        Select,
        "cases",
        sqlx::query!(
            "SELECT id, request_hash FROM cases WHERE guild_id = $1 AND operation_key = $2",
            i64::from(guild),
            scoped_key
        )
        .fetch_optional(conn),
    )
    .await?;
    row.map(|r| {
        let hash = r
            .request_hash
            .map(|h| {
                h.try_into()
                    .map_err(|_| DbError::Decode("request_hash".to_owned()))
            })
            .transpose()?;
        Ok((CaseId::from_uuid(r.id), hash))
    })
    .transpose()
}
