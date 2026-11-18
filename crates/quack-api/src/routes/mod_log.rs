//! The moderation log (route 35, §9.6): filtered, newest first, with each
//! entry's sentence and dashboard links.

use axum::Json;
use axum::extract::{RawQuery, State};
use quack_core::ids::{AccountId, AppealId, CaseId, GuildId, MemberId, ModLogId, RuleId, TicketId};
use quack_core::modlog::{ModLogKind, ModLogResult, ModLogSource};
use quack_db::modlog::ModLogFilter;
use quack_engine::modlog::{self, EntryView};
use quack_text::error::Scene;
use serde::Serialize;
use time::OffsetDateTime;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::error::{ApiError, ApiResult, ErrorEnvelope, Facts};
use crate::extract::{Moderate, Staff, snowflake, uuid_id};
use crate::pagination;
use crate::state::ApiState;
use crate::wire::{parse_ts, schema, ts};

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new().routes(routes!(mod_log))
}

/// Route 35's query (documentation; `kind` may repeat).
#[derive(Debug, IntoParams)]
#[into_params(parameter_in = Query)]
#[allow(
    dead_code,
    reason = "documents the query; parsed by hand so `kind` can repeat"
)]
pub(crate) struct ModLogQuery {
    /// Entry kinds (repeatable).
    kind: Option<Vec<String>>,
    /// `done`, `failed`, … (one result).
    result: Option<String>,
    actor_id: Option<String>,
    member_id: Option<String>,
    case_number: Option<u64>,
    rule_id: Option<String>,
    appeal_id: Option<String>,
    /// Inclusive (RFC 3339).
    created_after: Option<String>,
    /// Exclusive (RFC 3339).
    created_before: Option<String>,
    /// 1–100, default 50.
    limit: Option<u32>,
    cursor: Option<String>,
}

/// Dashboard paths for an entry's typed links.
#[derive(Debug, Default, Serialize, ToSchema)]
pub(crate) struct EntryLinks {
    #[serde(skip_serializing_if = "Option::is_none")]
    case: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    appeal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    member: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rule: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ticket: Option<String>,
}

/// One moderation log entry.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ModLogEntry {
    #[schema(value_type = uuid::Uuid)]
    id: ModLogId,
    #[schema(value_type = schema::ModLogKind)]
    kind: ModLogKind,
    #[schema(value_type = schema::ModLogResult)]
    result: ModLogResult,
    /// Who acted; null = Quack.
    #[schema(value_type = Option<schema::Snowflake>)]
    actor_id: Option<AccountId>,
    #[schema(value_type = schema::ModLogSource)]
    source: ModLogSource,
    #[schema(value_type = Option<uuid::Uuid>)]
    case_id: Option<CaseId>,
    case_number: Option<u64>,
    #[schema(value_type = Option<schema::Snowflake>)]
    member_id: Option<MemberId>,
    #[schema(value_type = Option<uuid::Uuid>)]
    appeal_id: Option<AppealId>,
    #[schema(value_type = Option<uuid::Uuid>)]
    action_id: Option<quack_core::ids::ActionId>,
    #[schema(value_type = Option<uuid::Uuid>)]
    rule_id: Option<RuleId>,
    #[schema(value_type = Option<uuid::Uuid>)]
    ticket_id: Option<TicketId>,
    /// The entry's typed detail.
    #[schema(value_type = Object)]
    detail: serde_json::Value,
    /// The sentence (§12.10).
    text: String,
    links: EntryLinks,
    #[serde(with = "ts")]
    #[schema(value_type = String, format = DateTime)]
    created_at: OffsetDateTime,
}

impl ModLogEntry {
    fn new(guild: GuildId, e: EntryView) -> Self {
        let base = format!("/guilds/{guild}");
        Self {
            links: EntryLinks {
                case: e.case_number.map(|n| format!("{base}/cases/{n}")),
                appeal: e.appeal_id.map(|a| format!("{base}/appeals/{}", a.get())),
                member: e.member_id.map(|m| format!("{base}/members/{m}")),
                rule: e.rule_id.map(|r| format!("{base}/rules/{}", r.get())),
                ticket: e.ticket_id.map(|_| format!("{base}/modules/tickets")),
            },
            id: e.id,
            kind: e.kind,
            result: e.result,
            actor_id: e.actor_id,
            source: e.source,
            case_id: e.case_id,
            case_number: e.case_number,
            member_id: e.member_id,
            appeal_id: e.appeal_id,
            action_id: e.action_id,
            rule_id: e.rule_id,
            ticket_id: e.ticket_id,
            detail: e.detail,
            text: e.text,
            created_at: e.created_at,
        }
    }
}

