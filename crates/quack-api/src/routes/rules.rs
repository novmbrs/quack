//! Rules (routes 12–19, 59): list, read, create, edit, archive, restore,
//! export, import, and one version. Moderators and rules managers read;
//! rules managers write.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use quack_core::ids::{AccountId, RuleId};
use quack_core::rule::{RuleDefinition, RuleFile};
use quack_db::rules::{Rule, VersionInfo};
use quack_engine::rules::{self, RuleError, RuleList};
use quack_text::error::Scene;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::error::{ApiError, ApiResult, ErrorEnvelope, Facts, map};
use crate::extract::{Body, IdempotencyKey, ManageRules, ReadRules, Staff, uuid_id};
use crate::state::ApiState;
use crate::wire::{schema, ts};

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(list_rules, create_rule))
        .routes(routes!(get_rule, edit_rule))
        .routes(routes!(archive_rule))
        .routes(routes!(restore_rule))
        .routes(routes!(export_rule))
        .routes(routes!(import_rule))
        .routes(routes!(rule_version))
}

/// Route 12's query.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct ListQuery {
    /// Archived rules instead of active ones.
    #[serde(default)]
    archived: bool,
}

/// A rule in a list.
#[derive(Debug, Serialize, ToSchema)]
#[schema(as = RuleSummary)]
pub(crate) struct RuleSummaryWire {
    #[schema(value_type = uuid::Uuid)]
    id: RuleId,
    name: String,
    description: String,
    current_version: u32,
    #[serde(with = "ts::option")]
    #[schema(value_type = Option<String>, format = DateTime)]
    archived_at: Option<OffsetDateTime>,
    /// The levels in words.
    levels_text: String,
    field_count: usize,
    is_starter: bool,
    used_by_honeypot: bool,
    /// Members may appeal its cases.
    appealable: bool,
    /// Only cases from the last this many days count (null: all).
    counting_window_days: Option<u16>,
    /// Its levels, ascending by `from_case_count`.
    #[schema(value_type = Vec<schema::Level>)]
    levels: Vec<quack_core::rule::Level>,
}

impl RuleSummaryWire {
    fn new(r: Rule, starter: Option<RuleId>, honeypot_rule: Option<RuleId>) -> Self {
        let levels: Vec<_> = r
            .definition
            .levels
            .iter()
            .map(|l| (l.from_case_count, l.action))
            .collect();
        Self {
            id: r.id,
            levels_text: quack_text::word::levels_summary(&levels),
            field_count: r.definition.fields.len(),
            is_starter: starter == Some(r.id),
            used_by_honeypot: honeypot_rule == Some(r.id),
            appealable: r.definition.appealable,
            counting_window_days: r.definition.counting_window_days,
            levels: r.definition.levels,
            name: r.definition.name,
            description: r.definition.description,
            current_version: r.current_version,
            archived_at: r.archived_at,
        }
    }
}

/// Route 12's body.
#[derive(Debug, Serialize, ToSchema)]
#[schema(as = RuleList)]
pub(crate) struct RuleListWire {
    rules: Vec<RuleSummaryWire>,
}

/// One version in a rule's history.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct VersionEntry {
    version: u32,
    #[serde(with = "ts")]
    #[schema(value_type = String, format = DateTime)]
    created_at: OffsetDateTime,
    #[schema(value_type = Option<schema::Snowflake>)]
    created_by: Option<AccountId>,
    /// Rebuilt by the importer from old cases.
    reconstructed: bool,
}

/// A rule with its current definition and history.
#[derive(Debug, Serialize, ToSchema)]
#[schema(as = Rule)]
pub(crate) struct RuleWire {
    #[schema(value_type = uuid::Uuid)]
    id: RuleId,
    current_version: u32,
    #[serde(with = "ts::option")]
    #[schema(value_type = Option<String>, format = DateTime)]
    archived_at: Option<OffsetDateTime>,
    #[serde(with = "ts")]
    #[schema(value_type = String, format = DateTime)]
    created_at: OffsetDateTime,
    #[schema(value_type = Option<schema::Snowflake>)]
    created_by: Option<AccountId>,
    #[serde(with = "ts")]
    #[schema(value_type = String, format = DateTime)]
    updated_at: OffsetDateTime,
    #[schema(value_type = Option<schema::Snowflake>)]
    updated_by: Option<AccountId>,
    #[schema(value_type = schema::RuleDefinition)]
    definition: RuleDefinition,
    /// Every version, oldest first.
    versions: Vec<VersionEntry>,
}

impl From<(Rule, Vec<VersionInfo>)> for RuleWire {
    fn from((r, versions): (Rule, Vec<VersionInfo>)) -> Self {
        Self {
            id: r.id,
            current_version: r.current_version,
            archived_at: r.archived_at,
            created_at: r.created_at,
            created_by: r.created_by,
            updated_at: r.updated_at,
            updated_by: r.updated_by,
            definition: r.definition,
            versions: versions
                .into_iter()
                .map(|v| VersionEntry {
                    version: v.version,
                    created_at: v.created_at,
                    created_by: v.created_by,
                    reconstructed: v.reconstructed,
                })
                .collect(),
        }
    }
}

