//! Action state changes, each a compare-and-set on the expected state, plus
//! the member lock and the undo guard (§5.3–§5.6).

use std::time::Duration;

use quack_core::action::{ActionKind, ActionSkipReason, ActionState, Failure};
use quack_core::ids::{AccountId, ActionId, CaseId, GuildId, MemberId};
use sqlx::PgConnection;

use super::secs;
use crate::DbError;
use crate::obs::{self, Select, Update};
use crate::pg::{Pg, count, int};

/// Takes the member's transaction-scoped advisory lock (§5.4 step 1.1).
/// Every action start for one (server, member) takes it first, so their
/// starts are serialized; it is released when the transaction ends.
pub async fn lock_member(
    conn: &mut PgConnection,
    guild: GuildId,
    member: MemberId,
) -> Result<(), DbError> {
    obs::run(
        Select,
        "pg_advisory_xact_lock",
        sqlx::query!(
            "SELECT pg_advisory_xact_lock(hashtextextended('member:' || $1::bigint || ':' || $2::bigint, 0))",
            i64::from(guild),
            i64::from(member),
        )
        .execute(conn),
    )
    .await?;
    Ok(())
}

/// Whether another action of the member is `running` (§5.4 step 1.6).
pub async fn member_running(
    conn: &mut PgConnection,
    guild: GuildId,
    member: MemberId,
    except: ActionId,
) -> Result<bool, DbError> {
    obs::run(
        Select,
        "actions",
        sqlx::query_scalar!(
            r#"SELECT EXISTS (SELECT 1 FROM actions
                              WHERE guild_id = $1 AND member_id = $2 AND state = 'running' AND id <> $3) AS "busy!""#,
            i64::from(guild),
            i64::from(member),
            except.get(),
        )
        .fetch_one(conn),
    )
    .await
}

/// The undo guard (§5.6): the newest other valid case of the member whose
/// `original` action (timeout or ban) is `done` and, for a timeout, still in
/// force. Returns its ID and number.
pub async fn guard(
    conn: &mut PgConnection,
    guild: GuildId,
    member: MemberId,
    original: ActionKind,
    own_case: CaseId,
) -> Result<Option<(CaseId, u64)>, DbError> {
    let row = obs::run(
        Select,
        "actions",
        sqlx::query!(
            "SELECT c.id, c.number
             FROM actions a JOIN cases c ON c.guild_id = a.guild_id AND c.id = a.case_id
             WHERE a.guild_id = $1 AND a.member_id = $2 AND a.kind = $3 AND a.case_id <> $4
               AND c.voided_at IS NULL AND a.state = 'done'
               AND (a.kind <> 'timeout' OR a.timeout_ends_at > now())
             ORDER BY c.number DESC LIMIT 1",
            i64::from(guild),
            i64::from(member),
            Pg(original) as _,
            own_case.get(),
        )
        .fetch_optional(conn),
    )
    .await?;
    row.map(|r| Ok((CaseId::from_uuid(r.id), count(r.number)?)))
        .transpose()
}

/// Whether case `number` of this server is a voided case of `member` with
/// a ban action, so a ban whose marker names it is Quack's to remove (§5.6).
pub async fn voided_ban_case(
    conn: &mut PgConnection,
    guild: GuildId,
    member: MemberId,
    number: u64,
) -> Result<bool, DbError> {
    obs::run(
        Select,
        "cases",
        sqlx::query_scalar!(
            r#"SELECT EXISTS (SELECT 1 FROM cases c JOIN actions a ON a.case_id = c.id AND a.kind = 'ban'
                              WHERE c.guild_id = $1 AND c.number = $2 AND c.member_id = $3
                                AND c.voided_at IS NOT NULL) AS "found!""#,
            i64::from(guild),
            int::<u64, i64>(number)?,
            i64::from(member),
        )
        .fetch_one(conn),
    )
    .await
}

