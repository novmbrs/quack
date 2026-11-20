//! Cases (routes 20–25, 58, 60–63): list, preview, add, read, void, edit
//! context, and evidence (add links, upload files, edit a note, show or
//! hide uploads, remove). Every case is found inside the path's server, so
//! another server's case is `404 not_found`.

mod context;
mod evidence;
mod items;
mod read;
mod types;
mod upload;
mod upload_form;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use quack_core::DomainError;
use quack_core::ids::{MemberId, RuleId};
use quack_engine::CaseRef;
use quack_engine::cases::{self, Actor, ApplyRule};
use quack_text::error::Scene;
use serde::Deserialize;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use types::{Case, CaseVoided};
pub(crate) use types::{CaseSummary, TimelineText};
pub(crate) use upload::{UPLOAD_BUDGET_BYTES, UploadBudget};

use crate::error::{ApiError, ApiResult, ErrorEnvelope, Facts, map};
use crate::extract::{Body, IdempotencyKey, Moderate, Staff, case_ref, snowflake, uuid_id};
use crate::state::ApiState;
use crate::wire::schema;
use context::ContextBody;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(read::list_cases, add_case))
        .routes(routes!(read::preview))
        .routes(routes!(read::get_case))
        .routes(routes!(void_case))
        .routes(routes!(edit_context))
        .routes(routes!(evidence::add_evidence))
        .routes(routes!(upload::upload_evidence))
        .routes(routes!(evidence::set_note))
        .routes(routes!(evidence::set_visibility))
        .routes(routes!(evidence::remove))
}

pub(super) fn invalid(field: &str) -> ApiError {
    ApiError::field(
        field,
        "invalid",
        quack_text::render::plain(&quack_text::api::value_invalid()),
    )
}

pub(super) fn staff_facts() -> Facts {
    Facts::scene(Scene::Dashboard)
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
pub(super) async fn rule_facts(
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
        evidence_files: Vec::new(),
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

pub(super) async fn read(
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

/// Route 23's body.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct VoidCase {
    /// Why (1–1,000 characters).
    reason: String,
}

/// The case's number, for messages that name it.
pub(super) async fn number_of(
    state: &ApiState,
    staff: &Staff<Moderate>,
    case: CaseRef,
) -> Option<u64> {
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
    responses((status = 200, body = CaseVoided), (status = 404, body = ErrorEnvelope),
              (status = 409, body = ErrorEnvelope)),
    security(("session" = [], "csrf" = [])))]
async fn void_case(
    State(state): State<ApiState>,
    staff: Staff<Moderate>,
    key: IdempotencyKey,
    Path(path): Path<(String, String)>,
    Body(body): Body<VoidCase>,
) -> ApiResult<Json<CaseVoided>> {
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
    let (results, appeal_closed) = match &voided.effects {
        Some(effects) => (
            cases::void_results(effects),
            effects.closed_appeal.is_some(),
        ),
        // A replay: what the first void did, read back from the case.
        None => cases::void_results_of(&view),
    };
    let member = view.names.person(voided.member.account());
    let message = quack_text::render::plain(&quack_text::case::void_done(
        voided.number,
        &voided.rule_name,
        &member,
        &results,
        appeal_closed,
    ));
    Ok(Json(CaseVoided {
        case: view.into(),
        message,
    }))
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
