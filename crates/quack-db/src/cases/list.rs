//! Case lists: staff lists with filters, a member's own cases, and member
//! summaries.

use quack_core::action::{ActionKind, ActionState};
use quack_core::appeal::AppealState;
use quack_core::case::CaseSource;
use quack_core::dm::DmState;
use quack_core::ids::{AccountId, CaseId, GuildId, MemberId, RuleId};
use sqlx::PgConnection;
use time::OffsetDateTime;

use crate::DbError;
use crate::obs::{self, Select};
use crate::pg::{Page, Pg, TimeCursor, count, cursor_parts, fetch_limit, id, int, opt_id};

/// Filters of the staff case list (route 20), combined with AND.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CaseFilter {
    /// Cases about this member.
    pub member: Option<MemberId>,
    /// Cases by this moderator.
    pub moderator: Option<AccountId>,
    /// Cases under this rule.
    pub rule: Option<RuleId>,
    /// `Some(true)` voided only, `Some(false)` valid only.
    pub voided: Option<bool>,
    /// Cases whose original action is in this state.
    pub action_state: Option<ActionState>,
    /// Cases whose appeal is in this state.
    pub appeal_state: Option<AppealState>,
    /// The case with this number.
    pub number: Option<u64>,
    /// Created at or after.
    pub created_after: Option<OffsetDateTime>,
    /// Created before.
    pub created_before: Option<OffsetDateTime>,
}

/// One row of the staff case list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaseSummary {
    /// The case.
    pub id: CaseId,
    /// Its number.
    pub number: u64,
    /// Its member.
    pub member: MemberId,
    /// Its rule and version, when any.
    pub rule: Option<(RuleId, u32)>,
    /// The name in that version; `None` for imported cases without a rule.
    pub rule_name: Option<String>,
    /// The chosen level.
    pub level_index: Option<u16>,
    /// Where it came from.
    pub source: CaseSource,
    /// The moderator.
    pub moderator: Option<AccountId>,
    /// When it was created.
    pub created_at: OffsetDateTime,
    /// When it was voided.
    pub voided_at: Option<OffsetDateTime>,
    /// The original action's kind and state.
    pub action: Option<(ActionKind, ActionState)>,
    /// The case DM's state; `None` when the level sends no DM.
    pub dm_state: Option<DmState>,
    /// The appeal's state, when appealed.
    pub appeal_state: Option<AppealState>,
}

/// The staff case list, newest first, after `before_number` (the cursor).
pub async fn list(
    conn: &mut PgConnection,
    guild: GuildId,
    filter: &CaseFilter,
    before_number: Option<u64>,
    page: u32,
) -> Result<Page<CaseSummary>, DbError> {
    let rows = obs::run(
        Select,
        "cases",
        sqlx::query!(
            r#"SELECT c.id, c.number, c.member_id, c.rule_id, c.rule_version, v.definition->>'name' AS rule_name,
                      c.level_index, c.source, c.moderator_id, c.created_at, c.voided_at,
                      a.kind AS "action_kind?", a.state AS "action_state?", d.state AS "dm_state?",
                      ap.state AS "appeal_state?"
               FROM cases c
               LEFT JOIN rule_versions v ON v.rule_id = c.rule_id AND v.version = c.rule_version
               LEFT JOIN actions a ON a.case_id = c.id AND a.undo_of_action_id IS NULL
               LEFT JOIN dms d ON d.case_id = c.id AND d.kind = 'case'
               LEFT JOIN appeals ap ON ap.case_id = c.id
               WHERE c.guild_id = $1
                 AND ($2::bigint IS NULL OR c.number < $2)
                 AND ($3::bigint IS NULL OR c.member_id = $3)
                 AND ($4::bigint IS NULL OR c.moderator_id = $4)
                 AND ($5::uuid IS NULL OR c.rule_id = $5)
                 AND ($6::boolean IS NULL OR (c.voided_at IS NOT NULL) = $6)
                 AND ($7::action_state IS NULL OR a.state = $7)
                 AND ($8::appeal_state IS NULL OR ap.state = $8)
                 AND ($9::bigint IS NULL OR c.number = $9)
                 AND ($10::timestamptz IS NULL OR c.created_at >= $10)
                 AND ($11::timestamptz IS NULL OR c.created_at < $11)
               ORDER BY c.number DESC
               LIMIT $12"#,
            i64::from(guild),
            before_number.map(int::<u64, i64>).transpose()?,
            filter.member.map(i64::from),
            filter.moderator.map(i64::from),
            filter.rule.map(RuleId::get),
            filter.voided,
            filter.action_state.map(Pg),
            filter.appeal_state.map(Pg),
            filter.number.map(int::<u64, i64>).transpose()?,
            filter.created_after,
            filter.created_before,
            fetch_limit(page, 100),
        )
        .fetch_all(conn),
    )
    .await?;
    let rows: Result<Vec<_>, DbError> = rows
        .into_iter()
        .map(|r| {
            Ok(CaseSummary {
                id: CaseId::from_uuid(r.id),
                number: count(r.number)?,
                member: id(r.member_id)?,
                rule: match (r.rule_id, r.rule_version) {
                    (Some(rule), Some(version)) => Some((RuleId::from_uuid(rule), count(version)?)),
                    _ => None,
                },
                rule_name: r.rule_name,
                level_index: r.level_index.map(count).transpose()?,
                source: r.source.0,
                moderator: opt_id(r.moderator_id)?,
                created_at: r.created_at,
                voided_at: r.voided_at,
                action: r.action_kind.zip(r.action_state).map(|(k, s)| (k.0, s.0)),
                dm_state: r.dm_state.map(|s| s.0),
                appeal_state: r.appeal_state.map(|s| s.0),
            })
        })
        .collect();
    Ok(Page::of(rows?, page, 100))
}

