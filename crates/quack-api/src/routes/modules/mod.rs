//! Module routes (39–50): turning modules on and off, each module's own
//! settings, and tickets. Each handler calls the module crate's service
//! function (§17.2); the binary hands over the modules' enable hooks and
//! the server activity log's counters in [`ModuleServices`]. Ticket routes
//! 48–50 accept the ticket's member (member projection, no staff IDs) or
//! staff with `moderate` (full projection); missing and hidden tickets are
//! both `404 not_found`.

mod settings;
mod tickets;
mod types;

use axum::Json;
use axum::extract::{Path, State};
use quack_core::modules::ModuleName;
use quack_engine::settings::{ModuleHooks, set_enabled};
use quack_text::api::ApiCode;
use quack_text::error::Scene;
use serde::Deserialize;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub(crate) use types::{ModuleState, ModulesState};

use crate::error::{ApiError, ApiResult, ErrorEnvelope, Facts, map};
use crate::extract::{IdempotencyKey, ManageSettings, Staff};
use crate::state::ApiState;

/// What the module routes call into, supplied by the binary.
#[derive(Clone, Debug, Default)]
pub struct ModuleServices {
    /// The tickets module's enable check and change hooks.
    pub tickets: Option<ModuleHooks>,
    /// The server activity log's hooks.
    pub activity_log: Option<ModuleHooks>,
    /// The honeypot's hooks.
    pub honeypot: Option<ModuleHooks>,
    /// The process's server activity log, for its since-start counters
    /// (both zero without it).
    pub activity: Option<quack_activity_log::ActivityLog>,
}

impl ModuleServices {
    fn hooks(&self, module: ModuleName) -> Option<&ModuleHooks> {
        match module {
            ModuleName::Tickets => self.tickets.as_ref(),
            ModuleName::ActivityLog => self.activity_log.as_ref(),
            ModuleName::Honeypot => self.honeypot.as_ref(),
        }
    }
}

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(enable))
        .routes(routes!(disable))
        .routes(routes!(settings::tickets_module))
        .routes(routes!(settings::update_ticket_settings))
        .routes(routes!(settings::activity_log_module))
        .routes(routes!(settings::update_activity_log_settings))
        .routes(routes!(settings::honeypot_module))
        .routes(routes!(settings::update_honeypot_settings))
        .routes(routes!(tickets::list_tickets))
        .routes(routes!(tickets::get_ticket))
        .routes(routes!(tickets::ticket_transcript))
        .routes(routes!(tickets::close_ticket))
}

/// The module a URL names (`tickets`, `activity-log`, `honeypot`).
fn module_from_url(name: &str) -> Result<ModuleName, ApiError> {
    match name {
        "tickets" => Ok(ModuleName::Tickets),
        "activity-log" => Ok(ModuleName::ActivityLog),
        "honeypot" => Ok(ModuleName::Honeypot),
        _ => Err(ApiError::not_found()),
    }
}

/// A module whose hooks the binary didn't supply.
fn not_wired<T>() -> ApiResult<T> {
    Err(ApiError::api(ApiCode::Unavailable))
}

/// Routes 39–40's path.
#[derive(Debug, Deserialize)]
pub(crate) struct ModulePath {
    /// The server (checked by the staff extractor).
    #[expect(dead_code, reason = "read by the staff extractor")]
    guild_id: String,
    /// `tickets`, `activity-log`, or `honeypot`.
    module: String,
}

async fn switch(
    state: &ApiState,
    staff: &Staff<ManageSettings>,
    module: &str,
    enabled: bool,
    key: &IdempotencyKey,
) -> ApiResult<Json<ModuleState>> {
    let module = module_from_url(module)?;
    let Some(hooks) = state.modules().hooks(module) else {
        return not_wired();
    };
    set_enabled(state.engine(), &staff.principal, hooks, enabled, &key.0)
        .await
        .map(|s| Json(s.into()))
        .map_err(|e| map::settings(e, &Facts::scene(Scene::Staff), Some(module), &[]))
}

/// Turns a module on (`409 module_not_ready` lists what to fix).
#[utoipa::path(post, path = "/api/guilds/{guild_id}/modules/{module}/enable", tag = "modules",
    params(("guild_id" = String, Path), ("module" = String, Path, description = "`tickets`, `activity-log`, or `honeypot`")),
    responses((status = 200, body = ModuleState), (status = 409, body = ErrorEnvelope)),
    security(("session" = [], "csrf" = [])))]
async fn enable(
    State(state): State<ApiState>,
    staff: Staff<ManageSettings>,
    key: IdempotencyKey,
    Path(path): Path<ModulePath>,
) -> ApiResult<Json<ModuleState>> {
    switch(&state, &staff, &path.module, true, &key).await
}

/// Turns a module off.
#[utoipa::path(post, path = "/api/guilds/{guild_id}/modules/{module}/disable", tag = "modules",
    params(("guild_id" = String, Path), ("module" = String, Path)),
    responses((status = 200, body = ModuleState), (status = 403, body = ErrorEnvelope)),
    security(("session" = [], "csrf" = [])))]
async fn disable(
    State(state): State<ApiState>,
    staff: Staff<ManageSettings>,
    key: IdempotencyKey,
    Path(path): Path<ModulePath>,
) -> ApiResult<Json<ModuleState>> {
    switch(&state, &staff, &path.module, false, &key).await
}
