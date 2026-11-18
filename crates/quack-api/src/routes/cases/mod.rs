//! Cases (routes 20–25, 58): list, preview, add, read, void, edit
//! context, and add evidence. Every case is found inside the path's
//! server, so another server's case is `404 not_found`.

mod context;
mod types;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use quack_core::DomainError;
use quack_core::ids::{AccountId, CaseId, MemberId, RuleId};
use quack_db::cases::CaseFilter;
use quack_engine::CaseRef;
use quack_engine::cases::{self, Actor, ApplyRule};
use quack_text::error::Scene;
use serde::Deserialize;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use types::{Case, CasePreview};
pub(crate) use types::{CaseSummary, TimelineText};

use crate::error::{ApiError, ApiResult, ErrorEnvelope, Facts, map};
use crate::extract::{Body, IdempotencyKey, Moderate, Staff, case_ref, snowflake, uuid_id};
use crate::pagination;
use crate::state::ApiState;
use crate::wire::{parse_ts, schema};
use context::ContextBody;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(list_cases, add_case))
        .routes(routes!(preview))
        .routes(routes!(get_case))
        .routes(routes!(void_case))
        .routes(routes!(edit_context))
        .routes(routes!(add_evidence))
}

fn invalid(field: &str) -> ApiError {
    ApiError::field(
        field,
        "invalid",
        quack_text::render::plain(&quack_text::api::validation_failed()),
    )
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

fn staff_facts() -> Facts {
    Facts::scene(Scene::Staff)
}

/// The server's cases, newest first.
#[utoipa::path(get, path = "/api/guilds/{guild_id}/cases", tag = "cases",
    params(("guild_id" = String, Path), CaseQuery),
    responses((status = 200, body = CasePage), (status = 400, body = ErrorEnvelope),
              (status = 403, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn list_cases(
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
    let next_cursor = pagination::next(&rows, limit, |c| {
        pagination::number_cursor(c.number, &filters)
    });
    Ok(Json(CasePage {
        cases: rows.into_iter().map(Into::into).collect(),
        next_cursor,
    }))
}

/// Route 21's body.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct AddCase {
    /// The member the case is about.
    #[schema(value_type = schema::Snowflake)]
    member_id: String,
    #[schema(value_type = uuid::Uuid)]
    rule_id: String,
    /// The version the preview showed; another is `409 rule_changed`.
    rule_version: Option<u32>,
    /// Field key → answer. Numbers may be JSON numbers or strings.
    #[serde(default)]
    #[schema(value_type = std::collections::HashMap<String, schema::ContextInput>)]
    context: ContextBody,
    /// Message links to save as evidence.
    #[serde(default)]
    evidence_links: Vec<String>,
    /// The voided case this one replaces.
    replaces_case_number: Option<u64>,
}

/// The rule's name and fields, for error messages.
async fn rule_facts(
    state: &ApiState,
    staff: &Staff<Moderate>,
    rule: RuleId,
    scene: Scene,
) -> Facts {
    let mut facts = Facts::scene(scene);
    if let Ok(mut conn) = state.engine().db().conn().await
        && let Ok(Some(found)) = quack_db::rules::get(&mut conn, staff.guild(), rule).await
    {
        facts.rule = Some(found.definition.name.clone());
        facts.fields = found.definition.context_fields().into_owned();
    }
    facts
}

/// Adds a case: applies the rule's level for the member now.
#[utoipa::path(post, path = "/api/guilds/{guild_id}/cases", tag = "cases",
    params(("guild_id" = String, Path)), request_body = AddCase,
    responses((status = 201, body = Case), (status = 400, body = ErrorEnvelope),
              (status = 409, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope)),
    security(("session" = [], "csrf" = [])))]
async fn add_case(
    State(state): State<ApiState>,
    staff: Staff<Moderate>,
    key: IdempotencyKey,
    Body(body): Body<AddCase>,
) -> ApiResult<(StatusCode, Json<Case>)> {
    let member: MemberId = snowflake(&body.member_id, "member_id")?;
    let rule: RuleId = uuid_id(&body.rule_id).map_err(|_| invalid("rule_id"))?;
    let context = body.context.into_inputs()?;
    let replaces = match body.replaces_case_number {
        None => None,
        Some(number) => {
            let mut conn = state.engine().db().conn().await?;
            let found = quack_db::cases::get_by_number(&mut conn, staff.guild(), number).await?;
            let Some(found) = found else {
                let facts = Facts {
                    case_number: Some(number),
                    ..Facts::scene(Scene::AddingCase)
                };
                return Err(facts.domain(DomainError::ReplacementInvalid));
            };
            Some(found.id)
        }
    };
    let request = ApplyRule {
        guild: staff.guild(),
        actor: Actor::Staff(staff.principal.clone()),
        member,
        rule,
        pinned_version: body.rule_version,
        context,
        evidence_links: body.evidence_links,
        source_message: None,
        receipt: None,
        replaces,
        op_key: key.0,
        draft: None,
        honeypot: None,
    };
    let applied = match cases::apply_rule(state.engine(), request).await {
        Ok(applied) => applied,
        Err(e) => {
            let mut facts = rule_facts(&state, &staff, rule, Scene::AddingCase).await;
            facts.case_number = body.replaces_case_number;
            return Err(map::apply(e, &facts));
        }
    };
    let view = read(&state, &staff, CaseRef::Id(applied.case_id())).await?;
    Ok((StatusCode::CREATED, Json(view.into())))
}