/// One case the "Add as evidence" picker offers (§11.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecentCase {
    /// The case.
    pub id: CaseId,
    /// Its number.
    pub number: u64,
    /// Its member.
    pub member: MemberId,
    /// The rule's name in the case's version; `None` for imported cases.
    pub rule_name: Option<String>,
    /// When it was created.
    pub created_at: OffsetDateTime,
    /// Whether it is voided (still offered, and marked).
    pub voided: bool,
}

/// Cases about `member` and cases `moderator` added, newest first, each
/// side at most `limit` before merging, without repeats, at most `limit`
/// in all (indexes `cases_member`, `cases_moderator`).
pub async fn recent(
    conn: &mut PgConnection,
    guild: GuildId,
    member: Option<MemberId>,
    moderator: AccountId,
    limit: u32,
) -> Result<Vec<RecentCase>, DbError> {
    let rows = obs::run(
        Select,
        "cases",
        sqlx::query!(
            r#"SELECT c.id AS "id!", c.number AS "number!", c.member_id AS "member_id!",
                      c.created_at AS "created_at!", c.voided_at, v.definition->>'name' AS rule_name
               FROM ((SELECT id, number, member_id, rule_id, rule_version, created_at, voided_at FROM cases
                      WHERE guild_id = $1 AND member_id = $2 ORDER BY number DESC LIMIT $4)
                     UNION
                     (SELECT id, number, member_id, rule_id, rule_version, created_at, voided_at FROM cases
                      WHERE guild_id = $1 AND moderator_id = $3 ORDER BY number DESC LIMIT $4)) c
               LEFT JOIN rule_versions v ON v.rule_id = c.rule_id AND v.version = c.rule_version
               ORDER BY c.number DESC
               LIMIT $4"#,
            i64::from(guild),
            member.map(i64::from),
            i64::from(moderator),
            i64::from(limit),
        )
        .fetch_all(conn),
    )
    .await?;
    rows.into_iter()
        .map(|r| {
            Ok(RecentCase {
                id: CaseId::from_uuid(r.id),
                number: count(r.number)?,
                member: id(r.member_id)?,
                rule_name: r.rule_name,
                created_at: r.created_at,
                voided: r.voided_at.is_some(),
            })
        })
        .collect()
}

/// One of a member's own cases (route 51).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberCaseSummary {
    /// The case.
    pub id: CaseId,
    /// Its server.
    pub guild: GuildId,
    /// The server's name.
    pub guild_name: String,
    /// The server's icon hash.
    pub guild_icon_hash: Option<String>,
    /// Its number.
    pub number: u64,
    /// The rule's name in the case's version.
    pub rule_name: Option<String>,
    /// The official reason in that version.
    pub reason: Option<String>,
    /// Whether that version is appealable.
    pub appealable: bool,
    /// Where it came from (imported cases can't be appealed).
    pub source: CaseSource,
    /// When it was created.
    pub created_at: OffsetDateTime,
    /// When it was voided.
    pub voided_at: Option<OffsetDateTime>,
    /// The original action's kind and state.
    pub action: Option<(ActionKind, ActionState)>,
    /// The appeal's state.
    pub appeal_state: Option<AppealState>,
}

