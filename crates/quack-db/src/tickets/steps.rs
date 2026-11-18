//! Ticket state changes and the open and close jobs' progress columns
//! (§13.2.2–§13.2.4). Each is a compare-and-set on the expected state.

use quack_core::ids::{AccountId, ChannelId, GuildId, TicketId};
use quack_core::modules::TicketState;
use sqlx::PgConnection;
use time::OffsetDateTime;

use crate::DbError;
use crate::obs::{self, Update};

/// Saves the thread of an `opening` ticket that has none. Returns whether
/// it saved; another ticket's thread is
/// [`Conflict::ThreadTaken`](crate::Conflict::ThreadTaken).
pub async fn set_thread(
    conn: &mut PgConnection,
    guild: GuildId,
    ticket: TicketId,
    thread: ChannelId,
) -> Result<bool, DbError> {
    let done = obs::run(
        Update,
        "tickets",
        sqlx::query!(
            "UPDATE tickets SET thread_id = $3, updated_at = now()
             WHERE guild_id = $1 AND id = $2 AND state = 'opening' AND thread_id IS NULL",
            i64::from(guild),
            ticket.get(),
            i64::from(thread),
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Starts (or restarts, after the window) the welcome's create window.
pub async fn begin_welcome(
    conn: &mut PgConnection,
    guild: GuildId,
    ticket: TicketId,
) -> Result<Option<OffsetDateTime>, DbError> {
    obs::run(
        Update,
        "tickets",
        sqlx::query_scalar!(
            r#"UPDATE tickets SET welcome_started_at = now(), updated_at = now()
               WHERE guild_id = $1 AND id = $2 AND state = 'opening' AND welcome_posted_at IS NULL
               RETURNING welcome_started_at AS "started!""#,
            i64::from(guild),
            ticket.get(),
        )
        .fetch_optional(conn),
    )
    .await
}

/// Records the posted welcome.
pub async fn mark_welcome_posted(
    conn: &mut PgConnection,
    guild: GuildId,
    ticket: TicketId,
) -> Result<bool, DbError> {
    let done = obs::run(
        Update,
        "tickets",
        sqlx::query!(
            "UPDATE tickets SET welcome_posted_at = now(), updated_at = now()
             WHERE guild_id = $1 AND id = $2 AND state = 'opening' AND welcome_posted_at IS NULL",
            i64::from(guild),
            ticket.get(),
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// `opening` → `open` (thread ready, member added, welcome posted).
pub async fn mark_open(
    conn: &mut PgConnection,
    guild: GuildId,
    ticket: TicketId,
) -> Result<bool, DbError> {
    let done = obs::run(
        Update,
        "tickets",
        sqlx::query!(
            "UPDATE tickets SET state = 'open', opened_at = now(), updated_at = now()
             WHERE guild_id = $1 AND id = $2 AND state = 'opening' AND thread_id IS NOT NULL",
            i64::from(guild),
            ticket.get(),
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// The open failed (§13.2.2): `opening` → `closed` without a thread, or
/// `closing` (so the close job deletes it) with one. Returns the new state.
pub async fn fail_open(
    conn: &mut PgConnection,
    guild: GuildId,
    ticket: TicketId,
) -> Result<Option<TicketState>, DbError> {
    let state = obs::run(
        Update,
        "tickets",
        sqlx::query_scalar!(
            "UPDATE tickets SET close_reason = 'open_failed', updated_at = now(),
                                state = CASE WHEN thread_id IS NULL THEN 'closed'::ticket_state ELSE 'closing' END,
                                closed_at = CASE WHEN thread_id IS NULL THEN now() END,
                                close_requested_at = now()
             WHERE guild_id = $1 AND id = $2 AND state = 'opening'
             RETURNING state",
            i64::from(guild),
            ticket.get(),
        )
        .fetch_optional(conn),
    )
    .await?;
    Ok(state.map(|s| s.0))
}

/// `open` → `closing` (the member, staff, or Quack when the thread was
/// deleted). Returns whether it was open.
pub async fn request_close(
    conn: &mut PgConnection,
    guild: GuildId,
    ticket: TicketId,
    by: Option<AccountId>,
) -> Result<bool, DbError> {
    let done = obs::run(
        Update,
        "tickets",
        sqlx::query!(
            "UPDATE tickets SET state = 'closing', closed_by = $3, close_requested_at = now(), updated_at = now()
             WHERE guild_id = $1 AND id = $2 AND state = 'open'",
            i64::from(guild),
            ticket.get(),
            by.map(i64::from),
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Close step 1 done: the thread is frozen; the journal admits nothing more.
pub async fn mark_frozen(
    conn: &mut PgConnection,
    guild: GuildId,
    ticket: TicketId,
) -> Result<bool, DbError> {
    let done = obs::run(
        Update,
        "tickets",
        sqlx::query!(
            "UPDATE tickets SET frozen_at = now(), updated_at = now()
             WHERE guild_id = $1 AND id = $2 AND state = 'closing' AND frozen_at IS NULL",
            i64::from(guild),
            ticket.get(),
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Close step 3: `closing` → `closed`; the journal rows expire with the
/// transcript. Returns whether it closed.
pub async fn mark_closed(
    conn: &mut PgConnection,
    guild: GuildId,
    ticket: TicketId,
    retention_days: u16,
) -> Result<bool, DbError> {
    let done = obs::run(
        Update,
        "tickets",
        sqlx::query!(
            "UPDATE tickets SET state = 'closed', closed_at = now(), updated_at = now()
             WHERE guild_id = $1 AND id = $2 AND state = 'closing'",
            i64::from(guild),
            ticket.get(),
        )
        .execute(&mut *conn),
    )
    .await?;
    if done.rows_affected() == 0 {
        return Ok(false);
    }
    obs::run(
        Update,
        "ticket_messages",
        sqlx::query!(
            "UPDATE ticket_messages SET expires_at = now() + make_interval(days => $3)
             WHERE guild_id = $1 AND ticket_id = $2 AND expires_at IS NULL",
            i64::from(guild),
            ticket.get(),
            i32::from(retention_days),
        )
        .execute(conn),
    )
    .await?;
    Ok(true)
}

/// Close step 4: the thread was deleted, or Discord refused (`failed`).
/// Never recorded for a ticket that is still opening or open.
pub async fn mark_thread_deleted(
    conn: &mut PgConnection,
    guild: GuildId,
    ticket: TicketId,
    failed: bool,
) -> Result<bool, DbError> {
    let done = obs::run(
        Update,
        "tickets",
        sqlx::query!(
            "UPDATE tickets SET thread_deleted_at = CASE WHEN $3 THEN thread_deleted_at ELSE now() END,
                                thread_delete_failed = $3, updated_at = now()
             WHERE guild_id = $1 AND id = $2 AND thread_deleted_at IS NULL
               AND state NOT IN ('opening', 'open')",
            i64::from(guild),
            ticket.get(),
            failed,
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}