/// `running` → `done` (`no_change` when the desired state already held).
pub async fn mark_done(
    conn: &mut PgConnection,
    guild: GuildId,
    action: ActionId,
    no_change: bool,
) -> Result<bool, DbError> {
    let done = obs::run(
        Update,
        "actions",
        sqlx::query!(
            "UPDATE actions SET state = 'done', done_at = now(), no_change = $3, updated_at = now()
             WHERE guild_id = $1 AND id = $2 AND state = 'running'",
            i64::from(guild),
            action.get(),
            no_change,
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// `from` (`pending` or `running`) → `failed` with the failure's details.
pub async fn mark_failed(
    conn: &mut PgConnection,
    guild: GuildId,
    action: ActionId,
    from: ActionState,
    failure: &Failure,
) -> Result<bool, DbError> {
    let done = obs::run(
        Update,
        "actions",
        sqlx::query!(
            "UPDATE actions SET state = 'failed', failure_reason = $4, failure_permission = $5,
                                failure_http_status = $6, failure_discord_code = $7, next_attempt_at = NULL,
                                updated_at = now()
             WHERE guild_id = $1 AND id = $2 AND state = $3",
            i64::from(guild),
            action.get(),
            Pg(from) as _,
            Pg(failure.reason) as _,
            failure.permission.map(Pg) as _,
            failure.http_status.map(int::<u16, i16>).transpose()?,
            failure.discord_code.map(int::<u32, i32>).transpose()?,
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// `from` (`pending` or `running`) → `cancelled` (originals only).
pub async fn mark_cancelled(
    conn: &mut PgConnection,
    guild: GuildId,
    action: ActionId,
    from: ActionState,
) -> Result<bool, DbError> {
    let done = obs::run(
        Update,
        "actions",
        sqlx::query!(
            "UPDATE actions SET state = 'cancelled', next_attempt_at = NULL, updated_at = now()
             WHERE guild_id = $1 AND id = $2 AND state = $3",
            i64::from(guild),
            action.get(),
            Pg(from) as _,
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// `from` (`pending` or `running`) → `skipped` (undos only), with the case
/// that still applies for `other_case_applies`.
pub async fn mark_skipped(
    conn: &mut PgConnection,
    guild: GuildId,
    action: ActionId,
    from: ActionState,
    reason: ActionSkipReason,
    because: Option<CaseId>,
) -> Result<bool, DbError> {
    let done = obs::run(
        Update,
        "actions",
        sqlx::query!(
            "UPDATE actions SET state = 'skipped', skip_reason = $4, skipped_because_case_id = $5,
                                next_attempt_at = NULL, updated_at = now()
             WHERE guild_id = $1 AND id = $2 AND state = $3",
            i64::from(guild),
            action.get(),
            Pg(from) as _,
            Pg(reason) as _,
            because.map(CaseId::get),
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// `running` → `pending` with the next attempt due after `delay` (§5.7).
pub async fn back_to_pending(
    conn: &mut PgConnection,
    guild: GuildId,
    action: ActionId,
    delay: Duration,
) -> Result<bool, DbError> {
    let done = obs::run(
        Update,
        "actions",
        sqlx::query!(
            "UPDATE actions SET state = 'pending', next_attempt_at = now() + make_interval(secs => $3),
                                updated_at = now()
             WHERE guild_id = $1 AND id = $2 AND state = 'running'",
            i64::from(guild),
            action.get(),
            secs(delay),
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Staff retry (§5.8): `failed`, not dismissed → `pending`, a new round
/// (`attempts = 0`), failure details cleared.
pub async fn retry(
    conn: &mut PgConnection,
    guild: GuildId,
    action: ActionId,
    by: AccountId,
) -> Result<bool, DbError> {
    let done = obs::run(
        Update,
        "actions",
        sqlx::query!(
            "UPDATE actions SET state = 'pending', attempts = 0, next_attempt_at = NULL, failure_reason = NULL,
                                failure_permission = NULL, failure_http_status = NULL, failure_discord_code = NULL,
                                requested_by = $3, updated_at = now()
             WHERE guild_id = $1 AND id = $2 AND state = 'failed' AND dismissed_at IS NULL",
            i64::from(guild),
            action.get(),
            i64::from(by),
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Staff dismiss: a `failed`, undismissed action leaves the failed list.
pub async fn dismiss(
    conn: &mut PgConnection,
    guild: GuildId,
    action: ActionId,
    by: AccountId,
) -> Result<bool, DbError> {
    let done = obs::run(
        Update,
        "actions",
        sqlx::query!(
            "UPDATE actions SET dismissed_at = now(), dismissed_by = $3, updated_at = now()
             WHERE guild_id = $1 AND id = $2 AND state = 'failed' AND dismissed_at IS NULL",
            i64::from(guild),
            action.get(),
            i64::from(by),
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Void (§4.7 step 2.4): a `pending` original of the case → `cancelled`.
/// Returns the cancelled action, if there was one.
pub async fn cancel_pending_original(
    conn: &mut PgConnection,
    guild: GuildId,
    case: CaseId,
) -> Result<Option<ActionId>, DbError> {
    let id = obs::run(
        Update,
        "actions",
        sqlx::query_scalar!(
            "UPDATE actions SET state = 'cancelled', next_attempt_at = NULL, updated_at = now()
             WHERE guild_id = $1 AND case_id = $2 AND undo_of_action_id IS NULL AND state = 'pending'
             RETURNING id",
            i64::from(guild),
            case.get(),
        )
        .fetch_optional(conn),
    )
    .await?;
    Ok(id.map(ActionId::from_uuid))
}
