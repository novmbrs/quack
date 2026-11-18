//! Appeals (§8) and their event timeline.

use quack_core::appeal::{AppealEventKind, AppealState};
use quack_core::ids::{AccountId, AppealEventId, AppealId, CaseId, GuildId, MemberId};
use sqlx::PgConnection;
use time::OffsetDateTime;

use crate::DbError;
use crate::obs::{self, Insert, Select, Update};
use crate::pg::{Pg, count, id, int};

/// An appeal row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Appeal {
    /// The appeal.
    pub id: AppealId,
    /// Its server.
    pub guild: GuildId,
    /// Its case.
    pub case: CaseId,
    /// The case's member.
    pub member: MemberId,
    /// Its state.
    pub state: AppealState,
    /// Incremented by every transition; every transition names the version
    /// it expects.
    pub version: u32,
    /// The member's statement.
    pub statement: String,
    /// The latest staff decision: who, when, and why.
    pub decision: Option<(AccountId, OffsetDateTime, String)>,
    /// The first accept, reject, or close (for the decision-time metric).
    pub first_decided_at: Option<OffsetDateTime>,
    /// When it was submitted.
    pub created_at: OffsetDateTime,
    /// When it last changed.
    pub updated_at: OffsetDateTime,
}

/// One event on an appeal's timeline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppealEvent {
    /// The event.
    pub id: AppealEventId,
    /// Its appeal.
    pub appeal: AppealId,
    /// What happened.
    pub kind: AppealEventKind,
    /// The member or staff; `None` = Quack.
    pub actor: Option<AccountId>,
    /// The statement, reply, or decision reason.
    pub text: String,
    /// Quack closed the appeal because its case was voided.
    pub closed_by_void: bool,
    /// When.
    pub created_at: OffsetDateTime,
}

/// Inserts a `pending` appeal (version 1). A second appeal for a case is
/// [`Conflict::AppealExists`](crate::Conflict::AppealExists).
pub async fn insert(
    conn: &mut PgConnection,
    appeal: AppealId,
    guild: GuildId,
    case: CaseId,
    member: MemberId,
    statement: &str,
) -> Result<(), DbError> {
    obs::run(
        Insert,
        "appeals",
        sqlx::query!(
            "INSERT INTO appeals (id, guild_id, case_id, member_id, statement) VALUES ($1, $2, $3, $4, $5)",
            appeal.get(),
            i64::from(guild),
            case.get(),
            i64::from(member),
            statement,
        )
        .execute(conn),
    )
    .await?;
    Ok(())
}

/// Appends an event to an appeal's timeline.
pub async fn insert_event(
    conn: &mut PgConnection,
    guild: GuildId,
    event: &AppealEvent,
) -> Result<(), DbError> {
    obs::run(
        Insert,
        "appeal_events",
        sqlx::query!(
            "INSERT INTO appeal_events (id, guild_id, appeal_id, kind, actor_id, text, closed_by_void, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
            event.id.get(),
            i64::from(guild),
            event.appeal.get(),
            Pg(event.kind) as _,
            event.actor.map(i64::from),
            event.text,
            event.closed_by_void,
            event.created_at,
        )
        .execute(conn),
    )
    .await?;
    Ok(())
}

struct AppealRow {
    id: uuid::Uuid,
    guild_id: i64,
    case_id: uuid::Uuid,
    member_id: i64,
    state: Pg<AppealState>,
    version: i32,
    statement: String,
    decided_by: Option<i64>,
    decided_at: Option<OffsetDateTime>,
    decision_reason: Option<String>,
    first_decided_at: Option<OffsetDateTime>,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

impl TryFrom<AppealRow> for Appeal {
    type Error = DbError;

