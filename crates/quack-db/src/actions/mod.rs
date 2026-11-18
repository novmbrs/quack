//! Actions (§5): original and undo actions, their attempts, and the
//! member-serialized section they run in.

mod attempts;
mod list;
mod state;

use std::time::Duration;

use quack_core::action::{ActionKind, ActionSkipReason, ActionState, Failure};
use quack_core::ids::{AccountId, ActionId, CaseId, GuildId, MemberId};
use quack_core::permissions::DiscordPermission;
use sqlx::PgConnection;
use time::OffsetDateTime;

use crate::DbError;
use crate::obs::{self, Insert, Select};
use crate::pg::{Pg, count, id, int, opt_id};

pub use attempts::{Attempt, attempts, attempts_for_case, finish_attempt, open_attempt, start};
pub use list::{ActionListItem, count_failed, failed_by_kind, list};
pub use state::{
    Expected, back_to_pending, cancel_pending_original, dismiss, guard, lock_member,
    mark_cancelled, mark_done, mark_failed, mark_skipped, member_running, retry, voided_ban_case,
};

/// An action row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Action {
    /// The action.
    pub id: ActionId,
    /// Its server.
    pub guild: GuildId,
    /// Its case.
    pub case: CaseId,
    /// The case's member.
    pub member: MemberId,
    /// What it does.
    pub kind: ActionKind,
    /// For undo actions, the original it undoes.
    pub undo_of: Option<ActionId>,
    /// Its state.
    pub state: ActionState,
    /// For timeouts: the fixed end, whole seconds (§5.2).
    pub timeout_ends_at: Option<OffsetDateTime>,
    /// For bans: how much message history Discord deletes.
    pub delete_message_seconds: Option<u32>,
    /// Staff who requested an undo or retry; `None` = Quack.
    pub requested_by: Option<AccountId>,
    /// Attempts in the current round.
    pub attempts: u32,
    /// When the next attempt is due, while `pending` after a retry.
    pub next_attempt_at: Option<OffsetDateTime>,
    /// When the last attempt started.
    pub last_attempt_at: Option<OffsetDateTime>,
    /// Why it failed, while `failed`.
    pub failure: Option<Failure>,
    /// Why an undo was skipped, and the case that still applies.
    pub skip: Option<(ActionSkipReason, Option<CaseId>)>,
    /// Done because the desired state already held.
    pub no_change: bool,
    /// When it became `done`.
    pub done_at: Option<OffsetDateTime>,
    /// When a failure was dismissed, and by whom (`None` for some
    /// imported history, which didn't record who).
    pub dismissed: Option<(OffsetDateTime, Option<AccountId>)>,
    /// When it was created (the case's creation, or the undo request).
    pub created_at: OffsetDateTime,
}

/// A new action, `pending`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NewAction {
    /// A fresh `UUIDv7`.
    pub id: ActionId,
    /// The server.
    pub guild: GuildId,
    /// The case.
    pub case: CaseId,
    /// The case's member.
    pub member: MemberId,
    /// What it does.
    pub kind: ActionKind,
    /// The original, for undo actions (same case).
    pub undo_of: Option<ActionId>,
    /// For timeouts.
    pub timeout_ends_at: Option<OffsetDateTime>,
    /// For bans.
    pub delete_message_seconds: Option<u32>,
    /// Staff who requested an undo; `None` = Quack.
    pub requested_by: Option<AccountId>,
    /// The case's `created_at` for originals; `None` = now (undos).
    pub created_at: Option<OffsetDateTime>,
}

/// Inserts an action. A second original for a case, or a second undo for an
/// original, is [`Conflict::ActionExists`](crate::Conflict::ActionExists).
pub async fn insert(conn: &mut PgConnection, action: &NewAction) -> Result<(), DbError> {
    obs::run(
        Insert,
        "actions",
        sqlx::query!(
            "INSERT INTO actions (id, guild_id, case_id, member_id, kind, undo_of_action_id, timeout_ends_at,
                                  delete_message_seconds, requested_by, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, COALESCE($10, now()), COALESCE($10, now()))",
            action.id.get(),
            i64::from(action.guild),
            action.case.get(),
            i64::from(action.member),
            Pg(action.kind) as _,
            action.undo_of.map(ActionId::get),
            action.timeout_ends_at,
            action.delete_message_seconds.map(int::<u32, i32>).transpose()?,
            action.requested_by.map(i64::from),
            action.created_at,
        )
        .execute(conn),
    )
    .await?;
    Ok(())
}

pub(crate) struct ActionRow {
    id: uuid::Uuid,
    guild_id: i64,
    case_id: uuid::Uuid,
    member_id: i64,
    kind: Pg<ActionKind>,
    undo_of_action_id: Option<uuid::Uuid>,
    state: Pg<ActionState>,
    timeout_ends_at: Option<OffsetDateTime>,
    delete_message_seconds: Option<i32>,
    requested_by: Option<i64>,
    attempts: i32,
    next_attempt_at: Option<OffsetDateTime>,
    last_attempt_at: Option<OffsetDateTime>,
    failure_reason: Option<Pg<quack_core::action::ActionFailure>>,
    failure_permission: Option<Pg<DiscordPermission>>,
    failure_http_status: Option<i16>,
    failure_discord_code: Option<i32>,
    skip_reason: Option<Pg<ActionSkipReason>>,
    skipped_because_case_id: Option<uuid::Uuid>,
    no_change: bool,
    done_at: Option<OffsetDateTime>,
    dismissed_at: Option<OffsetDateTime>,
    dismissed_by: Option<i64>,
    created_at: OffsetDateTime,
}

