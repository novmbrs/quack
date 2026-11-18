//! Server settings (routes 36–38): read, change in one transaction, and
//! acknowledge the starter-rule notice.

use axum::Json;
use axum::extract::State;
use quack_core::ids::{ChannelId, RoleId, RuleId};
use quack_engine::settings::{self, SettingsPatch, SettingsView};
use quack_text::error::Scene;
use serde::{Deserialize, Deserializer, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::modules::ModulesState;
use crate::error::{ApiResult, ErrorEnvelope, Facts, map};
use crate::extract::{Body, IdempotencyKey, ManageSettings, Staff, snowflake};
use crate::state::ApiState;
use crate::wire::{schema, ts};

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(get_settings, patch_settings))
        .routes(routes!(acknowledge_starter_notice))
}

/// The server's settings.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct Settings {
    /// Send back as `expected_version` when changing role lists.
    version: u64,
    #[schema(value_type = Vec<schema::Snowflake>)]
    moderator_role_ids: Vec<RoleId>,
    #[schema(value_type = Vec<schema::Snowflake>)]
    rules_manager_role_ids: Vec<RoleId>,
    #[schema(value_type = Option<schema::Snowflake>)]
    log_channel_id: Option<ChannelId>,
    #[schema(value_type = Option<schema::Snowflake>)]
    appeal_channel_id: Option<ChannelId>,
    #[schema(value_type = Option<schema::Snowflake>)]
    evidence_channel_id: Option<ChannelId>,
    appeal_invite_url: Option<String>,
    dm_intro: Option<String>,
    dm_footer: Option<String>,
    #[schema(value_type = Option<uuid::Uuid>)]
    starter_rule_id: Option<RuleId>,
    #[serde(with = "ts::option")]
    #[schema(value_type = Option<String>, format = DateTime)]
    starter_notice_acknowledged_at: Option<OffsetDateTime>,
    modules: ModulesState,
}

impl From<SettingsView> for Settings {
    fn from(s: SettingsView) -> Self {
        Self {
            version: s.version,
            moderator_role_ids: s.moderator_role_ids,
            rules_manager_role_ids: s.rules_manager_role_ids,
            log_channel_id: s.log_channel_id,
            appeal_channel_id: s.appeal_channel_id,
            evidence_channel_id: s.evidence_channel_id,
            appeal_invite_url: s.appeal_invite_url,
            dm_intro: s.dm_intro,
            dm_footer: s.dm_footer,
            starter_rule_id: s.starter_rule_id,
            starter_notice_acknowledged_at: s.starter_notice_acknowledged_at,
            modules: s.modules.into(),
        }
    }
}

/// Present-and-null (`Some(None)`) apart from absent (`None`).
#[expect(
    clippy::option_option,
    reason = "absent keeps, null clears, a value sets (the engine's patch shape)"
)]
fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(d).map(Some)
}

/// Route 37's body: only the fields sent change. A channel sent as null
/// is cleared; text sent empty is cleared.
#[expect(
    clippy::option_option,
    reason = "absent keeps, null clears, a value sets (the engine's patch shape)"
)]
#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SettingsChange {
    /// Required with a role list: the `version` the editor loaded.
    expected_version: Option<u64>,
    /// Needs `manage_staff_roles` when it changes.
    #[schema(value_type = Option<Vec<schema::Snowflake>>)]
    moderator_role_ids: Option<Vec<String>>,
    #[schema(value_type = Option<Vec<schema::Snowflake>>)]
    rules_manager_role_ids: Option<Vec<String>>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<schema::Snowflake>)]
    log_channel_id: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<schema::Snowflake>)]
    appeal_channel_id: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<schema::Snowflake>)]
    evidence_channel_id: Option<Option<String>>,
    appeal_invite_url: Option<String>,
    dm_intro: Option<String>,
    dm_footer: Option<String>,
}

fn roles(list: Option<&[String]>, field: &str) -> ApiResult<Option<Vec<RoleId>>> {
    list.map(|ids| {
        ids.iter()
            .enumerate()
            .map(|(i, id)| snowflake(id, &format!("{field}[{i}]")))
            .collect()
    })
    .transpose()
}

