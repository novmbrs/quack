//! Reading the moderation log (§9.6) and the facts an entry's sentence
//! needs (§9.4: immutable records, plus display names read live for the
//! dashboard).

use quack_core::DomainError;
use quack_core::ids::{
    AccountId, ActionId, AppealId, CaseId, MemberId, ModLogId, RuleId, TicketId,
};
use quack_core::modlog::{ModLogEntry, ModLogKind, ModLogResult, ModLogSource};
use quack_core::permissions::Capability;
use quack_db::modlog::{ModLogFilter, StoredEntry};
use quack_db::{DbError, PgConnection, TimeCursor, actions, cases, rules, tickets};
use quack_text::ChannelRef;
use quack_text::log::LogFacts;
use quack_text::render::plain;
use serde::Serialize;
use time::OffsetDateTime;

use crate::names::Names;
use crate::{Engine, Principal, obs};

/// One entry as the dashboard shows it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EntryView {
    /// The entry.
    pub id: ModLogId,
    /// Its kind.
    pub kind: ModLogKind,
    /// Its result.
    pub result: ModLogResult,
    /// Who acted; `None` = Quack.
    pub actor_id: Option<AccountId>,
    /// Where it came from.
    pub source: ModLogSource,
    /// The case.
    pub case_id: Option<CaseId>,
    /// The case's number.
    pub case_number: Option<u64>,
    /// The member.
    pub member_id: Option<MemberId>,
    /// The appeal.
    pub appeal_id: Option<AppealId>,
    /// The action.
    pub action_id: Option<ActionId>,
    /// The rule.
    pub rule_id: Option<RuleId>,
    /// The ticket.
    pub ticket_id: Option<TicketId>,
    /// The typed detail.
    pub detail: serde_json::Value,
    /// The rendered sentence (§12.10).
    pub text: String,
    /// When.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// One page of the moderation log, newest first.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ModLogPage {
    /// The entries.
    pub entries: Vec<EntryView>,
    /// The cursor for the next page; `None` on the last page.
    #[serde(skip)]
    pub next_cursor: Option<TimeCursor>,
}

/// The moderation log of the principal's server (§9.6).
pub async fn list(
    engine: &Engine,
    principal: &Principal,
    filter: &ModLogFilter,
    cursor: Option<TimeCursor>,
    limit: u32,
) -> Result<Result<ModLogPage, DomainError>, DbError> {
    obs::use_case("list_mod_log", Some(principal.guild), async {
        let mut conn = engine.db().conn().await?;
        if let Err(refusal) = principal.require(&mut conn, Capability::Moderate).await? {
            return Ok(Err(refusal));
        }
        let limit = limit.clamp(1, quack_core::limits::MAX_PAGE_SIZE);
        let rows =
            quack_db::modlog::list(&mut conn, principal.guild, filter, cursor, limit).await?;
        let next_cursor = rows
            .more
            .then(|| {
                rows.rows.last().map(|e| TimeCursor {
                    created_at: e.created_at,
                    id: e.id.get(),
                })
            })
            .flatten();
        let people = rows
            .rows
            .iter()
            .flat_map(|e| [e.actor, e.links.member_id.map(MemberId::account)])
            .flatten()
            .collect();
        let names = Names::read(engine, people).await;
        let mut entries = Vec::new();
        for entry in &rows.rows {
            entries.push(entry_view(engine, &mut conn, entry, &names).await?);
        }
        Ok(Ok(ModLogPage {
            entries,
            next_cursor,
        }))
    })
    .await
}

async fn entry_view(
    engine: &Engine,
    conn: &mut PgConnection,
    entry: &StoredEntry,
    names: &Names,
) -> Result<EntryView, DbError> {
    let facts = facts(engine, conn, entry, false, names).await?;
    Ok(EntryView {
        id: entry.id,
        kind: entry.kind,
        result: entry.result,
        actor_id: entry.actor,
        source: entry.source,
        case_id: entry.links.case_id,
        case_number: facts.case_number,
        member_id: entry.links.member_id,
        appeal_id: entry.links.appeal_id,
        action_id: entry.links.action_id,
        rule_id: entry.links.rule_id,
        ticket_id: entry.links.ticket_id,
        detail: entry.entry.detail().unwrap_or_default(),
        text: plain(&quack_text::log::text(&entry.entry, &facts)),
        created_at: entry.created_at,
    })
}