async fn read(
    state: &ApiState,
    staff: &Staff<Moderate>,
    case: CaseRef,
) -> ApiResult<quack_engine::cases::CaseView> {
    let number = match case {
        CaseRef::Number(n) => Some(n),
        CaseRef::Id(_) => None,
    };
    cases::get(state.engine(), &staff.principal, case)
        .await
        .map_err(|e| {
            let facts = Facts {
                case_number: number,
                ..staff_facts()
            };
            map::case(e, &facts, "")
        })
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
async fn preview(
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
async fn get_case(
    State(state): State<ApiState>,
    staff: Staff<Moderate>,
    Path(path): Path<(String, String)>,
) -> ApiResult<Json<Case>> {
    let view = read(&state, &staff, case_ref(&path.1)?).await?;
    Ok(Json(view.into()))
}

/// Route 23's body.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct VoidCase {
    /// Why (1–1,000 characters).
    reason: String,
}

/// The case's number, for messages that name it.
async fn number_of(state: &ApiState, staff: &Staff<Moderate>, case: CaseRef) -> Option<u64> {
    match case {
        CaseRef::Number(n) => Some(n),
        CaseRef::Id(id) => {
            let mut conn = state.engine().db().conn().await.ok()?;
            quack_db::cases::get(&mut conn, staff.guild(), id)
                .await
                .ok()
                .flatten()
                .map(|c| c.number)
        }
    }
}

/// Voids a case: it stops counting and its timeout or ban is undone.
#[utoipa::path(post, path = "/api/guilds/{guild_id}/cases/{case_ref}/void", tag = "cases",
    params(("guild_id" = String, Path), ("case_ref" = String, Path)), request_body = VoidCase,
    responses((status = 200, body = Case), (status = 404, body = ErrorEnvelope),
              (status = 409, body = ErrorEnvelope)),
    security(("session" = [], "csrf" = [])))]
async fn void_case(
    State(state): State<ApiState>,
    staff: Staff<Moderate>,
    key: IdempotencyKey,
    Path(path): Path<(String, String)>,
    Body(body): Body<VoidCase>,
) -> ApiResult<Json<Case>> {
    let case = case_ref(&path.1)?;
    let voided = match cases::void_case(
        state.engine(),
        &staff.principal,
        case,
        &body.reason,
        &key.0,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            let facts = Facts {
                case_number: number_of(&state, &staff, case).await,
                ..staff_facts()
            };
            return Err(map::case(e, &facts, &body.reason));
        }
    };
    let view = read(&state, &staff, CaseRef::Id(voided.case)).await?;
    Ok(Json(view.into()))
}

/// Route 24's body.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct EditContext {
    #[schema(value_type = std::collections::HashMap<String, schema::ContextInput>)]
    context: ContextBody,
    /// The revision the editor loaded; another is `409 context_changed`.
    expected_context_revision: u32,
}

/// Replaces a case's context.
#[utoipa::path(put, path = "/api/guilds/{guild_id}/cases/{case_ref}/context", tag = "cases",
    params(("guild_id" = String, Path), ("case_ref" = String, Path)), request_body = EditContext,
    responses((status = 200, body = Case), (status = 400, body = ErrorEnvelope),
              (status = 409, body = ErrorEnvelope)),
    security(("session" = [], "csrf" = [])))]
async fn edit_context(
    State(state): State<ApiState>,
    staff: Staff<Moderate>,
    key: IdempotencyKey,
    Path(path): Path<(String, String)>,
    Body(body): Body<EditContext>,
) -> ApiResult<Json<Case>> {
    let case = case_ref(&path.1)?;
    let inputs = body.context.into_inputs()?;
    let edited = cases::edit_context(
        state.engine(),
        &staff.principal,
        case,
        &inputs,
        body.expected_context_revision,
        &key.0,
    )
    .await;
    let edited = match edited {
        Ok(e) => e,
        Err(e) => {
            let view = read(&state, &staff, case).await.ok();
            let facts = Facts {
                case_number: view.as_ref().map(|v| v.number),
                fields: view.map_or_else(Vec::new, |v| v.fields),
                ..staff_facts()
            };
            return Err(map::case(e, &facts, ""));
        }
    };
    let view = read(&state, &staff, CaseRef::Id(edited.case)).await?;
    Ok(Json(view.into()))
}

/// Route 25's body.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct AddEvidence {
    /// 1–10 message links in this server.
    links: Vec<String>,
}

/// Saves more messages as a case's evidence.
#[utoipa::path(post, path = "/api/guilds/{guild_id}/cases/{case_ref}/evidence", tag = "cases",
    params(("guild_id" = String, Path), ("case_ref" = String, Path)), request_body = AddEvidence,
    responses((status = 200, body = Case), (status = 400, body = ErrorEnvelope),
              (status = 422, body = ErrorEnvelope)),
    security(("session" = [], "csrf" = [])))]
async fn add_evidence(
    State(state): State<ApiState>,
    staff: Staff<Moderate>,
    key: IdempotencyKey,
    Path(path): Path<(String, String)>,
    Body(body): Body<AddEvidence>,
) -> ApiResult<Json<Case>> {
    let case = case_ref(&path.1)?;
    if body.links.is_empty() {
        return Err(invalid("links"));
    }
    let added =
        cases::add_evidence(state.engine(), &staff.principal, case, &body.links, &key.0).await;
    let added = match added {
        Ok(a) => a,
        Err(e) => {
            let facts = Facts {
                case_number: number_of(&state, &staff, case).await,
                ..staff_facts()
            };
            return Err(map::case(e, &facts, ""));
        }
    };
    let id: CaseId = added.case;
    let view = read(&state, &staff, CaseRef::Id(id)).await?;
    Ok(Json(view.into()))
}
