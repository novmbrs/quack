//! The append-only moderation log (§9).

use quack_core::ids::{
    AccountId, ActionId, AppealId, CaseId, ChannelId, GuildId, MemberId, ModLogId, RuleId, TicketId,
};
use quack_core::modlog::{ModLogEntry, ModLogKind, ModLogLinks, ModLogResult, ModLogSource};
use sqlx::PgConnection;
use time::OffsetDateTime;

use crate::jobs::{self, NewJob};
use crate::obs::{self, AfterCommit, Insert, Select};
use crate::pg::{Pg, TimeCursor, cursor_parts, id, limit, opt_id};
use crate::{DbError, Tx};

/// An entry to write. Its links must satisfy the kind's link rules
/// (§9.2) and are taken from one loaded record by the caller.
#[derive(Clone, Debug, PartialEq)]
pub struct NewEntry<'a> {
    /// The server.
    pub guild: GuildId,
    /// The typed entry (its kind and `detail`).
    pub entry: &'a ModLogEntry,
    /// Who acted; `None` = Quack.
    pub actor: Option<AccountId>,
    /// Where it came from.
    pub source: ModLogSource,
    /// The typed links.
    pub links: ModLogLinks,
    /// The result for `legacy` entries, which keep the imported one; every
    /// other kind's result is fixed by its kind.
    pub legacy_result: Option<ModLogResult>,
}

/// What [`insert`] wrote.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Written {
    /// The entry.
    pub id: ModLogId,
    /// The log channel it will be posted to, when its kind is posted and a
    /// log channel is set (its `post_mod_log` job is enqueued).
    pub posted_to: Option<ChannelId>,
}

/// Appends an entry in the caller's transaction and, when its kind is
/// posted and the server has a log channel, enqueues its `post_mod_log`
/// job pinned to that channel (§9.4). `quack.modlog.entries` counts on
/// commit.
pub async fn insert(tx: &mut Tx, new: &NewEntry<'_>) -> Result<Written, DbError> {
    let kind = new.entry.kind();
    new.links
        .check(kind)
        .map_err(|problem| DbError::Invalid(problem.to_string()))?;
    let result = new
        .entry
        .result()
        .or(new.legacy_result)
        .ok_or_else(|| DbError::Invalid("legacy entry without a result".to_owned()))?;
    let detail = new
        .entry
        .detail()
        .map_err(|error| DbError::Invalid(format!("mod log detail: {error}")))?;
    let trace_id = quack_telemetry::current_trace_context().and_then(|tp| trace_id_of(&tp));
    let entry_id = ModLogId::from_uuid(uuid::Uuid::now_v7());
    let l = new.links;
    obs::run(
        Insert,
        "mod_log",
        sqlx::query!(
            "INSERT INTO mod_log (id, guild_id, kind, result, actor_id, source, case_id, member_id, appeal_id,
                                  action_id, rule_id, ticket_id, detail, trace_id)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
            entry_id.get(),
            i64::from(new.guild),
            Pg(kind) as _,
            Pg(result) as _,
            new.actor.map(i64::from),
            Pg(new.source) as _,
            l.case_id.map(CaseId::get),
            l.member_id.map(i64::from),
            l.appeal_id.map(AppealId::get),
            l.action_id.map(ActionId::get),
            l.rule_id.map(RuleId::get),
            l.ticket_id.map(TicketId::get),
            detail,
            trace_id,
        )
        .execute(&mut **tx),
    )
    .await?;
    tx.after_commit(AfterCommit::ModLogEntry(kind, result));
    let posted_to = if new.entry.is_posted() {
        log_channel(tx, new.guild).await?
    } else {
        None
    };
    if let Some(channel) = posted_to {
        jobs::enqueue(tx, &NewJob::post_mod_log(new.guild, entry_id, channel)).await?;
    }
    Ok(Written {
        id: entry_id,
        posted_to,
    })
}

async fn log_channel(
    conn: &mut PgConnection,
    guild: GuildId,
) -> Result<Option<ChannelId>, DbError> {
    let channel = obs::run(
        Select,
        "guild_settings",
        sqlx::query_scalar!(
            "SELECT log_channel_id FROM guild_settings WHERE guild_id = $1",
            i64::from(guild)
        )
        .fetch_optional(conn),
    )
    .await?
    .flatten();
    channel.map(id).transpose()
}

