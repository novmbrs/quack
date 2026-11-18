//! Action attempts: opened before the Discord call, finished after.

use quack_core::action::AttemptOutcome;
use quack_core::ids::{ActionId, CaseId, GuildId};
use sqlx::PgConnection;
use time::OffsetDateTime;

use crate::DbError;
use crate::obs::{self, Insert, Select, Update};
use crate::pg::{Pg, count, int};

/// One attempt at an action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Attempt {
    /// The action.
    pub action: ActionId,
    /// Its number, counting across rounds (1-based).
    pub number: u32,
    /// When it started (committed before the Discord call).
    pub started_at: OffsetDateTime,
    /// When it finished; `None` while open.
    pub finished_at: Option<OffsetDateTime>,
    /// How it ended; `None` while open.
    pub outcome: Option<AttemptOutcome>,
    /// Discord's HTTP status, when it answered.
    pub http_status: Option<u16>,
    /// Discord's JSON error code, when it sent one.
    pub discord_code: Option<u32>,
}

/// Starts an attempt (§5.4 step 1.9): the action goes `pending` →
/// `running` (`attempts + 1`, `last_attempt_at`), and the open attempt row
/// is inserted. Returns the attempt number, or `None` when the action was
/// not `pending`.
pub async fn start(
    conn: &mut PgConnection,
    guild: GuildId,
    action: ActionId,
) -> Result<Option<u32>, DbError> {
    let number = obs::run(
        Insert,
        "action_attempts",
        sqlx::query_scalar!(
            "WITH a AS (
               UPDATE actions SET state = 'running', attempts = attempts + 1, last_attempt_at = now(),
                                  next_attempt_at = NULL, updated_at = now()
               WHERE guild_id = $1 AND id = $2 AND state = 'pending'
               RETURNING guild_id, id
             )
             INSERT INTO action_attempts (guild_id, action_id, number, started_at)
             SELECT a.guild_id, a.id,
                    COALESCE((SELECT max(number) FROM action_attempts WHERE action_id = a.id), 0) + 1, now()
             FROM a
             RETURNING number",
            i64::from(guild),
            action.get(),
        )
        .fetch_optional(conn),
    )
    .await?;
    number.map(count).transpose()
}

/// Finishes the open attempt `number`. Returns `false` when it is not open
/// (another run already settled it).
pub async fn finish_attempt(
    conn: &mut PgConnection,
    guild: GuildId,
    action: ActionId,
    number: u32,
    outcome: AttemptOutcome,
    http_status: Option<u16>,
    discord_code: Option<u32>,
) -> Result<bool, DbError> {
    let done = obs::run(
        Update,
        "action_attempts",
        sqlx::query!(
            "UPDATE action_attempts SET finished_at = now(), outcome = $4, http_status = $5, discord_code = $6
             WHERE guild_id = $1 AND action_id = $2 AND number = $3 AND finished_at IS NULL",
            i64::from(guild),
            action.get(),
            int::<u32, i32>(number)?,
            Pg(outcome) as _,
            http_status.map(int::<u16, i16>).transpose()?,
            discord_code.map(int::<u32, i32>).transpose()?,
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// The open attempt's number, if one is open.
pub async fn open_attempt(
    conn: &mut PgConnection,
    action: ActionId,
) -> Result<Option<u32>, DbError> {
    let number = obs::run(
        Select,
        "action_attempts",
        sqlx::query_scalar!(
            "SELECT number FROM action_attempts WHERE action_id = $1 AND finished_at IS NULL",
            action.get()
        )
        .fetch_optional(conn),
    )
    .await?;
    number.map(count).transpose()
}

struct AttemptRow {
    action_id: uuid::Uuid,
    number: i32,
    started_at: OffsetDateTime,
    finished_at: Option<OffsetDateTime>,
    outcome: Option<Pg<AttemptOutcome>>,
    http_status: Option<i16>,
    discord_code: Option<i32>,
}

impl TryFrom<AttemptRow> for Attempt {
    type Error = DbError;

    fn try_from(r: AttemptRow) -> Result<Self, DbError> {
        Ok(Self {
            action: ActionId::from_uuid(r.action_id),
            number: count(r.number)?,
            started_at: r.started_at,
            finished_at: r.finished_at,
            outcome: r.outcome.map(|o| o.0),
            http_status: r.http_status.map(count).transpose()?,
            discord_code: r.discord_code.map(count).transpose()?,
        })
    }
}

/// Every attempt of an action, oldest first. The current round is the last
/// `Action::attempts` of them.
pub async fn attempts(conn: &mut PgConnection, action: ActionId) -> Result<Vec<Attempt>, DbError> {
    let rows = obs::run(
        Select,
        "action_attempts",
        sqlx::query_as!(
            AttemptRow,
            "SELECT action_id, number, started_at, finished_at, outcome, http_status, discord_code
             FROM action_attempts WHERE action_id = $1 ORDER BY number",
            action.get()
        )
        .fetch_all(conn),
    )
    .await?;
    rows.into_iter().map(Attempt::try_from).collect()
}

/// Every attempt of a case's actions, oldest first (case views and the
/// member timeline).
pub async fn attempts_for_case(
    conn: &mut PgConnection,
    guild: GuildId,
    case: CaseId,
) -> Result<Vec<Attempt>, DbError> {
    let rows = obs::run(
        Select,
        "action_attempts",
        sqlx::query_as!(
            AttemptRow,
            "SELECT t.action_id, t.number, t.started_at, t.finished_at, t.outcome, t.http_status, t.discord_code
             FROM action_attempts t JOIN actions a ON a.id = t.action_id
             WHERE a.guild_id = $1 AND a.case_id = $2
             ORDER BY t.started_at, t.number",
            i64::from(guild),
            case.get()
        )
        .fetch_all(conn),
    )
    .await?;
    rows.into_iter().map(Attempt::try_from).collect()
}
