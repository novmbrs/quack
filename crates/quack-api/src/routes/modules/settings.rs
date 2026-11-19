//! Routes 41–46: each module's settings page and its settings update,
//! through the module crate's `settings` and `update_settings`.
//!
//! Each page carries its settings' `version`; an update sends it back as
//! `expected_version`, and a change saved since is refused with
//! `settings_changed` (as the core settings `PATCH` does).

use axum::Json;
use axum::extract::State;
use quack_core::ids::{ChannelId, GuildId};
use quack_core::modules::ModuleName;
use quack_engine::settings::SettingsError;
use quack_text::error::Scene;

use super::types::{
    ActivityLogModule, ActivityLogSettings, ActivityLogSettingsUpdate, EligibleRule,
    HoneypotModule, HoneypotSettings, HoneypotSettingsUpdate, ModuleState, TicketSettings,
    TicketSettingsUpdate, TicketsModule,
};
use crate::error::{ApiError, ApiResult, Facts, map};
use crate::extract::{Body, IdempotencyKey, ManageSettings, Staff};
use crate::state::ApiState;

fn mapped(
    module: ModuleName,
    channels: &[(&str, ChannelId)],
) -> impl FnOnce(SettingsError) -> ApiError {
    move |e| map::settings(e, &Facts::scene(Scene::Dashboard), Some(module), channels)
}

fn state_of(
    enabled: bool,
    disabled_reason: Option<quack_core::modules::ModuleDisabledReason>,
) -> ModuleState {
    ModuleState {
        enabled,
        disabled_reason,
        problems: Vec::new(),
    }
}

async fn tickets_page(
    state: &ApiState,
    guild: GuildId,
    view: quack_tickets::TicketSettingsView,
) -> ApiResult<TicketsModule> {
    let open_tickets = {
        let mut conn = state.engine().db().conn().await?;
        quack_db::tickets::count_open(&mut conn, Some(guild)).await?
    };
    Ok(TicketsModule {
        state: state_of(view.enabled, view.disabled_reason),
        version: view.version,
        settings: TicketSettings {
            entry_channel_id: view.entry_channel_id,
            queue_channel_id: view.queue_channel_id,
            transcript_retention_days: view.transcript_retention_days,
        },
        open_tickets,
    })
}

/// The tickets module.
#[utoipa::path(get, path = "/api/guilds/{guild_id}/modules/tickets", tag = "modules",
    params(("guild_id" = String, Path)),
    responses((status = 200, body = TicketsModule)), security(("session" = [])))]
pub(super) async fn tickets_module(
    State(state): State<ApiState>,
    staff: Staff<ManageSettings>,
) -> ApiResult<Json<TicketsModule>> {
    let view = quack_tickets::settings(state.engine(), &staff.principal)
        .await
        .map_err(mapped(ModuleName::Tickets, &[]))?;
    Ok(Json(tickets_page(&state, staff.guild(), view).await?))
}

/// Changes ticket settings.
#[utoipa::path(put, path = "/api/guilds/{guild_id}/modules/tickets/settings", tag = "modules",
    params(("guild_id" = String, Path)), request_body = TicketSettingsUpdate,
    responses((status = 200, body = TicketsModule)), security(("session" = [], "csrf" = [])))]
pub(super) async fn update_ticket_settings(
    State(state): State<ApiState>,
    staff: Staff<ManageSettings>,
    key: IdempotencyKey,
    Body(body): Body<TicketSettingsUpdate>,
) -> ApiResult<Json<TicketsModule>> {
    let engine = state.engine();
    let channels: Vec<(&str, ChannelId)> = [
        ("entry_channel_id", body.entry_channel_id),
        ("queue_channel_id", body.queue_channel_id),
    ]
    .into_iter()
    .filter_map(|(f, c)| c.map(|c| (f, c)))
    .collect();
    let input = quack_tickets::TicketSettingsInput {
        expected_version: body.expected_version,
        entry_channel_id: body.entry_channel_id,
        queue_channel_id: body.queue_channel_id,
        transcript_retention_days: body.transcript_retention_days,
    };
    let view = quack_tickets::update_settings(engine, &staff.principal, &input, &key.0)
        .await
        .map_err(mapped(ModuleName::Tickets, &channels))?;
    Ok(Json(tickets_page(&state, staff.guild(), view).await?))
}

fn activity_page(
    state: &ApiState,
    guild: GuildId,
    view: quack_activity_log::ActivitySettingsView,
) -> ActivityLogModule {
    let (posted, dropped) = state
        .modules()
        .activity
        .as_ref()
        .map_or((0, 0), |log| log.counts(guild));
    ActivityLogModule {
        state: state_of(view.enabled, view.disabled_reason),
        version: view.version,
        settings: ActivityLogSettings {
            routes: view.routes,
            include_message_content: view.include_message_content,
            include_file_names: view.include_file_names,
            include_embed_types: view.include_embed_types,
        },
        posted_since_start: posted,
        dropped_since_start: dropped,
    }
}

/// The server activity log module. Its counters are in-process and reset
/// when Quack restarts.
#[utoipa::path(get, path = "/api/guilds/{guild_id}/modules/activity-log", tag = "modules",
    params(("guild_id" = String, Path)),
    responses((status = 200, body = ActivityLogModule)), security(("session" = [])))]