/// Route 35's body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ModLogPage {
    entries: Vec<ModLogEntry>,
    next_cursor: Option<String>,
}

fn invalid(field: &str) -> ApiError {
    ApiError::field(
        field,
        "invalid",
        quack_text::render::plain(&quack_text::api::value_invalid()),
    )
}

/// The parsed query: the filter, its canonical form, the case number
/// (resolved to a case later), the limit, and the cursor.
struct Parsed {
    filter: ModLogFilter,
    case_number: Option<u64>,
    limit: Option<u32>,
    cursor: Option<String>,
}

fn parse(query: Option<&str>) -> ApiResult<Parsed> {
    let mut p = Parsed {
        filter: ModLogFilter::default(),
        case_number: None,
        limit: None,
        cursor: None,
    };
    for (key, value) in url::form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
        let v = value.as_ref();
        let f = &mut p.filter;
        match key.as_ref() {
            "kind" => f.kinds.push(v.parse().map_err(|_| invalid("kind"))?),
            "result" => f.result = Some(v.parse().map_err(|_| invalid("result"))?),
            "actor_id" => f.actor = Some(snowflake(v, "actor_id")?),
            "member_id" => f.member = Some(snowflake(v, "member_id")?),
            "case_number" => p.case_number = Some(v.parse().map_err(|_| invalid("case_number"))?),
            "rule_id" => f.rule = Some(uuid_id(v).map_err(|_| invalid("rule_id"))?),
            "appeal_id" => f.appeal = Some(uuid_id(v).map_err(|_| invalid("appeal_id"))?),
            "created_after" => f.created_after = Some(parse_ts(v).ok_or_else(|| invalid(&key))?),
            "created_before" => f.created_before = Some(parse_ts(v).ok_or_else(|| invalid(&key))?),
            "limit" => p.limit = Some(v.parse().map_err(|_| invalid("limit"))?),
            "cursor" => p.cursor = Some(v.to_owned()),
            _ => {}
        }
    }
    p.filter.kinds.sort();
    p.filter.kinds.dedup();
    Ok(p)
}

/// The server's moderation log, newest first.
#[utoipa::path(get, path = "/api/guilds/{guild_id}/mod-log", tag = "mod-log",
    params(("guild_id" = String, Path), ModLogQuery),
    responses((status = 200, body = ModLogPage), (status = 400, body = ErrorEnvelope),
              (status = 403, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn mod_log(
    State(state): State<ApiState>,
    staff: Staff<Moderate>,
    RawQuery(query): RawQuery,
) -> ApiResult<Json<ModLogPage>> {
    let mut p = parse(query.as_deref())?;
    let limit = pagination::limit(p.limit)?;
    let filters = format!("{:?}|{:?}", p.filter, p.case_number);
    let cursor = pagination::decode_time(p.cursor.as_deref(), &filters)?;
    let guild = staff.guild();
    if let Some(number) = p.case_number {
        let mut conn = state.engine().db().conn().await?;
        match quack_db::cases::get_by_number(&mut conn, guild, number).await? {
            Some(case) => p.filter.case = Some(case.id),
            None => {
                return Ok(Json(ModLogPage {
                    entries: Vec::new(),
                    next_cursor: None,
                }));
            }
        }
    }
    let page = modlog::list(state.engine(), &staff.principal, &p.filter, cursor, limit)
        .await?
        .map_err(|e| Facts::scene(Scene::Dashboard).domain(e))?;
    let next_cursor = page
        .next_cursor
        .map(|c| pagination::time_cursor(c.created_at, c.id, &filters));
    Ok(Json(ModLogPage {
        entries: page
            .entries
            .into_iter()
            .map(|e| ModLogEntry::new(guild, e))
            .collect(),
        next_cursor,
    }))
}
