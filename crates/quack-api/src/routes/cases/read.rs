//! Reading cases (routes 20, 22, 58): the filtered list, one case, and
//! the preview of what adding a case would choose.

use axum::Json;
use axum::extract::{Path, Query, State};
use quack_core::ids::{AccountId, MemberId, RuleId};
use quack_db::cases::CaseFilter;
use quack_engine::cases;
use quack_text::error::Scene;
use serde::Deserialize;
use utoipa::{IntoParams, ToSchema};

use super::types::{Case, CasePreview, CaseSummary};
use super::{invalid, read, rule_facts};
use crate::error::{ApiResult, ErrorEnvelope, Facts, map};
use crate::extract::{Moderate, Staff, case_ref, snowflake, uuid_id};
use crate::pagination;
use crate::state::ApiState;
use crate::wire::parse_ts;

fn staff_facts() -> Facts {
    Facts::scene(Scene::Staff)
}

fn parse_enum<T: std::str::FromStr>(value: Option<&str>, field: &str) -> ApiResult<Option<T>> {
    value
        .map(|v| v.parse().map_err(|_| invalid(field)))
        .transpose()
}

/// Route 20's query.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct CaseQuery {
    /// Cases about this member.
    member_id: Option<String>,
    /// Cases by this moderator.
    moderator_id: Option<String>,
    /// Cases under this rule.
    rule_id: Option<String>,
    /// `valid` or `voided`.
    state: Option<String>,
    /// The original action's state.
    action_state: Option<String>,
    /// The appeal's state.
    appeal_state: Option<String>,
    /// The case with this number.
    number: Option<u64>,
    /// Created at or after (RFC 3339).
    created_after: Option<String>,
    /// Created before (RFC 3339).
    created_before: Option<String>,
    /// 1–100, default 50.
    limit: Option<u32>,
    /// From the previous page's `next_cursor`.
    cursor: Option<String>,
}

impl CaseQuery {
    fn filter(&self) -> ApiResult<CaseFilter> {
        let time = |v: Option<&str>, field: &str| {
            v.map(|t| parse_ts(t).ok_or_else(|| invalid(field)))
                .transpose()
        };
        Ok(CaseFilter {
            member: self
                .member_id
                .as_deref()
                .map(|v| snowflake::<MemberId>(v, "member_id"))
                .transpose()?,
            moderator: self
                .moderator_id
                .as_deref()
                .map(|v| snowflake::<AccountId>(v, "moderator_id"))
                .transpose()?,
            rule: self
                .rule_id
                .as_deref()
                .map(|v| uuid_id::<RuleId>(v).map_err(|_| invalid("rule_id")))
                .transpose()?,
            voided: match self.state.as_deref() {
                None => None,
                Some("valid") => Some(false),
                Some("voided") => Some(true),
                Some(_) => return Err(invalid("state")),
            },
            action_state: parse_enum(self.action_state.as_deref(), "action_state")?,
            appeal_state: parse_enum(self.appeal_state.as_deref(), "appeal_state")?,
            number: self.number,
            created_after: time(self.created_after.as_deref(), "created_after")?,
            created_before: time(self.created_before.as_deref(), "created_before")?,
        })
    }
}

/// Route 20's body.
#[derive(Debug, serde::Serialize, ToSchema)]
pub(crate) struct CasePage {
    cases: Vec<CaseSummary>,
    next_cursor: Option<String>,
}

/// The server's cases, newest first.
#[utoipa::path(get, path = "/api/guilds/{guild_id}/cases", tag = "cases",
    params(("guild_id" = String, Path), CaseQuery),
    responses((status = 200, body = CasePage), (status = 400, body = ErrorEnvelope),
              (status = 403, body = ErrorEnvelope)),
    security(("session" = [])))]
pub(super) async fn list_cases(
    State(state): State<ApiState>,
    staff: Staff<Moderate>,
    Query(q): Query<CaseQuery>,
) -> ApiResult<Json<CasePage>> {
    let filter = q.filter()?;
    let limit = pagination::limit(q.limit)?;
    let filters = format!("{filter:?}");
    let before = pagination::decode_number(q.cursor.as_deref(), &filters)?;
    let rows = cases::list(state.engine(), &staff.principal, &filter, before, limit)
        .await
        .map_err(|e| map::case(e, &staff_facts(), ""))?;
    let (rows, next_cursor) =
        pagination::split(rows, |c| pagination::number_cursor(c.number, &filters));
    Ok(Json(CasePage {
        cases: rows.into_iter().map(Into::into).collect(),
        next_cursor,
    }))
}

/// Route 58's query.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct PreviewQuery {
    /// The member.
    member_id: String,
    /// The rule.
    rule_id: String,
}

/// What adding a case would choose for the member now.
#[utoipa::path(get, path = "/api/guilds/{guild_id}/cases/preview", tag = "cases",
    params(("guild_id" = String, Path), PreviewQuery),
    responses((status = 200, body = CasePreview), (status = 404, body = ErrorEnvelope),
              (status = 422, body = ErrorEnvelope)),
    security(("session" = [])))]
pub(super) async fn preview(
    State(state): State<ApiState>,
    staff: Staff<Moderate>,
    Query(q): Query<PreviewQuery>,
) -> ApiResult<Json<CasePreview>> {
    let member: MemberId = snowflake(&q.member_id, "member_id")?;
    let rule: RuleId = uuid_id(&q.rule_id).map_err(|_| invalid("rule_id"))?;
    match cases::preview(state.engine(), &staff.principal, member, rule).await {
        Ok(p) => Ok(Json(p.into())),
        Err(e) => {
            let facts = rule_facts(&state, &staff, rule, Scene::AddingCase).await;
            Err(map::case(e, &facts, ""))
        }
    }
}

/// A case of the server, by number, UUID, or legacy ULID.
#[utoipa::path(get, path = "/api/guilds/{guild_id}/cases/{case_ref}", tag = "cases",
    params(("guild_id" = String, Path), ("case_ref" = String, Path)),
    responses((status = 200, body = Case), (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
pub(super) async fn get_case(
    State(state): State<ApiState>,
    staff: Staff<Moderate>,
    Path(path): Path<(String, String)>,
) -> ApiResult<Json<Case>> {
    let view = read(&state, &staff, case_ref(&path.1)?).await?;
    Ok(Json(view.into()))
}
