//! Module routes (39–50): turning modules on and off, and each module's
//! own settings and tickets.
//!
//! # Integration point
//!
//! The module crates (`quack-tickets`, `quack-activity-log`,
//! `quack-honeypot`) are not wired yet. What exists now:
//!
//! - Routes 39–40 (enable/disable) work through the engine
//!   (`quack_engine::settings::set_enabled`) with the module's
//!   [`ModuleHooks`], which the binary hands over in [`ModuleServices`].
//!   A module without hooks answers `503 unavailable`.
//! - Routes 41–50 are routed, authorized, documented, and typed
//!   ([`types`]); until their module's service functions are called here
//!   they answer `503 unavailable` after the capability check. To wire
//!   one, add the module crate as a dependency and replace the
//!   `not_wired()` call with the service function named in its doc:
//!   tickets `settings`, `update_settings`, `list`, `get`, `transcript`,
//!   `close`; activity log `settings`, `update_settings`; honeypot
//!   `settings`, `update_settings` (§17.2). Ticket routes 48–50 accept the
//!   ticket's member (member projection, no staff IDs) or staff with
//!   `moderate` (full projection); missing and hidden tickets are both
//!   `404 not_found`.

mod types;

use axum::Json;
use axum::extract::{Path, Query, State};
use quack_core::modules::ModuleName;
use quack_engine::settings::{ModuleHooks, set_enabled};
use quack_text::api::ApiCode;
use quack_text::error::Scene;
use serde::Deserialize;
use utoipa::IntoParams;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use types::{
    ActivityLogModule, ActivityLogSettings, HoneypotModule, HoneypotSettings, Ticket, TicketPage,
    TicketSettings, TicketsModule, Transcript,
};
pub(crate) use types::{ModuleState, ModulesState};

use crate::error::{ApiError, ApiResult, ErrorEnvelope, Facts, map};
use crate::extract::{Body, IdempotencyKey, ManageSettings, Moderate, SessionAccount, Staff};
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
        .routes(routes!(tickets_module))
        .routes(routes!(update_ticket_settings))
        .routes(routes!(activity_log_module))
        .routes(routes!(update_activity_log_settings))
        .routes(routes!(honeypot_module))
        .routes(routes!(update_honeypot_settings))
        .routes(routes!(list_tickets))
        .routes(routes!(get_ticket))
        .routes(routes!(ticket_transcript))
        .routes(routes!(close_ticket))
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

/// A module route whose module crate isn't wired yet.
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

/// The tickets module. Wire: `quack_tickets::settings`.
#[utoipa::path(get, path = "/api/guilds/{guild_id}/modules/tickets", tag = "modules",
    params(("guild_id" = String, Path)),
    responses((status = 200, body = TicketsModule)), security(("session" = [])))]
async fn tickets_module(_staff: Staff<ManageSettings>) -> ApiResult<Json<TicketsModule>> {
    not_wired()
}

/// Changes ticket settings. Wire: `quack_tickets::update_settings`.
#[utoipa::path(put, path = "/api/guilds/{guild_id}/modules/tickets/settings", tag = "modules",
    params(("guild_id" = String, Path)), request_body = TicketSettings,
    responses((status = 200, body = TicketsModule)), security(("session" = [], "csrf" = [])))]
async fn update_ticket_settings(
    _staff: Staff<ManageSettings>,
    _key: IdempotencyKey,
    Body(_body): Body<TicketSettings>,
) -> ApiResult<Json<TicketsModule>> {
    not_wired()
}

/// The server activity log module. Wire: `quack_activity_log::settings`.
#[utoipa::path(get, path = "/api/guilds/{guild_id}/modules/activity-log", tag = "modules",
    params(("guild_id" = String, Path)),
    responses((status = 200, body = ActivityLogModule)), security(("session" = [])))]
async fn activity_log_module(_staff: Staff<ManageSettings>) -> ApiResult<Json<ActivityLogModule>> {
    not_wired()
}