#[expect(
    clippy::option_option,
    reason = "absent keeps, null clears, a value sets (the engine's patch shape)"
)]
fn channel(value: Option<&Option<String>>, field: &str) -> ApiResult<Option<Option<ChannelId>>> {
    value
        .map(|v| v.as_deref().map(|id| snowflake(id, field)).transpose())
        .transpose()
}

impl SettingsChange {
    fn patch(&self) -> ApiResult<SettingsPatch> {
        Ok(SettingsPatch {
            expected_version: self.expected_version,
            moderator_role_ids: roles(self.moderator_role_ids.as_deref(), "moderator_role_ids")?,
            rules_manager_role_ids: roles(
                self.rules_manager_role_ids.as_deref(),
                "rules_manager_role_ids",
            )?,
            log_channel_id: channel(self.log_channel_id.as_ref(), "log_channel_id")?,
            appeal_channel_id: channel(self.appeal_channel_id.as_ref(), "appeal_channel_id")?,
            evidence_channel_id: channel(self.evidence_channel_id.as_ref(), "evidence_channel_id")?,
            appeal_invite_url: self.appeal_invite_url.clone(),
            dm_intro: self.dm_intro.clone(),
            dm_footer: self.dm_footer.clone(),
        })
    }
}

fn facts() -> Facts {
    Facts::scene(Scene::Staff)
}

/// The server's settings.
#[utoipa::path(get, path = "/api/guilds/{guild_id}/settings", tag = "settings",
    params(("guild_id" = String, Path)),
    responses((status = 200, body = Settings), (status = 403, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn get_settings(
    State(state): State<ApiState>,
    staff: Staff<ManageSettings>,
) -> ApiResult<Json<Settings>> {
    settings::get(state.engine(), &staff.principal)
        .await
        .map(|s| Json(s.into()))
        .map_err(|e| map::settings(e, &facts(), None, &[]))
}

/// Changes settings; every field is validated and nothing is saved when
/// one has a problem.
#[utoipa::path(patch, path = "/api/guilds/{guild_id}/settings", tag = "settings",
    params(("guild_id" = String, Path)), request_body = SettingsChange,
    responses((status = 200, body = Settings), (status = 400, body = ErrorEnvelope),
              (status = 403, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope)),
    security(("session" = [], "csrf" = [])))]
async fn patch_settings(
    State(state): State<ApiState>,
    staff: Staff<ManageSettings>,
    key: IdempotencyKey,
    Body(change): Body<SettingsChange>,
) -> ApiResult<Json<Settings>> {
    let patch = change.patch()?;
    let channels: Vec<(&str, ChannelId)> = [
        ("log_channel_id", patch.log_channel_id.flatten()),
        ("appeal_channel_id", patch.appeal_channel_id.flatten()),
        ("evidence_channel_id", patch.evidence_channel_id.flatten()),
    ]
    .into_iter()
    .filter_map(|(f, c)| c.map(|c| (f, c)))
    .collect();
    settings::patch(state.engine(), &staff.principal, &patch, &key.0)
        .await
        .map(|s| Json(s.into()))
        .map_err(|e| map::settings(e, &facts(), None, &channels))
}

/// Hides the starter-rule notice for everyone in the server.
#[utoipa::path(post, path = "/api/guilds/{guild_id}/settings/starter-notice/acknowledge", tag = "settings",
    params(("guild_id" = String, Path)),
    responses((status = 200, body = Settings), (status = 403, body = ErrorEnvelope)),
    security(("session" = [], "csrf" = [])))]
async fn acknowledge_starter_notice(
    State(state): State<ApiState>,
    staff: Staff<ManageSettings>,
    key: IdempotencyKey,
) -> ApiResult<Json<Settings>> {
    settings::acknowledge_starter_notice(state.engine(), &staff.principal, &key.0)
        .await
        .map(|s| Json(s.into()))
        .map_err(|e| map::settings(e, &facts(), None, &[]))
}
