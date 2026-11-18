//! Probes (`/livez`, `/readyz`, §14.9) and server health (route 7).

use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use quack_core::ids::ChannelId;
use quack_core::permissions::Permission;
use quack_engine::settings::health;
use quack_text::error::Scene;
use serde::Serialize;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::modules::ModulesState;
use crate::error::{ApiResult, ErrorEnvelope, Facts, map};
use crate::extract::{ManageSettings, Staff};
use crate::state::ApiState;
use crate::wire::schema;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(livez))
        .routes(routes!(readyz))
        .routes(routes!(guild_health))
}

/// `/livez`'s body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct Live {
    /// Always `ok`.
    status: &'static str,
}

/// One readiness check's result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Check {
    /// Passing.
    Ok,
    /// Failing.
    Failing,
}

impl From<bool> for Check {
    fn from(ok: bool) -> Self {
        if ok { Self::Ok } else { Self::Failing }
    }
}

/// Each readiness check.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct Checks {
    /// PostgreSQL answers within 1 s.
    database: Check,
    /// The applied migrations equal the binary's.
    migrations: Check,
    /// This process holds the singleton lock.
    fence: Check,
    /// The gateway session is ready.
    gateway: Check,
    /// Every job dispatcher ran a pass in the last 15 s.
    workers: Check,
    /// Not shutting down.
    shutdown: Check,
}

/// `/readyz`'s body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct Readiness {
    /// Every check passes.
    ready: bool,
    /// Each check.
    checks: Checks,
}

/// 200 while the process serves HTTP; never checks dependencies.
#[utoipa::path(get, path = "/livez", tag = "health", responses((status = 200, body = Live)))]
async fn livez() -> Json<Live> {
    Json(Live { status: "ok" })
}

/// 200 when every check passes, else 503.
#[utoipa::path(get, path = "/readyz", tag = "health",
    responses((status = 200, body = Readiness), (status = 503, body = Readiness)))]
async fn readyz(State(state): State<ApiState>) -> (StatusCode, Json<Readiness>) {
    // Acquiring a pooled connection pings it first.
    let database = tokio::time::timeout(Duration::from_secs(1), state.engine().db().conn())
        .await
        .is_ok_and(|conn| conn.is_ok());
    let facts = state.readiness();
    let checks = Checks {
        database: database.into(),
        migrations: facts.migrations_current().into(),
        fence: facts.fence_held().into(),
        gateway: facts.gateway_ready().into(),
        workers: facts.workers_fresh().into(),
        shutdown: (!facts.shutting_down()).into(),
    };
    let ready = [
        checks.database,
        checks.migrations,
        checks.fence,
        checks.gateway,
        checks.workers,
        checks.shutdown,
    ]
    .iter()
    .all(|c| *c == Check::Ok);
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(Readiness { ready, checks }))
}

/// One configured staff channel's problems.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ChannelHealth {
    /// The setting's API name: `log_channel_id`, `appeal_channel_id`,
    /// `evidence_channel_id`, or a module's (`tickets.queue_channel_id`,
    /// `tickets.entry_channel_id`, `activity_log.routes`,
    /// `honeypot.channel_id`).
    setting: String,
    /// Whether it must be staff-only; the ticket entry and honeypot
    /// channels are for members, so only Quack's access is checked.
    staff_only: bool,
    #[schema(value_type = schema::Snowflake)]
    channel_id: ChannelId,
    /// Each problem's code and how to fix it.
    problems: Vec<ChannelProblem>,
}

/// One reason a channel isn't a usable staff channel.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ChannelProblem {
    /// `not_text`, `everyone_can_view`, `role_can_view`,
    /// `member_can_view`, `quack_missing`, `inaccessible`, or `missing`.
    code: String,
    /// What to fix.
    message: String,
}

/// What a server is missing (route 7).
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct Health {
    /// Quack is in the server.
    quack_in_server: bool,
    /// Server permissions Quack needs and lacks.
    #[schema(value_type = Vec<schema::Permission>)]
    missing_permissions: Vec<Permission>,
    /// Each configured channel: the core staff channels, then the channels
    /// of the modules that are on.
    channels: Vec<ChannelHealth>,
    /// Each module's state.
    modules: ModulesState,
}

/// The server's health.
#[utoipa::path(get, path = "/api/guilds/{guild_id}/health", tag = "settings",
    params(("guild_id" = String, Path, description = "The server")),
    responses((status = 200, body = Health), (status = 403, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn guild_health(
    State(state): State<ApiState>,
    staff: Staff<ManageSettings>,
) -> ApiResult<Json<Health>> {
    let h = health(state.engine(), &staff.principal)
        .await
        .map_err(|e| map::settings(e, &Facts::scene(Scene::Dashboard), None, &[]))?;
    Ok(Json(Health {
        quack_in_server: h.quack_in_server,
        missing_permissions: h.missing_permissions,
        channels: h
            .channels
            .into_iter()
            .map(|c| ChannelHealth {
                staff_only: c.staff_only,
                setting: c.setting,
                channel_id: c.channel_id,
                problems: c
                    .details
                    .iter()
                    .map(|p| ChannelProblem {
                        code: p.code().to_owned(),
                        message: map::channel_problem(c.channel_id, p),
                    })
                    .collect(),
            })
            .collect(),
        modules: h.modules.into(),
    }))
}