impl TryFrom<ActionRow> for Action {
    type Error = DbError;

    fn try_from(r: ActionRow) -> Result<Self, DbError> {
        let failure = r
            .failure_reason
            .map(|reason| {
                Ok::<_, DbError>(Failure {
                    reason: reason.0,
                    permission: r.failure_permission.map(|p| p.0),
                    http_status: r.failure_http_status.map(count).transpose()?,
                    discord_code: r.failure_discord_code.map(count).transpose()?,
                })
            })
            .transpose()?;
        let dismissed = match r.dismissed_at {
            Some(at) => Some((at, opt_id(r.dismissed_by)?)),
            None => None,
        };
        Ok(Self {
            id: ActionId::from_uuid(r.id),
            guild: id(r.guild_id)?,
            case: CaseId::from_uuid(r.case_id),
            member: id(r.member_id)?,
            kind: r.kind.0,
            undo_of: r.undo_of_action_id.map(ActionId::from_uuid),
            state: r.state.0,
            timeout_ends_at: r.timeout_ends_at,
            delete_message_seconds: r.delete_message_seconds.map(count).transpose()?,
            requested_by: opt_id(r.requested_by)?,
            attempts: count(r.attempts)?,
            next_attempt_at: r.next_attempt_at,
            last_attempt_at: r.last_attempt_at,
            failure,
            skip: r
                .skip_reason
                .map(|reason| (reason.0, r.skipped_because_case_id.map(CaseId::from_uuid))),
            no_change: r.no_change,
            done_at: r.done_at,
            dismissed,
            created_at: r.created_at,
        })
    }
}

/// `query_as!` for [`ActionRow`] with the shared column list; `$tail`
/// follows `FROM actions`.
macro_rules! query_action {
    ($tail:literal $(, $arg:expr)* $(,)?) => {
        ::sqlx::sqlx_macros::expand_query!(
            record = ActionRow,
            source = "SELECT id, guild_id, case_id, member_id, kind, undo_of_action_id, state, timeout_ends_at,
                             delete_message_seconds, requested_by, attempts, next_attempt_at, last_attempt_at,
                             failure_reason, failure_permission, failure_http_status, failure_discord_code,
                             skip_reason, skipped_because_case_id, no_change, done_at, dismissed_at,
                             dismissed_by, created_at
                      FROM actions " + $tail,
            args = [$($arg),*]
        )
    };
}

/// An action of this server.
pub async fn get(
    conn: &mut PgConnection,
    guild: GuildId,
    action: ActionId,
) -> Result<Option<Action>, DbError> {
    let row = obs::run(
        Select,
        "actions",
        query_action!(
            "WHERE guild_id = $1 AND id = $2",
            i64::from(guild),
            action.get()
        )
        .fetch_optional(conn),
    )
    .await?;
    row.map(Action::try_from).transpose()
}

/// Locks an action `FOR UPDATE` (after its case, §4.12).
pub async fn lock(
    conn: &mut PgConnection,
    guild: GuildId,
    action: ActionId,
) -> Result<Option<Action>, DbError> {
    let row = obs::run(
        Select,
        "actions",
        query_action!(
            "WHERE guild_id = $1 AND id = $2 FOR UPDATE",
            i64::from(guild),
            action.get()
        )
        .fetch_optional(conn),
    )
    .await?;
    row.map(Action::try_from).transpose()
}

/// Locks a case's original action `FOR UPDATE`, if it has one (void).
pub async fn lock_original(
    conn: &mut PgConnection,
    guild: GuildId,
    case: CaseId,
) -> Result<Option<Action>, DbError> {
    let row = obs::run(
        Select,
        "actions",
        query_action!(
            "WHERE guild_id = $1 AND case_id = $2 AND undo_of_action_id IS NULL FOR UPDATE",
            i64::from(guild),
            case.get()
        )
        .fetch_optional(conn),
    )
    .await?;
    row.map(Action::try_from).transpose()
}

/// A case's actions: the original first, then its undo.
pub async fn for_case(
    conn: &mut PgConnection,
    guild: GuildId,
    case: CaseId,
) -> Result<Vec<Action>, DbError> {
    let rows = obs::run(
        Select,
        "actions",
        query_action!(
            "WHERE guild_id = $1 AND case_id = $2 ORDER BY undo_of_action_id IS NOT NULL, created_at, id",
            i64::from(guild),
            case.get()
        )
        .fetch_all(conn),
    )
    .await?;
    rows.into_iter().map(Action::try_from).collect()
}

/// The undo of an original action, if one exists.
pub async fn undo_of(
    conn: &mut PgConnection,
    guild: GuildId,
    original: ActionId,
) -> Result<Option<Action>, DbError> {
    let row = obs::run(
        Select,
        "actions",
        query_action!(
            "WHERE guild_id = $1 AND undo_of_action_id = $2",
            i64::from(guild),
            original.get()
        )
        .fetch_optional(conn),
    )
    .await?;
    row.map(Action::try_from).transpose()
}

/// Seconds as the `interval` argument the state SQL takes.
pub(crate) fn secs(delay: Duration) -> f64 {
    delay.as_secs_f64()
}