/// One version of a rule (route 59).
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct RuleVersion {
    #[schema(value_type = uuid::Uuid)]
    rule_id: RuleId,
    version: u32,
    #[serde(with = "ts")]
    #[schema(value_type = String, format = DateTime)]
    created_at: OffsetDateTime,
    #[schema(value_type = Option<schema::Snowflake>)]
    created_by: Option<AccountId>,
    reconstructed: bool,
    #[schema(value_type = Option<schema::RuleDefinition>)]
    definition: Option<RuleDefinition>,
}

impl From<quack_db::rules::RuleVersion> for RuleVersion {
    fn from(v: quack_db::rules::RuleVersion) -> Self {
        Self {
            rule_id: v.rule,
            version: v.version,
            created_at: v.created_at,
            created_by: v.created_by,
            reconstructed: v.reconstructed,
            definition: Some(v.definition),
        }
    }
}

/// Route 15's body.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct EditRule {
    /// The version the editor loaded; another is `409 rule_changed`.
    expected_version: u32,
    #[schema(value_type = schema::RuleDefinition)]
    definition: RuleDefinition,
}

/// Route 19's body.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImportRule {
    /// Must be `true`: the reader confirmed the import.
    confirm: bool,
    #[schema(value_type = schema::RuleFile)]
    file: RuleFile,
    /// A different name for the imported rule.
    name: Option<String>,
}

fn facts(rule: Option<&str>) -> Facts {
    Facts {
        rule: rule.map(str::to_owned),
        ..Facts::scene(Scene::EditingRule)
    }
}

/// The rule as written, read back for the response.
async fn written(
    state: &ApiState,
    staff: &Staff<impl Send + Sync>,
    result: Result<rules::RuleWritten, RuleError>,
    facts: &Facts,
    prefix: &str,
) -> ApiResult<Json<RuleWire>> {
    let written = result.map_err(|e| map::rule(e, facts, prefix))?;
    let view = rules::get(state.engine(), &staff.principal, written.rule)
        .await
        .map_err(|e| map::rule(e, facts, prefix))?;
    Ok(Json(view.into()))
}

/// The server's rules, by name.
#[utoipa::path(get, path = "/api/guilds/{guild_id}/rules", tag = "rules",
    params(("guild_id" = String, Path), ListQuery),
    responses((status = 200, body = RuleListWire), (status = 403, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn list_rules(
    State(state): State<ApiState>,
    staff: Staff<ReadRules>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<RuleListWire>> {
    let RuleList {
        rules,
        starter,
        honeypot_rule,
    } = rules::list(state.engine(), &staff.principal, q.archived)
        .await
        .map_err(|e| map::rule(e, &facts(None), ""))?;
    Ok(Json(RuleListWire {
        rules: rules
            .into_iter()
            .map(|r| RuleSummaryWire::new(r, starter, honeypot_rule))
            .collect(),
    }))
}

/// Creates a rule.
#[utoipa::path(post, path = "/api/guilds/{guild_id}/rules", tag = "rules",
    params(("guild_id" = String, Path)), request_body = schema::RuleDefinition,
    responses((status = 201, body = RuleWire), (status = 400, body = ErrorEnvelope),
              (status = 409, body = ErrorEnvelope)),
    security(("session" = [], "csrf" = [])))]
async fn create_rule(
    State(state): State<ApiState>,
    staff: Staff<ManageRules>,
    key: IdempotencyKey,
    Body(definition): Body<RuleDefinition>,
) -> ApiResult<(StatusCode, Json<RuleWire>)> {
    let facts = facts(Some(definition.name.trim()));
    let result = rules::create(state.engine(), &staff.principal, definition, &key.0).await;
    let rule = written(&state, &staff, result, &facts, "").await?;
    Ok((StatusCode::CREATED, rule))
}

/// A rule with its history.
#[utoipa::path(get, path = "/api/guilds/{guild_id}/rules/{rule_id}", tag = "rules",
    params(("guild_id" = String, Path), ("rule_id" = String, Path)),
    responses((status = 200, body = RuleWire), (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn get_rule(
    State(state): State<ApiState>,
    staff: Staff<ReadRules>,
    Path(path): Path<(String, String)>,
) -> ApiResult<Json<RuleWire>> {
    let rule: RuleId = uuid_id(&path.1)?;
    let view = rules::get(state.engine(), &staff.principal, rule)
        .await
        .map_err(|e| map::rule(e, &facts(None), ""))?;
    Ok(Json(view.into()))
}

/// Saves a new version of a rule.
#[utoipa::path(put, path = "/api/guilds/{guild_id}/rules/{rule_id}", tag = "rules",
    params(("guild_id" = String, Path), ("rule_id" = String, Path)), request_body = EditRule,
    responses((status = 200, body = RuleWire), (status = 409, body = ErrorEnvelope)),
    security(("session" = [], "csrf" = [])))]
async fn edit_rule(
    State(state): State<ApiState>,
    staff: Staff<ManageRules>,
    key: IdempotencyKey,
    Path(path): Path<(String, String)>,
    Body(body): Body<EditRule>,
) -> ApiResult<Json<RuleWire>> {
    let rule: RuleId = uuid_id(&path.1)?;
    let facts = facts(Some(body.definition.name.trim()));
    let result = rules::edit(
        state.engine(),
        &staff.principal,
        rule,
        body.expected_version,
        body.definition,
        &key.0,
    )
    .await;
    written(&state, &staff, result, &facts, "definition.").await
}

/// Archives a rule: it stays, but can't be applied.
#[utoipa::path(post, path = "/api/guilds/{guild_id}/rules/{rule_id}/archive", tag = "rules",
    params(("guild_id" = String, Path), ("rule_id" = String, Path)),
    responses((status = 200, body = RuleWire), (status = 409, body = ErrorEnvelope)),
    security(("session" = [], "csrf" = [])))]
