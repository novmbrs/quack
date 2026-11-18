//! Appeal timelines and appeal lists.

use quack_core::appeal::{AppealEventKind, AppealState};
use quack_core::ids::{AppealEventId, AppealId, GuildId};
use sqlx::PgConnection;
use time::OffsetDateTime;

use super::{Appeal, AppealEvent, AppealRow};
use crate::DbError;
use crate::obs::{self, Select};
use crate::pg::{Pg, TimeCursor, count, cursor_parts, limit, opt_id};

struct EventRow {
    id: uuid::Uuid,
    appeal_id: uuid::Uuid,
    kind: Pg<AppealEventKind>,
    actor_id: Option<i64>,
    text: String,
    closed_by_void: bool,
    created_at: OffsetDateTime,
}

impl TryFrom<EventRow> for AppealEvent {
    type Error = DbError;

    fn try_from(r: EventRow) -> Result<Self, DbError> {
        Ok(Self {
            id: AppealEventId::from_uuid(r.id),
            appeal: AppealId::from_uuid(r.appeal_id),
            kind: r.kind.0,
            actor: opt_id(r.actor_id)?,
            text: r.text,
            closed_by_void: r.closed_by_void,
            created_at: r.created_at,
        })
    }
}

/// An appeal's timeline, oldest first.
pub async fn events(
    conn: &mut PgConnection,
    guild: GuildId,
    appeal: AppealId,
) -> Result<Vec<AppealEvent>, DbError> {
    let rows = obs::run(
        Select,
        "appeal_events",
        sqlx::query_as!(
            EventRow,
            "SELECT id, appeal_id, kind, actor_id, text, closed_by_void, created_at FROM appeal_events
             WHERE guild_id = $1 AND appeal_id = $2 ORDER BY created_at, id",
            i64::from(guild),
            appeal.get()
        )
        .fetch_all(conn),
    )
    .await?;
    rows.into_iter().map(AppealEvent::try_from).collect()
}

/// One appeal event (an appeal-update DM renders from it).
pub async fn event(
    conn: &mut PgConnection,
    guild: GuildId,
    event: AppealEventId,
) -> Result<Option<AppealEvent>, DbError> {
    let row = obs::run(
        Select,
        "appeal_events",
        sqlx::query_as!(
            EventRow,
            "SELECT id, appeal_id, kind, actor_id, text, closed_by_void, created_at FROM appeal_events
             WHERE guild_id = $1 AND id = $2",
            i64::from(guild),
            event.get()
        )
        .fetch_optional(conn),
    )
    .await?;
    row.map(AppealEvent::try_from).transpose()
}

/// One row of an appeals list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppealSummary {
    /// The appeal.
    pub appeal: Appeal,
    /// Its case's number.
    pub case_number: u64,
    /// The rule's name in the case's version.
    pub rule_name: Option<String>,
}

/// Appeals (optionally in one state), newest first after `cursor`.
pub async fn list(
    conn: &mut PgConnection,
    guild: GuildId,
    state: Option<AppealState>,
    cursor: Option<TimeCursor>,
    page: u32,
) -> Result<Vec<AppealSummary>, DbError> {
    let (after_time, after_id) = cursor_parts(cursor);
    let rows = obs::run(
        Select,
        "appeals",
        sqlx::query!(
            r#"SELECT p.id, p.guild_id, p.case_id, p.member_id, p.state, p.version, p.statement, p.decided_by,
                      p.decided_at, p.decision_reason, p.first_decided_at, p.created_at, p.updated_at,
                      c.number AS case_number, v.definition->>'name' AS rule_name
               FROM appeals p
               JOIN cases c ON c.guild_id = p.guild_id AND c.id = p.case_id
               LEFT JOIN rule_versions v ON v.rule_id = c.rule_id AND v.version = c.rule_version
               WHERE p.guild_id = $1 AND ($2::appeal_state IS NULL OR p.state = $2)
                 AND ($3::timestamptz IS NULL OR (p.created_at, p.id) < ($3, $4))
               ORDER BY p.created_at DESC, p.id DESC
               LIMIT $5"#,
            i64::from(guild),
            state.map(Pg),
            after_time,
            after_id,
            limit(page, 100),
        )
        .fetch_all(conn),
    )
    .await?;
    rows.into_iter()
        .map(|r| {
            Ok(AppealSummary {
                case_number: count(r.case_number)?,
                rule_name: r.rule_name,
                appeal: Appeal::try_from(AppealRow {
                    id: r.id,
                    guild_id: r.guild_id,
                    case_id: r.case_id,
                    member_id: r.member_id,
                    state: r.state,
                    version: r.version,
                    statement: r.statement,
                    decided_by: r.decided_by,
                    decided_at: r.decided_at,
                    decision_reason: r.decision_reason,
                    first_decided_at: r.first_decided_at,
                    created_at: r.created_at,
                    updated_at: r.updated_at,
                })?,
            })
        })
        .collect()
}

/// Pending appeals, oldest first (`/appeals`; `appeals_guild_state`).
pub async fn pending(
    conn: &mut PgConnection,
    guild: GuildId,
    page: u32,
) -> Result<Vec<Appeal>, DbError> {
    let rows = obs::run(
        Select,
        "appeals",
        query_appeal!(
            "WHERE guild_id = $1 AND state = 'pending' ORDER BY created_at, id LIMIT $2",
            i64::from(guild),
            limit(page, 100)
        )
        .fetch_all(conn),
    )
    .await?;
    rows.into_iter().map(Appeal::try_from).collect()
}

/// Pending appeals in a server (`GuildMe` counts).
pub async fn count_pending(conn: &mut PgConnection, guild: GuildId) -> Result<u64, DbError> {
    let n = obs::run(
        Select,
        "appeals",
        sqlx::query_scalar!(
            r#"SELECT count(*) AS "n!" FROM appeals WHERE guild_id = $1 AND state = 'pending'"#,
            i64::from(guild)
        )
        .fetch_one(conn),
    )
    .await?;
    count(n)
}

/// Open appeals (`pending` or `needs_information`) across servers, for the
/// `quack.appeals.open` gauge.
pub async fn count_open(conn: &mut PgConnection) -> Result<u64, DbError> {
    let n = obs::run(
        Select,
        "appeals",
        sqlx::query_scalar!(
            r#"SELECT count(*) AS "n!" FROM appeals WHERE state IN ('pending', 'needs_information')"#
        )
        .fetch_one(conn),
    )
    .await?;
    count(n)
}