    fn try_from(r: AppealRow) -> Result<Self, DbError> {
        let decision = match (r.decided_by, r.decided_at, r.decision_reason) {
            (Some(by), Some(at), Some(reason)) => Some((id(by)?, at, reason)),
            _ => None,
        };
        Ok(Self {
            id: AppealId::from_uuid(r.id),
            guild: id(r.guild_id)?,
            case: CaseId::from_uuid(r.case_id),
            member: id(r.member_id)?,
            state: r.state.0,
            version: count(r.version)?,
            statement: r.statement,
            decision,
            first_decided_at: r.first_decided_at,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
    }
}

macro_rules! query_appeal {
    ($tail:literal $(, $arg:expr)* $(,)?) => {
        ::sqlx::sqlx_macros::expand_query!(
            record = AppealRow,
            source = "SELECT id, guild_id, case_id, member_id, state, version, statement, decided_by, decided_at,
                             decision_reason, first_decided_at, created_at, updated_at
                      FROM appeals " + $tail,
            args = [$($arg),*]
        )
    };
}

/// An appeal of this server.
pub async fn get(
    conn: &mut PgConnection,
    guild: GuildId,
    appeal: AppealId,
) -> Result<Option<Appeal>, DbError> {
    let row = obs::run(
        Select,
        "appeals",
        query_appeal!(
            "WHERE guild_id = $1 AND id = $2",
            i64::from(guild),
            appeal.get()
        )
        .fetch_optional(conn),
    )
    .await?;
    row.map(Appeal::try_from).transpose()
}

/// A case's appeal, if it has one.
pub async fn for_case(
    conn: &mut PgConnection,
    guild: GuildId,
    case: CaseId,
) -> Result<Option<Appeal>, DbError> {
    let row = obs::run(
        Select,
        "appeals",
        query_appeal!(
            "WHERE guild_id = $1 AND case_id = $2",
            i64::from(guild),
            case.get()
        )
        .fetch_optional(conn),
    )
    .await?;
    row.map(Appeal::try_from).transpose()
}

/// Locks a case's appeal `FOR UPDATE` (after the case, §4.12).
pub async fn lock_for_case(
    conn: &mut PgConnection,
    guild: GuildId,
    case: CaseId,
) -> Result<Option<Appeal>, DbError> {
    let row = obs::run(
        Select,
        "appeals",
        query_appeal!(
            "WHERE guild_id = $1 AND case_id = $2 FOR UPDATE",
            i64::from(guild),
            case.get()
        )
        .fetch_optional(conn),
    )
    .await?;
    row.map(Appeal::try_from).transpose()
}

/// A staff decision recorded with a transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decision<'a> {
    /// Who decided.
    pub by: AccountId,
    /// Their reason (1–2,000 characters).
    pub reason: &'a str,
}

/// Moves an appeal `from` → `to` if it is still in `from` at
/// `expected_version` (`None`: any version, for Quack's close on void).
/// Records a staff decision when given, and the first accept, reject, or
/// close. Returns the new version, or `None` when the appeal changed.
pub async fn transition(
    conn: &mut PgConnection,
    guild: GuildId,
    appeal: AppealId,
    from: AppealState,
    expected_version: Option<u32>,
    to: AppealState,
    decision: Option<Decision<'_>>,
) -> Result<Option<u32>, DbError> {
    let version = obs::run(
        Update,
        "appeals",
        sqlx::query_scalar!(
            "UPDATE appeals SET state = $5, version = version + 1, updated_at = now(),
                                decided_by = CASE WHEN $6::bigint IS NULL THEN decided_by ELSE $6 END,
                                decided_at = CASE WHEN $6::bigint IS NULL THEN decided_at ELSE now() END,
                                decision_reason = CASE WHEN $6::bigint IS NULL THEN decision_reason ELSE $7 END,
                                first_decided_at = CASE WHEN $5::appeal_state IN ('accepted', 'rejected', 'closed')
                                                        THEN COALESCE(first_decided_at, now())
                                                        ELSE first_decided_at END
             WHERE guild_id = $1 AND id = $2 AND state = $3 AND ($4::integer IS NULL OR version = $4)
             RETURNING version",
            i64::from(guild),
            appeal.get(),
            Pg(from) as _,
            expected_version.map(int::<u32, i32>).transpose()?,
            Pg(to) as _,
            decision.map(|d| i64::from(d.by)),
            decision.map(|d| d.reason),
        )
        .fetch_optional(conn),
    )
    .await?;
    version.map(count).transpose()
}

mod list;

pub use list::{AppealSummary, count_open, count_pending, event, events, list, pending};