pub(super) async fn activity_log_module(
    State(state): State<ApiState>,
    staff: Staff<ManageSettings>,
) -> ApiResult<Json<ActivityLogModule>> {
    let view = quack_activity_log::settings(state.engine(), &staff.principal)
        .await
        .map_err(mapped(ModuleName::ActivityLog, &[]))?;
    Ok(Json(activity_page(&state, staff.guild(), view)))
}

/// Changes server activity log settings.
#[utoipa::path(put, path = "/api/guilds/{guild_id}/modules/activity-log/settings", tag = "modules",
    params(("guild_id" = String, Path)), request_body = ActivityLogSettingsUpdate,
    responses((status = 200, body = ActivityLogModule)), security(("session" = [], "csrf" = [])))]
pub(super) async fn update_activity_log_settings(
    State(state): State<ApiState>,
    staff: Staff<ManageSettings>,
    key: IdempotencyKey,
    Body(body): Body<ActivityLogSettingsUpdate>,
) -> ApiResult<Json<ActivityLogModule>> {
    let engine = state.engine();
    let names: Vec<String> = body.routes.keys().map(|e| e.as_str().to_owned()).collect();
    let channels: Vec<(&str, ChannelId)> = body
        .routes
        .iter()
        .zip(&names)
        .map(|((_, c), name)| (name.as_str(), *c))
        .collect();
    let input = quack_activity_log::ActivitySettingsInput {
        expected_version: body.expected_version,
        include_message_content: body.include_message_content,
        include_file_names: body.include_file_names,
        include_embed_types: body.include_embed_types,
        routes: body.routes.clone(),
    };
    let view = quack_activity_log::update_settings(engine, &staff.principal, &input, &key.0)
        .await
        .map_err(mapped(ModuleName::ActivityLog, &channels))?;
    Ok(Json(activity_page(&state, staff.guild(), view)))
}

async fn honeypot_page(
    state: &ApiState,
    guild: GuildId,
    view: quack_honeypot::HoneypotSettingsView,
) -> ApiResult<HoneypotModule> {
    let engine = state.engine();
    let rules = engine.rules_cache().get(engine.db(), guild).await?;
    let mut eligible_rules: Vec<EligibleRule> = rules
        .active
        .iter()
        .filter(|r| !r.definition.has_required_fields())
        .map(|r| EligibleRule {
            id: r.id,
            name: r.definition.name.clone(),
            levels: r.definition.levels.clone(),
        })
        .collect();
    eligible_rules.sort_by_key(|r| r.name.to_lowercase());
    let cases_created = {
        let mut conn = engine.db().conn().await?;
        quack_db::cases::count_honeypot(&mut conn, guild).await?
    };
    Ok(HoneypotModule {
        state: state_of(view.enabled, view.disabled_reason),
        version: view.version,
        settings: HoneypotSettings {
            channel_id: view.channel_id,
            rule_id: view.rule_id,
            warning_text: view.warning_text,
        },
        eligible_rules,
        cases_created,
        pending_hits: view.pending_hits,
        failed_hits: view.failed_hits,
    })
}

/// The honeypot module.
#[utoipa::path(get, path = "/api/guilds/{guild_id}/modules/honeypot", tag = "modules",
    params(("guild_id" = String, Path)),
    responses((status = 200, body = HoneypotModule)), security(("session" = [])))]
pub(super) async fn honeypot_module(
    State(state): State<ApiState>,
    staff: Staff<ManageSettings>,
) -> ApiResult<Json<HoneypotModule>> {
    let view = quack_honeypot::settings(state.engine(), &staff.principal)
        .await
        .map_err(mapped(ModuleName::Honeypot, &[]))?;
    Ok(Json(honeypot_page(&state, staff.guild(), view).await?))
}

/// Changes honeypot settings.
#[utoipa::path(put, path = "/api/guilds/{guild_id}/modules/honeypot/settings", tag = "modules",
    params(("guild_id" = String, Path)), request_body = HoneypotSettingsUpdate,
    responses((status = 200, body = HoneypotModule)), security(("session" = [], "csrf" = [])))]
pub(super) async fn update_honeypot_settings(
    State(state): State<ApiState>,
    staff: Staff<ManageSettings>,
    key: IdempotencyKey,
    Body(body): Body<HoneypotSettingsUpdate>,
) -> ApiResult<Json<HoneypotModule>> {
    let engine = state.engine();
    let channels: Vec<(&str, ChannelId)> = body
        .channel_id
        .map(|c| ("channel_id", c))
        .into_iter()
        .collect();
    let input = quack_honeypot::HoneypotSettingsInput {
        expected_version: body.expected_version,
        channel_id: body.channel_id,
        rule_id: body.rule_id,
        warning_text: body.warning_text,
    };
    let view = quack_honeypot::update_settings(engine, &staff.principal, &input, &key.0)
        .await
        .map_err(mapped(ModuleName::Honeypot, &channels))?;
    Ok(Json(honeypot_page(&state, staff.guild(), view).await?))
}