/// A member's own cases across servers (or in one), newest first, after
/// `cursor`. Uses `cases_member_all`.
pub async fn member_cases(
    conn: &mut PgConnection,
    member: MemberId,
    guild: Option<GuildId>,
    cursor: Option<TimeCursor>,
    page: u32,
) -> Result<Page<MemberCaseSummary>, DbError> {
    let (after_time, after_id) = cursor_parts(cursor);
    let rows = obs::run(
        Select,
        "cases",
        sqlx::query!(
            r#"SELECT c.id, c.guild_id, g.name AS guild_name, g.icon_hash, c.number,
                      v.definition->>'name' AS rule_name, v.definition->>'reason' AS reason,
                      COALESCE((v.definition->>'appealable')::boolean, false) AS "appealable!",
                      c.source, c.created_at, c.voided_at,
                      a.kind AS "action_kind?", a.state AS "action_state?", ap.state AS "appeal_state?"
               FROM cases c
               JOIN guilds g ON g.guild_id = c.guild_id
               LEFT JOIN rule_versions v ON v.rule_id = c.rule_id AND v.version = c.rule_version
               LEFT JOIN actions a ON a.case_id = c.id AND a.undo_of_action_id IS NULL
               LEFT JOIN appeals ap ON ap.case_id = c.id
               WHERE c.member_id = $1
                 AND ($2::bigint IS NULL OR c.guild_id = $2)
                 AND ($3::timestamptz IS NULL OR (c.created_at, c.id) < ($3, $4))
               ORDER BY c.created_at DESC, c.id DESC
               LIMIT $5"#,
            i64::from(member),
            guild.map(i64::from),
            after_time,
            after_id,
            fetch_limit(page, 100),
        )
        .fetch_all(conn),
    )
    .await?;
    let rows: Result<Vec<_>, DbError> = rows
        .into_iter()
        .map(|r| {
            Ok(MemberCaseSummary {
                id: CaseId::from_uuid(r.id),
                guild: id(r.guild_id)?,
                guild_name: r.guild_name,
                guild_icon_hash: r.icon_hash,
                number: count(r.number)?,
                rule_name: r.rule_name,
                reason: r.reason,
                appealable: r.appealable,
                source: r.source.0,
                created_at: r.created_at,
                voided_at: r.voided_at,
                action: r.action_kind.zip(r.action_state).map(|(k, s)| (k.0, s.0)),
                appeal_state: r.appeal_state.map(|s| s.0),
            })
        })
        .collect();
    Ok(Page::of(rows?, page, 100))
}

/// A member's case counts under one rule (route 27).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleTally {
    /// The rule; `None` for imported cases without one.
    pub rule: Option<RuleId>,
    /// The rule's current name.
    pub rule_name: Option<String>,
    /// Valid cases.
    pub valid: u64,
    /// Voided cases.
    pub voided: u64,
}

/// A member's cases in a server, counted per rule.
pub async fn member_summary(
    conn: &mut PgConnection,
    guild: GuildId,
    member: MemberId,
) -> Result<Vec<RuleTally>, DbError> {
    let rows = obs::run(
        Select,
        "cases",
        sqlx::query!(
            r#"SELECT c.rule_id, max(r.name) AS rule_name,
                      count(*) FILTER (WHERE c.voided_at IS NULL) AS "valid!",
                      count(*) FILTER (WHERE c.voided_at IS NOT NULL) AS "voided!"
               FROM cases c LEFT JOIN rules r ON r.id = c.rule_id
               WHERE c.guild_id = $1 AND c.member_id = $2
               GROUP BY c.rule_id
               ORDER BY max(r.name) NULLS LAST"#,
            i64::from(guild),
            i64::from(member),
        )
        .fetch_all(conn),
    )
    .await?;
    rows.into_iter()
        .map(|r| {
            Ok(RuleTally {
                rule: r.rule_id.map(RuleId::from_uuid),
                rule_name: r.rule_name,
                valid: count(r.valid)?,
                voided: count(r.voided)?,
            })
        })
        .collect()
}