/// Changes activity log settings. Wire: `quack_activity_log::update_settings`.
#[utoipa::path(put, path = "/api/guilds/{guild_id}/modules/activity-log/settings", tag = "modules",
    params(("guild_id" = String, Path)), request_body = ActivityLogSettings,
    responses((status = 200, body = ActivityLogModule)), security(("session" = [], "csrf" = [])))]
async fn update_activity_log_settings(
    _staff: Staff<ManageSettings>,
    _key: IdempotencyKey,
    Body(_body): Body<ActivityLogSettings>,
) -> ApiResult<Json<ActivityLogModule>> {
    not_wired()
}

/// The honeypot module. Wire: `quack_honeypot::settings`.
#[utoipa::path(get, path = "/api/guilds/{guild_id}/modules/honeypot", tag = "modules",
    params(("guild_id" = String, Path)),
    responses((status = 200, body = HoneypotModule)), security(("session" = [])))]
async fn honeypot_module(_staff: Staff<ManageSettings>) -> ApiResult<Json<HoneypotModule>> {
    not_wired()
}

/// Changes honeypot settings. Wire: `quack_honeypot::update_settings`.
#[utoipa::path(put, path = "/api/guilds/{guild_id}/modules/honeypot/settings", tag = "modules",
    params(("guild_id" = String, Path)), request_body = HoneypotSettings,
    responses((status = 200, body = HoneypotModule)), security(("session" = [], "csrf" = [])))]
async fn update_honeypot_settings(
    _staff: Staff<ManageSettings>,
    _key: IdempotencyKey,
    Body(_body): Body<HoneypotSettings>,
) -> ApiResult<Json<HoneypotModule>> {
    not_wired()
}

/// Route 47's query.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct TicketListQuery {
    /// Only tickets in this state.
    state: Option<String>,
    /// 1–100, default 50.
    limit: Option<u32>,
    /// From the previous page's `next_cursor`.
    cursor: Option<String>,
}

/// The server's tickets. Wire: `quack_tickets::list`.
#[utoipa::path(get, path = "/api/guilds/{guild_id}/tickets", tag = "modules",
    params(("guild_id" = String, Path), TicketListQuery),
    responses((status = 200, body = TicketPage)), security(("session" = [])))]
async fn list_tickets(
    _staff: Staff<Moderate>,
    Query(query): Query<TicketListQuery>,
) -> ApiResult<Json<TicketPage>> {
    crate::pagination::limit(query.limit)?;
    let _ = (query.state, query.cursor);
    not_wired()
}

/// One ticket: its member or staff. Wire: `quack_tickets::get`.
#[utoipa::path(get, path = "/api/guilds/{guild_id}/tickets/{ticket_id}", tag = "modules",
    params(("guild_id" = String, Path), ("ticket_id" = String, Path)),
    responses((status = 200, body = Ticket), (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn get_ticket(_session: SessionAccount) -> ApiResult<Json<Ticket>> {
    not_wired()
}

/// A ticket's transcript. Wire: `quack_tickets::transcript`.
#[utoipa::path(get, path = "/api/guilds/{guild_id}/tickets/{ticket_id}/transcript", tag = "modules",
    params(("guild_id" = String, Path), ("ticket_id" = String, Path)),
    responses((status = 200, body = Transcript), (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn ticket_transcript(_session: SessionAccount) -> ApiResult<Json<Transcript>> {
    not_wired()
}

/// Closes a ticket. Wire: `quack_tickets::close`.
#[utoipa::path(post, path = "/api/guilds/{guild_id}/tickets/{ticket_id}/close", tag = "modules",
    params(("guild_id" = String, Path), ("ticket_id" = String, Path)),
    responses((status = 200, body = Ticket), (status = 409, body = ErrorEnvelope)),
    security(("session" = [], "csrf" = [])))]
async fn close_ticket(_session: SessionAccount, _key: IdempotencyKey) -> ApiResult<Json<Ticket>> {
    not_wired()
}
