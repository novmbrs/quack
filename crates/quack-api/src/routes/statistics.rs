//! Server statistics (route 8): counts by bucket over `[from, to)`.

use axum::Json;
use axum::extract::{Query, State};
use quack_engine::stats::{compute, range};
use quack_text::error::Scene;
use quack_text::fmt::upper_first;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::error::{ApiError, ApiResult, ErrorEnvelope, Facts, map};
use crate::extract::{Moderate, Staff};
use crate::state::ApiState;
use crate::wire::{parse_ts, ts};

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new().routes(routes!(statistics))
}

/// Route 8's query.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct StatsQuery {
    /// Start (RFC 3339, inclusive); default one calendar month before `to`.
    from: Option<String>,
    /// End (RFC 3339, exclusive); default now. 1 s to 366 days after `from`.
    to: Option<String>,
}

/// One count.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct Bucket {
    /// The bucket's stable key (a date, rule ID, or enum value).
    key: String,
    /// Its name for people.
    label: String,
    /// How many.
    count: u64,
}

/// Route 8's body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct Statistics {
    #[serde(with = "ts")]
    #[schema(value_type = String, format = DateTime)]
    from: OffsetDateTime,
    #[serde(with = "ts")]
    #[schema(value_type = String, format = DateTime)]
    to: OffsetDateTime,
    cases_total: u64,
    cases_by_day: Vec<Bucket>,
    cases_by_rule: Vec<Bucket>,
    cases_by_state: Vec<Bucket>,
    cases_by_source: Vec<Bucket>,
    actions_by_kind: Vec<Bucket>,
    actions_by_state: Vec<Bucket>,
    appeals_by_state: Vec<Bucket>,
    mod_log_by_kind: Vec<Bucket>,
}

/// `some_value` → "Some value".
fn humanize(key: &str) -> String {
    upper_first(&key.replace('_', " "))
}

fn buckets<T>(rows: Vec<(T, u64)>, key: impl Fn(&T) -> String) -> Vec<Bucket> {
    rows.into_iter()
        .map(|(value, count)| {
            let key = key(&value);
            Bucket {
                label: humanize(&key),
                key,
                count,
            }
        })
        .collect()
}

fn time_param(value: Option<&str>, field: &str) -> ApiResult<Option<OffsetDateTime>> {
    value
        .map(|v| {
            parse_ts(v).ok_or_else(|| {
                ApiError::field(
                    field,
                    "invalid_time",
                    quack_text::render::plain(&quack_text::api::validation_failed()),
                )
            })
        })
        .transpose()
}

/// The server's statistics.
#[utoipa::path(get, path = "/api/guilds/{guild_id}/statistics", tag = "statistics",
    params(("guild_id" = String, Path, description = "The server"), StatsQuery),
    responses((status = 200, body = Statistics), (status = 400, body = ErrorEnvelope),
              (status = 403, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn statistics(
    State(state): State<ApiState>,
    staff: Staff<Moderate>,
    Query(query): Query<StatsQuery>,
) -> ApiResult<Json<Statistics>> {
    let from = time_param(query.from.as_deref(), "from")?;
    let to = time_param(query.to.as_deref(), "to")?;
    let facts = Facts::scene(Scene::Staff);
    let (from, to) = range(from, to).map_err(|e| map::stats(e, &facts))?;
    let s = compute(state.engine(), &staff.principal, Some(from), Some(to))
        .await
        .map_err(|e| map::stats(e, &facts))?;
    Ok(Json(Statistics {
        from,
        to,
        cases_total: s.cases_total,
        cases_by_day: s
            .cases_by_day
            .into_iter()
            .map(|(day, count)| Bucket {
                key: day.to_string(),
                label: day.to_string(),
                count,
            })
            .collect(),
        cases_by_rule: s
            .cases_by_rule
            .into_iter()
            .map(|(id, name, count)| Bucket {
                key: id.map_or_else(|| "imported".to_owned(), |id| id.get().to_string()),
                label: name.unwrap_or_else(|| "Imported".to_owned()),
                count,
            })
            .collect(),
        cases_by_state: buckets(s.cases_by_voided, |voided| {
            if *voided { "voided" } else { "valid" }.to_owned()
        }),
        cases_by_source: buckets(s.cases_by_source, |v| v.as_str().to_owned()),
        actions_by_kind: s
            .actions_by_kind
            .into_iter()
            .map(|(kind, count)| Bucket {
                key: kind.as_str().to_owned(),
                label: upper_first(quack_text::word::action(kind)),
                count,
            })
            .collect(),
        actions_by_state: buckets(s.actions_by_state, |v| v.as_str().to_owned()),
        appeals_by_state: buckets(s.appeals_by_state, |v| v.as_str().to_owned()),
        mod_log_by_kind: buckets(s.mod_log_by_kind, |v| v.as_str().to_owned()),
    }))
}
