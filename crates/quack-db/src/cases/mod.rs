//! Cases (§4): creation, the escalation count, locks, voids, context edits.

mod list;

use quack_core::action::ActionKind;
use quack_core::case::{CaseDecision, CaseSource, ContextValues, VoidCause};
use quack_core::ids::{AccountId, CaseId, GuildId, MemberId, RuleId};
use quack_core::rule::LevelAction;
use sqlx::PgConnection;
use sqlx::types::Json;
use time::OffsetDateTime;

use crate::DbError;
use crate::obs::{self, Insert, Select, Update};
use crate::pg::{Pg, count, id, int};

pub use list::{
    CaseFilter, CaseSummary, MemberCaseSummary, RuleTally, list, member_cases, member_summary,
};

/// A case row.
#[derive(Clone, Debug, PartialEq)]
pub struct Case {
    /// The case.
    pub id: CaseId,
    /// Its server.
    pub guild: GuildId,
    /// Its number in the server.
    pub number: u64,
    /// The member it is about.
    pub member: MemberId,
    /// The membership it is about (`joined_at`); `None` when imported or
    /// the member was absent.
    pub member_joined_at: Option<OffsetDateTime>,
    /// Where it came from.
    pub source: CaseSource,
    /// The moderator; `None` for honeypot and some imported cases.
    pub moderator: Option<AccountId>,
    /// The rule and version applied, when any (imported cases may lack one).
    pub rule: Option<(RuleId, u32)>,
    /// The decision; `None` for imported cases.
    pub decision: Option<CaseDecision>,
    /// Case context values.
    pub context: ContextValues,
    /// Incremented by every context edit.
    pub context_revision: u32,
    /// `<account_id>:<operation>:<key>` of the creating request.
    pub operation_key: Option<String>,
    /// SHA-256 of the creating request.
    pub request_hash: Option<[u8; 32]>,
    /// The voided case this one replaces.
    pub replaces: Option<CaseId>,
    /// Set when voided.
    pub void: Option<CaseVoid>,
    /// The importer's record of a v4 case.
    pub imported: Option<serde_json::Value>,
    /// When it was created.
    pub created_at: OffsetDateTime,
}

impl Case {
    /// Whether the case still counts (not voided).
    pub fn is_valid(&self) -> bool {
        self.void.is_none()
    }
}

/// How a case was voided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaseVoid {
    /// When.
    pub at: OffsetDateTime,
    /// Who; `None` = Quack or unknown.
    pub by: Option<AccountId>,
    /// The reason.
    pub reason: String,
    /// Why.
    pub cause: VoidCause,
}

/// A new case (§4.2 step 15).
#[derive(Clone, Debug, PartialEq)]
pub struct NewCase<'a> {
    /// A fresh `UUIDv7`.
    pub id: CaseId,
    /// The server.
    pub guild: GuildId,
    /// From [`crate::guilds::next_case_number`].
    pub number: u64,
    /// The member.
    pub member: MemberId,
    /// The member's `joined_at`, if present.
    pub member_joined_at: Option<OffsetDateTime>,
    /// `discord`, `dashboard`, or `honeypot`.
    pub source: CaseSource,
    /// The moderator (`None` for honeypot cases).
    pub moderator: Option<AccountId>,
    /// The decision.
    pub decision: &'a CaseDecision,
    /// Validated context values.
    pub context: &'a ContextValues,
    /// The scoped operation key.
    pub operation_key: Option<&'a str>,
    /// SHA-256 of the request.
    pub request_hash: Option<&'a [u8; 32]>,
    /// The voided case it replaces.
    pub replaces: Option<CaseId>,
    /// `now()`, captured once in Rust and reused for the action's timeout
    /// end and the case's jobs.
    pub created_at: OffsetDateTime,
}

/// Inserts a case.
pub async fn insert(conn: &mut PgConnection, case: &NewCase<'_>) -> Result<(), DbError> {
    let d = case.decision;
    let (kind, timeout, ban) = match d.action {
        None => (None, None, None),
        Some(LevelAction::Timeout { duration_seconds }) => (
            Some(ActionKind::Timeout),
            Some(int::<u32, i32>(duration_seconds)?),
            None,
        ),
        Some(LevelAction::Kick) => (Some(ActionKind::Kick), None, None),
        Some(LevelAction::Ban {
            delete_message_seconds,
        }) => (
            Some(ActionKind::Ban),
            None,
            Some(int::<u32, i32>(delete_message_seconds)?),
        ),
    };
    obs::run(
        Insert,
        "cases",
        sqlx::query!(
            "INSERT INTO cases (id, guild_id, number, member_id, member_joined_at, source, moderator_id, rule_id,
                                rule_version, case_count, level_index, level_from_case_count, action_kind,
                                timeout_seconds, ban_delete_message_seconds, sends_dm, context, operation_key,
                                request_hash, replaces_case_id, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $21)",
            case.id.get(),
            i64::from(case.guild),
            int::<u64, i64>(case.number)?,
            i64::from(case.member),
            case.member_joined_at,
            Pg(case.source) as _,
            case.moderator.map(i64::from),
            d.rule_id.get(),
            int::<u32, i32>(d.rule_version)?,
            int::<u32, i32>(d.case_count)?,
            int::<u16, i16>(d.level_index)?,
            int::<u32, i32>(d.level_from_case_count)?,
            kind.map(Pg) as _,
            timeout,
            ban,
            d.sends_dm,
            Json(case.context) as _,
            case.operation_key,
            case.request_hash.map(|h| &h[..]),
            case.replaces.map(CaseId::get),
            case.created_at,
        )
        .execute(conn),
    )
    .await?;
    Ok(())
}

