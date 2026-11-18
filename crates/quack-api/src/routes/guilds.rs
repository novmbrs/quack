//! The account's servers (route 5) and the reader in one server (route 6).

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use quack_core::ids::{AccountId, GuildId};
use quack_core::permissions::Capabilities as CoreCapabilities;
use quack_db::{actions, appeals, guilds};
use quack_engine::Origin;
use quack_text::api::ApiCode;
use serde::Serialize;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::auth::oauth::{OauthError, UserGuild};
use crate::error::{ApiError, ApiResult, ErrorEnvelope};
use crate::extract::{InServer, SessionAccount, Staff};
use crate::state::ApiState;
use crate::wire::{icon_url, schema};

/// Discord's Administrator and Manage Server permission bits.
const ADMINISTRATOR: u64 = 1 << 3;
const MANAGE_GUILD: u64 = 1 << 5;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(list_guilds))
        .routes(routes!(guild_me))
}

/// Effective capabilities (after the 2FA rule).
#[expect(
    clippy::struct_excessive_bools,
    reason = "one flag per capability, as on the wire"
)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, ToSchema)]
pub(crate) struct Capabilities {
    moderate: bool,
    manage_rules: bool,
    manage_settings: bool,
    manage_staff_roles: bool,
}

impl Capabilities {
    fn effective(c: CoreCapabilities) -> Self {
        Self {
            moderate: c.moderate && c.mfa_ok,
            manage_rules: c.manage_rules && c.mfa_ok,
            manage_settings: c.manage_settings && c.mfa_ok,
            manage_staff_roles: c.manage_staff_roles && c.mfa_ok,
        }
    }
}

/// Whether the 2FA rule blocks capabilities the account would have.
fn mfa_blocked(c: CoreCapabilities) -> bool {
    !c.mfa_ok && (c.moderate || c.manage_rules || c.manage_settings || c.manage_staff_roles)
}

/// One of the account's servers.
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent flags, as on the wire"
)]
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct GuildSummary {
    #[schema(value_type = schema::Snowflake)]
    guild_id: GuildId,
    name: String,
    icon_url: Option<String>,
    /// Quack is installed there.
    installed: bool,
    capabilities: Capabilities,
    /// The server requires 2FA for moderation.
    mfa_required: bool,
    /// The account would have a staff capability but Quack has no record
    /// of its 2FA.
    mfa_blocked: bool,
    /// Not installed, and the account has Manage Server there.
    can_install: bool,
}

/// Route 5's body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct GuildList {
    guilds: Vec<GuildSummary>,
}

async fn account_guilds(
    state: &ApiState,
    session: &SessionAccount,
) -> ApiResult<Arc<Vec<UserGuild>>> {
    let id_hash = session.session.id_hash;
    if let Some(list) = state.account_guilds().get(&id_hash) {
        return Ok(list);
    }
    let token = session
        .session
        .access_token(state)
        .ok_or_else(|| ApiError::api(ApiCode::SessionExpired))?;
    let list = state
        .oauth()
        .user_guilds(&token)
        .await
        .map_err(|e| match e {
            OauthError::Status(401) => ApiError::api(ApiCode::SessionExpired),
            OauthError::Status(_) | OauthError::Transport | OauthError::Body => {
                ApiError::api(ApiCode::DiscordUnavailable)
            }
        })?;
    let list = Arc::new(list);
    state.account_guilds().put(id_hash, list.clone());
    Ok(list)
}