/// An entry's sentence for the dashboard, naming people from `names`.
pub async fn entry_text(
    engine: &Engine,
    conn: &mut PgConnection,
    entry: &StoredEntry,
    names: &Names,
) -> Result<String, DbError> {
    let facts = facts(engine, conn, entry, false, names).await?;
    Ok(plain(&quack_text::log::text(&entry.entry, &facts)))
}

/// The names and links an entry's sentence needs. `post` adds the links a
/// log channel post carries and, for a retryable failed action, the Retry
/// button's action (§9.4). People are named from `names` (plain text only;
/// a post mentions them).
pub(crate) async fn facts(
    engine: &Engine,
    conn: &mut PgConnection,
    entry: &StoredEntry,
    post: bool,
    names: &Names,
) -> Result<LogFacts, DbError> {
    let guild = entry.guild;
    let links = engine.links();
    let mut f = LogFacts::new(entry.created_at, names.actor(entry.actor));
    f.member = entry.links.member_id.map(|m| names.person(m.account()));
    let case = match entry.links.case_id {
        Some(id) => cases::get(conn, guild, id).await?,
        None => None,
    };
    if let Some(case) = &case {
        f.case_number = Some(case.number);
        f.void_reason = case.void.as_ref().map(|v| v.reason.clone());
        if post {
            f.case_url = Some(links.case(guild, case.number));
        }
    }
    // An entry about a case names the rule as the case's version had it
    // (a later rename doesn't rewrite history); only an entry about a rule
    // itself uses the rule's current name.
    f.rule = match (&case, entry.links.rule_id) {
        (Some(_), _) | (None, None) => case_rule_name(conn, case.as_ref()).await?,
        (None, Some(rule)) => rules::get(conn, guild, rule)
            .await?
            .map(|r| r.definition.name),
    };
    if let Some(action) = entry.links.action_id {
        let found = actions::get(conn, guild, action).await?;
        f.timeout_ends_at = found.as_ref().and_then(|a| a.timeout_ends_at);
        if post
            && matches!(entry.entry, ModLogEntry::ActionFailed(_))
            && let (Some(a), Some(c)) = (found, &case)
        {
            let records = crate::cases::CaseRecords::of(conn, c.clone()).await?;
            if records.controls(&a, OffsetDateTime::now_utc()).can_retry() {
                f.retry_action = Some(a.id);
            }
        }
    }
    if let Some(ticket) = entry.links.ticket_id {
        f.thread = tickets::get(conn, guild, ticket)
            .await?
            .and_then(|t| t.thread)
            .map(|id| ChannelRef { id, name: None });
    }
    if post {
        f.dashboard_url = Some(dashboard_url(engine, entry, f.case_number));
    }
    Ok(f)
}

async fn case_rule_name(
    conn: &mut PgConnection,
    case: Option<&quack_db::cases::Case>,
) -> Result<Option<String>, DbError> {
    let Some((rule, version)) = case.and_then(|c| c.rule) else {
        return Ok(None);
    };
    let guild = case.map(|c| c.guild);
    let Some(guild) = guild else {
        return Ok(None);
    };
    Ok(rules::version(conn, guild, rule, version)
        .await?
        .map(|v| v.definition.name))
}

/// The dashboard page an entry links to (§14.11).
fn dashboard_url(engine: &Engine, entry: &StoredEntry, case_number: Option<u64>) -> String {
    let links = engine.links();
    let guild = entry.guild;
    if let Some(appeal) = entry.links.appeal_id {
        return links.appeal(guild, appeal);
    }
    if matches!(entry.entry, ModLogEntry::ActionFailed(_)) {
        return links.failures(guild);
    }
    if let Some(number) = case_number {
        return links.case(guild, number);
    }
    if let Some(rule) = entry.links.rule_id {
        return links.rule(guild, rule);
    }
    match entry.kind {
        ModLogKind::SettingsChanged | ModLogKind::ChannelSettingCleared => links.settings(guild),
        _ => links.mod_log(guild),
    }
}
