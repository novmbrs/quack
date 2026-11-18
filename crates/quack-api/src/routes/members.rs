//! A member's history in the server (route 27).

use axum::Json;
use axum::extract::{Path, State};
use quack_core::ids::{MemberId, RuleId};
use quack_engine::cases;
use quack_text::error::Scene;
use serde::Serialize;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::error::{ApiResult, ErrorEnvelope, Facts, map};
use crate::extract::{Moderate, Staff, snowflake};
use crate::state::ApiState;
use crate::wire::schema;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new().routes(routes!(member_summary))
}

/// A member's cases under one rule.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct RuleCount {
    /// Null for imported cases without a rule.
    #[schema(value_type = Option<uuid::Uuid>)]
    rule_id: Option<RuleId>,
    rule_name: String,
    valid: u64,
    voided: u64,
}

/// A member's cases in the server, counted.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct MemberSummary {
    #[schema(value_type = schema::Snowflake)]
    member_id: MemberId,
    total: u64,
    valid: u64,
    voided: u64,
    by_rule: Vec<RuleCount>,
}

/// A member's cases, counted per rule.
#[utoipa::path(get, path = "/api/guilds/{guild_id}/members/{member_id}/summary", tag = "members",
    params(("guild_id" = String, Path), ("member_id" = String, Path)),
    responses((status = 200, body = MemberSummary), (status = 400, body = ErrorEnvelope),
              (status = 403, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn member_summary(
    State(state): State<ApiState>,
    staff: Staff<Moderate>,
    Path(path): Path<(String, String)>,
) -> ApiResult<Json<MemberSummary>> {
    let member: MemberId = snowflake(&path.1, "member_id")?;
    let tallies = cases::member_summary(state.engine(), &staff.principal, member)
        .await
        .map_err(|e| map::case(e, &Facts::scene(Scene::Dashboard), ""))?;
    let valid = tallies.iter().map(|t| t.valid).sum();
    let voided = tallies.iter().map(|t| t.voided).sum();
    Ok(Json(MemberSummary {
        member_id: member,
        total: valid + voided,
        valid,
        voided,
        by_rule: tallies
            .into_iter()
            .map(|t| RuleCount {
                rule_id: t.rule,
                rule_name: t
                    .rule_name
                    .unwrap_or_else(|| quack_text::word::IMPORTED.to_owned()),
                valid: t.valid,
                voided: t.voided,
            })
            .collect(),
    }))
}