async fn archive_rule(
    State(state): State<ApiState>,
    staff: Staff<ManageRules>,
    key: IdempotencyKey,
    Path(path): Path<(String, String)>,
) -> ApiResult<Json<RuleWire>> {
    let rule: RuleId = uuid_id(&path.1)?;
    let result = rules::archive(state.engine(), &staff.principal, rule, &key.0).await;
    written(&state, &staff, result, &facts(None), "").await
}

/// Restores an archived rule.
#[utoipa::path(post, path = "/api/guilds/{guild_id}/rules/{rule_id}/restore", tag = "rules",
    params(("guild_id" = String, Path), ("rule_id" = String, Path)),
    responses((status = 200, body = RuleWire), (status = 409, body = ErrorEnvelope)),
    security(("session" = [], "csrf" = [])))]
async fn restore_rule(
    State(state): State<ApiState>,
    staff: Staff<ManageRules>,
    key: IdempotencyKey,
    Path(path): Path<(String, String)>,
) -> ApiResult<Json<RuleWire>> {
    let rule: RuleId = uuid_id(&path.1)?;
    let result = rules::restore(state.engine(), &staff.principal, rule, &key.0).await;
    written(&state, &staff, result, &facts(None), "").await
}

/// The rule's export file.
#[utoipa::path(get, path = "/api/guilds/{guild_id}/rules/{rule_id}/export", tag = "rules",
    params(("guild_id" = String, Path), ("rule_id" = String, Path)),
    responses((status = 200, body = schema::RuleFile), (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn export_rule(
    State(state): State<ApiState>,
    staff: Staff<ManageRules>,
    Path(path): Path<(String, String)>,
) -> ApiResult<Json<RuleFile>> {
    let rule: RuleId = uuid_id(&path.1)?;
    rules::export(state.engine(), &staff.principal, rule)
        .await
        .map(Json)
        .map_err(|e| map::rule(e, &facts(None), ""))
}

/// Imports a rule file as a new rule.
#[utoipa::path(post, path = "/api/guilds/{guild_id}/rules/import", tag = "rules",
    params(("guild_id" = String, Path)), request_body = ImportRule,
    responses((status = 201, body = RuleWire), (status = 400, body = ErrorEnvelope),
              (status = 409, body = ErrorEnvelope)),
    security(("session" = [], "csrf" = [])))]
async fn import_rule(
    State(state): State<ApiState>,
    staff: Staff<ManageRules>,
    key: IdempotencyKey,
    Body(body): Body<ImportRule>,
) -> ApiResult<(StatusCode, Json<RuleWire>)> {
    if !body.confirm {
        return Err(ApiError::field(
            "confirm",
            "required",
            quack_text::render::plain(&quack_text::api::validation_failed()),
        ));
    }
    let name = body
        .name
        .clone()
        .unwrap_or_else(|| body.file.rule.name.clone());
    let facts = facts(Some(name.trim()));
    let result = rules::import(
        state.engine(),
        &staff.principal,
        body.file,
        body.name,
        &key.0,
    )
    .await;
    let rule = written(&state, &staff, result, &facts, "file.rule.").await?;
    Ok((StatusCode::CREATED, rule))
}

/// One version of a rule, with its definition.
#[utoipa::path(get, path = "/api/guilds/{guild_id}/rules/{rule_id}/versions/{version}", tag = "rules",
    params(("guild_id" = String, Path), ("rule_id" = String, Path), ("version" = u32, Path)),
    responses((status = 200, body = RuleVersion), (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn rule_version(
    State(state): State<ApiState>,
    staff: Staff<ReadRules>,
    Path(path): Path<(String, String, String)>,
) -> ApiResult<Json<RuleVersion>> {
    let rule: RuleId = uuid_id(&path.1)?;
    let version: u32 = path.2.parse().map_err(|_| ApiError::not_found())?;
    let view = rules::version(state.engine(), &staff.principal, rule, version)
        .await
        .map_err(|e| map::rule(e, &facts(None), ""))?;
    Ok(Json(view.into()))
}