/// The account's Discord servers, with Quack's install state and the
/// account's capabilities in each installed one.
#[utoipa::path(get, path = "/api/guilds", tag = "guilds",
    responses((status = 200, body = GuildList), (status = 401, body = ErrorEnvelope),
              (status = 502, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn list_guilds(
    State(state): State<ApiState>,
    session: SessionAccount,
) -> ApiResult<Json<GuildList>> {
    let list = account_guilds(&state, &session).await?;
    let ids: Vec<GuildId> = list.iter().filter_map(|g| g.id.parse().ok()).collect();
    let installed: HashMap<GuildId, bool> = {
        let mut conn = state.engine().db().conn().await?;
        guilds::get_many(&mut conn, &ids)
            .await?
            .into_iter()
            .map(|g| (g.id, g.installed))
            .collect()
    };
    let mut out = Vec::new();
    for g in list.iter() {
        let Ok(guild_id) = g.id.parse::<GuildId>() else {
            continue;
        };
        let is_installed = installed.get(&guild_id).copied().unwrap_or(false);
        let (caps, mfa_required) = if is_installed {
            capabilities(&state, guild_id, session.account).await
        } else {
            (None, false)
        };
        let bits = g
            .permissions
            .as_deref()
            .and_then(|p| p.parse::<u64>().ok())
            .unwrap_or(0);
        out.push(GuildSummary {
            guild_id,
            name: g.name.clone(),
            icon_url: icon_url(guild_id.get(), g.icon.as_deref()),
            installed: is_installed,
            capabilities: caps.map_or_else(Capabilities::default, Capabilities::effective),
            mfa_required,
            mfa_blocked: caps.is_some_and(mfa_blocked),
            can_install: !is_installed && (g.owner || bits & (ADMINISTRATOR | MANAGE_GUILD) != 0),
        });
    }
    Ok(Json(GuildList { guilds: out }))
}

/// The account's capabilities in an installed server, from current
/// facts; none when Discord can't say or the account isn't a member.
async fn capabilities(
    state: &ApiState,
    guild: GuildId,
    account: AccountId,
) -> (Option<CoreCapabilities>, bool) {
    let Ok(principal) = state
        .engine()
        .principal(guild, account, Origin::Dashboard)
        .await
    else {
        return (None, false);
    };
    if principal.member.is_none() {
        return (None, principal.mfa_required);
    }
    let Ok(mut conn) = state.engine().db().conn().await else {
        return (None, principal.mfa_required);
    };
    let caps = principal.current_capabilities(&mut conn).await.ok();
    (caps, principal.mfa_required)
}

/// The server in brief.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct GuildInfo {
    #[schema(value_type = schema::Snowflake)]
    guild_id: GuildId,
    name: String,
    icon_url: Option<String>,
    #[schema(value_type = schema::Snowflake)]
    owner_id: AccountId,
}

/// Exact counts for the staff layout.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct GuildCounts {
    /// Appeals waiting for staff.
    pending_appeals: u64,
    /// Failed actions not dismissed.
    failed_actions: u64,
}

/// The reader in one server (route 6).
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct GuildMe {
    guild: GuildInfo,
    capabilities: Capabilities,
    mfa_required: bool,
    mfa_blocked: bool,
    is_owner: bool,
    /// Present for staff with `moderate`.
    counts: Option<GuildCounts>,
}

/// The reader's capabilities in the server, and staff counts.
#[utoipa::path(get, path = "/api/guilds/{guild_id}/me", tag = "guilds",
    params(("guild_id" = String, Path, description = "The server")),
    responses((status = 200, body = GuildMe), (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn guild_me(
    State(state): State<ApiState>,
    staff: Staff<InServer>,
) -> ApiResult<Json<GuildMe>> {
    let guild = staff.guild();
    let mut conn = state.engine().db().conn().await?;
    let stored = guilds::get(&mut conn, guild)
        .await?
        .ok_or_else(|| ApiError::api(ApiCode::ServerNotFound))?;
    let caps = Capabilities::effective(staff.capabilities);
    let counts = if caps.moderate {
        Some(GuildCounts {
            pending_appeals: appeals::count_pending(&mut conn, guild).await?,
            failed_actions: actions::count_failed(&mut conn, guild).await?,
        })
    } else {
        None
    };
    Ok(Json(GuildMe {
        guild: GuildInfo {
            guild_id: guild,
            name: stored.name,
            icon_url: icon_url(guild.get(), stored.icon_hash.as_deref()),
            owner_id: stored.owner_id,
        },
        capabilities: caps,
        mfa_required: staff.principal.mfa_required,
        mfa_blocked: mfa_blocked(staff.capabilities),
        is_owner: staff.principal.actor.is_owner,
        counts,
    }))
}