/// The trace ID of a W3C `traceparent` (`00-<32 hex>-<16 hex>-<flags>`).
fn trace_id_of(traceparent: &str) -> Option<String> {
    let trace = traceparent.split('-').nth(1)?;
    (trace.len() == 32
        && trace
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
    .then(|| trace.to_owned())
}

/// A stored entry.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredEntry {
    /// The entry.
    pub id: ModLogId,
    /// Its server.
    pub guild: GuildId,
    /// The stored kind (always right, even when `detail` can't be read).
    pub kind: ModLogKind,
    /// The typed entry, parsed from `kind` and `detail`. When the detail
    /// can't be read (`detail_readable` is false) it is a `legacy` entry
    /// naming the kind, so the rest of the page stays visible.
    pub entry: ModLogEntry,
    /// Whether `detail` matched its kind.
    pub detail_readable: bool,
    /// Its result.
    pub result: ModLogResult,
    /// Who acted; `None` = Quack.
    pub actor: Option<AccountId>,
    /// Where it came from.
    pub source: ModLogSource,
    /// The typed links.
    pub links: ModLogLinks,
    /// The trace that wrote it.
    pub trace_id: Option<String>,
    /// When.
    pub created_at: OffsetDateTime,
}

struct EntryRow {
    id: uuid::Uuid,
    guild_id: i64,
    kind: Pg<ModLogKind>,
    result: Pg<ModLogResult>,
    actor_id: Option<i64>,
    source: Pg<ModLogSource>,
    case_id: Option<uuid::Uuid>,
    member_id: Option<i64>,
    appeal_id: Option<uuid::Uuid>,
    action_id: Option<uuid::Uuid>,
    rule_id: Option<uuid::Uuid>,
    ticket_id: Option<uuid::Uuid>,
    detail: serde_json::Value,
    trace_id: Option<String>,
    created_at: OffsetDateTime,
}

impl TryFrom<EntryRow> for StoredEntry {
    type Error = DbError;

    fn try_from(r: EntryRow) -> Result<Self, DbError> {
        let kind = r.kind.0;
        let (entry, detail_readable) = match ModLogEntry::from_detail(kind, r.detail) {
            Ok(entry) => (entry, true),
            Err(error) => {
                tracing::warn!(
                    mod_log_id = %r.id,
                    kind = kind.as_str(),
                    error = %error,
                    "a moderation log entry's detail can't be read; it is shown as unavailable"
                );
                (unreadable(kind), false)
            }
        };
        Ok(Self {
            id: ModLogId::from_uuid(r.id),
            guild: id(r.guild_id)?,
            kind,
            entry,
            detail_readable,
            result: r.result.0,
            actor: opt_id(r.actor_id)?,
            source: r.source.0,
            links: ModLogLinks {
                case_id: r.case_id.map(CaseId::from_uuid),
                member_id: opt_id(r.member_id)?,
                appeal_id: r.appeal_id.map(AppealId::from_uuid),
                action_id: r.action_id.map(ActionId::from_uuid),
                rule_id: r.rule_id.map(RuleId::from_uuid),
                ticket_id: r.ticket_id.map(TicketId::from_uuid),
            },
            trace_id: r.trace_id,
            created_at: r.created_at,
        })
    }
}

/// The stand-in for an entry whose detail can't be read.
fn unreadable(kind: ModLogKind) -> ModLogEntry {
    ModLogEntry::Legacy(quack_core::modlog::LegacyDetail {
        action: kind.as_str().to_owned(),
        resource_type: "unreadable_detail".to_owned(),
        resource_id: None,
        failure_reason: None,
    })
}

macro_rules! query_entry {
    ($tail:literal $(, $arg:expr)* $(,)?) => {
        ::sqlx::sqlx_macros::expand_query!(
            record = EntryRow,
            source = "SELECT id, guild_id, kind, result, actor_id, source, case_id, member_id, appeal_id, action_id,
                             rule_id, ticket_id, detail, trace_id, created_at
                      FROM mod_log " + $tail,
            args = [$($arg),*]
        )
    };
}

/// One entry of this server.
pub async fn get(
    conn: &mut PgConnection,
    guild: GuildId,
    entry: ModLogId,
) -> Result<Option<StoredEntry>, DbError> {
    let row = obs::run(
        Select,
        "mod_log",
        query_entry!(
            "WHERE guild_id = $1 AND id = $2",
            i64::from(guild),
            entry.get()
        )
        .fetch_optional(conn),
    )
    .await?;
    row.map(StoredEntry::try_from).transpose()
}

