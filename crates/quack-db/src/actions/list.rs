//! Action lists: the failed-actions page and counts for gauges.

use quack_core::action::{ActionKind, ActionState};
use quack_core::ids::{GuildId, MemberId};
use sqlx::PgConnection;

use super::{Action, ActionRow};
use crate::DbError;
use crate::obs::{self, Select};
use crate::pg::{Page, Pg, TimeCursor, count, cursor_parts, fetch_limit};

/// One row of the actions page (route 28).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionListItem {
    /// The action.
    pub action: Action,
    /// Its case's number.
    pub case_number: u64,
    /// Its case's member.
    pub member: MemberId,
    /// The rule's name in the case's version.
    pub rule_name: Option<String>,
}

/// Actions in `state`, newest first after `cursor`. For `failed`,
/// `dismissed` picks undismissed (`actions_review`) or dismissed ones.
pub async fn list(
    conn: &mut PgConnection,
    guild: GuildId,
    state: ActionState,
    dismissed: bool,
    cursor: Option<TimeCursor>,
    page: u32,
) -> Result<Page<ActionListItem>, DbError> {
    let (after_time, after_id) = cursor_parts(cursor);
    let rows = obs::run(
        Select,
        "actions",
        sqlx::query!(
            r#"SELECT a.id, a.guild_id, a.case_id, a.member_id, a.kind, a.undo_of_action_id, a.state,
                      a.timeout_ends_at, a.delete_message_seconds, a.requested_by, a.attempts, a.next_attempt_at,
                      a.last_attempt_at, a.failure_reason, a.failure_permission, a.failure_http_status,
                      a.failure_discord_code, a.skip_reason, a.skipped_because_case_id, a.no_change, a.done_at,
                      a.dismissed_at, a.dismissed_by, a.created_at,
                      c.number AS case_number, v.definition->>'name' AS rule_name
               FROM actions a
               JOIN cases c ON c.guild_id = a.guild_id AND c.id = a.case_id
               LEFT JOIN rule_versions v ON v.rule_id = c.rule_id AND v.version = c.rule_version
               WHERE a.guild_id = $1 AND a.state = $2
                 AND ($2 <> 'failed' OR (a.dismissed_at IS NOT NULL) = $3)
                 AND ($4::timestamptz IS NULL OR (a.created_at, a.id) < ($4, $5))
               ORDER BY a.created_at DESC, a.id DESC
               LIMIT $6"#,
            i64::from(guild),
            Pg(state) as _,
            dismissed,
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
            let action = Action::try_from(ActionRow {
                id: r.id,
                guild_id: r.guild_id,
                case_id: r.case_id,
                member_id: r.member_id,
                kind: r.kind,
                undo_of_action_id: r.undo_of_action_id,
                state: r.state,
                timeout_ends_at: r.timeout_ends_at,
                delete_message_seconds: r.delete_message_seconds,
                requested_by: r.requested_by,
                attempts: r.attempts,
                next_attempt_at: r.next_attempt_at,
                last_attempt_at: r.last_attempt_at,
                failure_reason: r.failure_reason,
                failure_permission: r.failure_permission,
                failure_http_status: r.failure_http_status,
                failure_discord_code: r.failure_discord_code,
                skip_reason: r.skip_reason,
                skipped_because_case_id: r.skipped_because_case_id,
                no_change: r.no_change,
                done_at: r.done_at,
                dismissed_at: r.dismissed_at,
                dismissed_by: r.dismissed_by,
                created_at: r.created_at,
            })?;
            Ok(ActionListItem {
                member: action.member,
                action,
                case_number: count(r.case_number)?,
                rule_name: r.rule_name,
            })
        })
        .collect();
    Ok(Page::of(rows?, page, 100))
}

/// Failed, undismissed actions in a server (`GuildMe` counts).
pub async fn count_failed(conn: &mut PgConnection, guild: GuildId) -> Result<u64, DbError> {
    let n = obs::run(
        Select,
        "actions",
        sqlx::query_scalar!(
            r#"SELECT count(*) AS "n!" FROM actions WHERE guild_id = $1 AND state = 'failed' AND dismissed_at IS NULL"#,
            i64::from(guild)
        )
        .fetch_one(conn),
    )
    .await?;
    count(n)
}

/// Failed, undismissed actions across servers per kind, for the
/// `quack.actions.failed` gauge: every kind, zero when it has no rows.
pub async fn failed_by_kind(conn: &mut PgConnection) -> Result<Vec<(ActionKind, u64)>, DbError> {
    let rows = obs::run(
        Select,
        "actions",
        sqlx::query!(
            r#"SELECT kind, count(*) AS "n!" FROM actions WHERE state = 'failed' AND dismissed_at IS NULL GROUP BY kind"#
        )
        .fetch_all(conn),
    )
    .await?;
    ActionKind::ALL
        .iter()
        .map(|kind| {
            let n = rows.iter().find(|r| r.kind.0 == *kind).map_or(0, |r| r.n);
            Ok((*kind, count(n)?))
        })
        .collect()
}