/// Counted cases for escalation (§3.4): same server, member, and rule (any
/// version), valid, not imported, created at or after `since` when the
/// version has a counting window. Uses `cases_escalation`.
pub async fn count_counted(
    conn: &mut PgConnection,
    guild: GuildId,
    member: MemberId,
    rule: RuleId,
    since: Option<OffsetDateTime>,
) -> Result<u32, DbError> {
    let n = obs::run(
        Select,
        "cases",
        sqlx::query_scalar!(
            r#"SELECT count(*) AS "n!" FROM cases
               WHERE guild_id = $1 AND member_id = $2 AND rule_id = $3 AND voided_at IS NULL
                 AND source <> 'imported' AND created_at >= COALESCE($4, '-infinity'::timestamptz)"#,
            i64::from(guild),
            i64::from(member),
            rule.get(),
            since,
        )
        .fetch_one(conn),
    )
    .await?;
    count(n)
}

/// Voids a valid case (§4.7 step 2.3). Returns whether it was valid.
pub async fn void(
    conn: &mut PgConnection,
    guild: GuildId,
    case: CaseId,
    void_cause: VoidCause,
    by: Option<AccountId>,
    reason: &str,
) -> Result<bool, DbError> {
    let done = obs::run(
        Update,
        "cases",
        sqlx::query!(
            "UPDATE cases SET voided_at = now(), voided_by = $3, void_reason = $4, void_cause = $5, updated_at = now()
             WHERE guild_id = $1 AND id = $2 AND voided_at IS NULL",
            i64::from(guild),
            case.get(),
            by.map(i64::from),
            reason,
            Pg(void_cause) as _,
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Replaces the case context if it is still at `expected_revision` (§4.9).
/// Returns the new revision, or `None` when it changed.
pub async fn set_context(
    conn: &mut PgConnection,
    guild: GuildId,
    case: CaseId,
    expected_revision: u32,
    context: &ContextValues,
) -> Result<Option<u32>, DbError> {
    let revision = obs::run(
        Update,
        "cases",
        sqlx::query_scalar!(
            "UPDATE cases SET context = $4, context_revision = context_revision + 1, updated_at = now()
             WHERE guild_id = $1 AND id = $2 AND context_revision = $3
             RETURNING context_revision",
            i64::from(guild),
            case.get(),
            int::<u32, i32>(expected_revision)?,
            Json(context) as _,
        )
        .fetch_optional(conn),
    )
    .await?;
    revision.map(count).transpose()
}

/// A case named by a replacement link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaseRef {
    /// The case.
    pub id: CaseId,
    /// Its number.
    pub number: u64,
    /// Its member (member views show a link only when both match, §4.8).
    pub member: MemberId,
}

/// The case that replaces `case`, if any (`cases_replaces`).
pub async fn replaced_by(
    conn: &mut PgConnection,
    guild: GuildId,
    case: CaseId,
) -> Result<Option<CaseRef>, DbError> {
    case_ref(conn, guild, None, Some(case)).await
}

/// The case `case_id` names as its `replaces`, by ID.
pub async fn by_id_ref(
    conn: &mut PgConnection,
    guild: GuildId,
    case: CaseId,
) -> Result<Option<CaseRef>, DbError> {
    case_ref(conn, guild, Some(case), None).await
}

async fn case_ref(
    conn: &mut PgConnection,
    guild: GuildId,
    id_is: Option<CaseId>,
    replaces: Option<CaseId>,
) -> Result<Option<CaseRef>, DbError> {
    let row = obs::run(
        Select,
        "cases",
        sqlx::query!(
            "SELECT id, number, member_id FROM cases
             WHERE guild_id = $1 AND (id = $2 OR replaces_case_id = $3)",
            i64::from(guild),
            id_is.map(CaseId::get),
            replaces.map(CaseId::get),
        )
        .fetch_optional(conn),
    )
    .await?;
    row.map(|r| {
        Ok(CaseRef {
            id: CaseId::from_uuid(r.id),
            number: count(r.number)?,
            member: id(r.member_id)?,
        })
    })
    .transpose()
}

/// Honeypot cases in the server (the warning's count; `cases_honeypot`).
pub async fn count_honeypot(conn: &mut PgConnection, guild: GuildId) -> Result<u64, DbError> {
    let n = obs::run(
        Select,
        "cases",
        sqlx::query_scalar!(
            r#"SELECT count(*) AS "n!" FROM cases WHERE guild_id = $1 AND source = 'honeypot'"#,
            i64::from(guild)
        )
        .fetch_one(conn),
    )
    .await?;
    count(n)
}

mod read;

pub use read::{find_by_operation_key, get, get_by_number, get_for_member, lock};