/// Filters of the moderation log page (§9.6), combined with AND.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModLogFilter {
    /// Any of these kinds (empty: all).
    pub kinds: Vec<ModLogKind>,
    /// This result.
    pub result: Option<ModLogResult>,
    /// By this actor.
    pub actor: Option<AccountId>,
    /// About this member.
    pub member: Option<MemberId>,
    /// About this case.
    pub case: Option<CaseId>,
    /// About this rule.
    pub rule: Option<RuleId>,
    /// About this appeal.
    pub appeal: Option<AppealId>,
    /// Created at or after.
    pub created_after: Option<OffsetDateTime>,
    /// Created before.
    pub created_before: Option<OffsetDateTime>,
}

/// The moderation log, newest first after `cursor`.
pub async fn list(
    conn: &mut PgConnection,
    guild: GuildId,
    filter: &ModLogFilter,
    cursor: Option<TimeCursor>,
    page: u32,
) -> Result<Vec<StoredEntry>, DbError> {
    let (after_time, after_id) = cursor_parts(cursor);
    let kinds: Option<Vec<Pg<ModLogKind>>> =
        (!filter.kinds.is_empty()).then(|| filter.kinds.iter().copied().map(Pg).collect());
    let rows = obs::run(
        Select,
        "mod_log",
        query_entry!(
            "WHERE guild_id = $1
               AND ($2::mod_log_kind[] IS NULL OR kind = ANY($2))
               AND ($3::mod_log_result IS NULL OR result = $3)
               AND ($4::bigint IS NULL OR actor_id = $4)
               AND ($5::bigint IS NULL OR member_id = $5)
               AND ($6::uuid IS NULL OR case_id = $6)
               AND ($7::uuid IS NULL OR rule_id = $7)
               AND ($8::uuid IS NULL OR appeal_id = $8)
               AND ($9::timestamptz IS NULL OR created_at >= $9)
               AND ($10::timestamptz IS NULL OR created_at < $10)
               AND ($11::timestamptz IS NULL OR (created_at, id) < ($11, $12))
             ORDER BY created_at DESC, id DESC
             LIMIT $13",
            i64::from(guild),
            kinds,
            filter.result.map(Pg),
            filter.actor.map(i64::from),
            filter.member.map(i64::from),
            filter.case.map(CaseId::get),
            filter.rule.map(RuleId::get),
            filter.appeal.map(AppealId::get),
            filter.created_after,
            filter.created_before,
            after_time,
            after_id,
            limit(page, 100),
        )
        .fetch_all(conn),
    )
    .await?;
    rows.into_iter().map(StoredEntry::try_from).collect()
}

/// A case's staff timeline, oldest first, at most `page` entries
/// (`mod_log_case`).
pub async fn for_case(
    conn: &mut PgConnection,
    guild: GuildId,
    case: CaseId,
    page: u32,
) -> Result<Vec<StoredEntry>, DbError> {
    let rows = obs::run(
        Select,
        "mod_log",
        query_entry!(
            "WHERE case_id = $1 AND guild_id = $2 ORDER BY created_at, id LIMIT $3",
            case.get(),
            i64::from(guild),
            limit(page, 1000)
        )
        .fetch_all(conn),
    )
    .await?;
    rows.into_iter().map(StoredEntry::try_from).collect()
}

/// When the server's newest `log_channel_failed` entry was written, for the
/// "once per outage" rule (§9.4).
pub async fn last_log_channel_failure(
    conn: &mut PgConnection,
    guild: GuildId,
) -> Result<Option<OffsetDateTime>, DbError> {
    obs::run(
        Select,
        "mod_log",
        sqlx::query_scalar!(
            "SELECT max(created_at) FROM mod_log WHERE guild_id = $1 AND kind = 'log_channel_failed'",
            i64::from(guild)
        )
        .fetch_one(conn),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::trace_id_of;

    #[test]
    fn takes_the_trace_id_of_a_traceparent() {
        assert_eq!(
            trace_id_of("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01").as_deref(),
            Some("4bf92f3577b34da6a3ce929d0e0e4736")
        );
        assert_eq!(trace_id_of("garbage"), None);
        assert_eq!(trace_id_of("00-XYZ-00f067aa0ba902b7-01"), None);
    }
}
