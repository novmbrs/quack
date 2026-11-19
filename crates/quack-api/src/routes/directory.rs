//! The server directory (routes 9–11): member search and lookup for case
//! forms and member chips, and the channels and roles settings choose
//! from. Read through the `Discord` port under §2's freshness rules.

use std::collections::HashMap;

use axum::Json;
use axum::extract::{Query, State};
use quack_core::ids::{AccountId, ChannelId, MemberId, RoleId};
use quack_core::permissions::{AccountFacts, MemberFacts};
use quack_engine::Outcome;
use quack_engine::port::{Fresh, Membership};
use quack_text::api::ApiCode;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::error::{ApiError, ApiResult, ErrorEnvelope};
use crate::extract::{ManageSettings, Moderate, Staff, snowflake};
use crate::pagination::limit_within;
use crate::state::ApiState;
use crate::wire::{avatar_url, schema};

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(members))
        .routes(routes!(channels))
        .routes(routes!(roles))
}

fn discord<T>(outcome: Outcome<T>) -> ApiResult<T> {
    outcome
        .done()
        .ok_or_else(|| ApiError::api(ApiCode::DiscordUnavailable))
}

/// Route 9's query: `query` (with `limit`) searches; `ids` looks up.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct MemberQuery {
    /// Names starting with this (1–100 characters).
    query: Option<String>,
    /// 1–25, default 10 (search only).
    limit: Option<u32>,
    /// Comma-separated account IDs (at most 100).
    ids: Option<String>,
}

/// A member or account, for display.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct DirectoryMember {
    #[schema(value_type = schema::Snowflake)]
    id: AccountId,
    username: String,
    global_name: Option<String>,
    nick: Option<String>,
    /// Nickname, else global name, else username.
    display_name: String,
    avatar_url: String,
    bot: bool,
    /// Whether they are in the server now.
    in_server: bool,
}

/// Route 9's body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct DirectoryMembers {
    members: Vec<DirectoryMember>,
}

fn from_member(m: &MemberFacts, avatar: Option<&str>) -> DirectoryMember {
    let id = m.id.account();
    DirectoryMember {
        id,
        username: m.username.clone(),
        global_name: m.global_name.clone(),
        nick: m.nick.clone(),
        display_name: m.display_name().to_owned(),
        avatar_url: avatar_url(id.get(), avatar),
        bot: m.bot,
        in_server: true,
    }
}

fn from_account(a: &AccountFacts) -> DirectoryMember {
    DirectoryMember {
        id: a.id,
        username: a.username.clone(),
        global_name: a.global_name.clone(),
        nick: None,
        display_name: a.global_name.clone().unwrap_or_else(|| a.username.clone()),
        avatar_url: avatar_url(a.id.get(), a.avatar_hash.as_deref()),
        bot: a.bot,
        in_server: false,
    }
}

fn bad_query(field: &str) -> ApiError {
    ApiError::field(
        field,
        "invalid",
        quack_text::render::plain(&quack_text::api::value_invalid()),
    )
}

/// Searches the server's members, or looks accounts up by ID.
#[utoipa::path(get, path = "/api/guilds/{guild_id}/directory/members", tag = "directory",
    params(("guild_id" = String, Path, description = "The server"), MemberQuery),
    responses((status = 200, body = DirectoryMembers), (status = 400, body = ErrorEnvelope),
              (status = 502, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn members(
    State(state): State<ApiState>,
    staff: Staff<Moderate>,
    Query(q): Query<MemberQuery>,
) -> ApiResult<Json<DirectoryMembers>> {
    let guild = staff.guild();
    let port = state.engine().discord();
    let (ids, found) = match (q.query.as_deref(), q.ids.as_deref()) {
        (Some(text), None) => {
            let len = text.chars().count();
            if !(1..=100).contains(&len) {
                return Err(bad_query("query"));
            }
            let limit = limit_within(q.limit, 10, 25)?;
            let limit = u8::try_from(limit).unwrap_or(25);
            let found = discord(port.search_members(guild, text, limit).await)?;
            (found.iter().map(|m| m.id.account()).collect(), found)
        }
        (None, Some(list)) => {
            let ids: Vec<AccountId> = list
                .split(',')
                .filter(|s| !s.is_empty())
                .map(|s| snowflake(s, "ids"))
                .collect::<ApiResult<_>>()?;
            if ids.is_empty() || ids.len() > 100 {
                return Err(bad_query("ids"));
            }
            let mut found = Vec::new();
            for id in &ids {
                if let Membership::Present(m) =
                    discord(port.member(guild, id.member(), Fresh::Cached).await)?
                {
                    found.push(m);
                }
            }
            (ids, found)
        }
        _ => return Err(bad_query("query")),
    };
    let accounts: HashMap<AccountId, AccountFacts> = discord(port.accounts(&ids).await)?
        .into_iter()
        .map(|a| (a.id, a))
        .collect();
    let present: HashMap<MemberId, &MemberFacts> = found.iter().map(|m| (m.id, m)).collect();
    let members = ids
        .iter()
        .filter_map(|id| {
            let account = accounts.get(id);
            match present.get(&id.member()) {
                Some(m) => Some(from_member(
                    m,
                    account.and_then(|a| a.avatar_hash.as_deref()),
                )),
                None => account.map(from_account),
            }
        })
        .collect();
    Ok(Json(DirectoryMembers { members }))
}

/// A channel kind the dashboard shows.
#[derive(Clone, Copy, Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ChannelKind {
    Text,
    Announcement,
    Forum,
    Voice,
    Stage,
    Category,
}

impl ChannelKind {
    /// Discord's channel type; threads and other kinds are left out.
    const fn from_discord(kind: u8) -> Option<Self> {
        match kind {
            0 => Some(Self::Text),
            2 => Some(Self::Voice),
            4 => Some(Self::Category),
            5 => Some(Self::Announcement),
            13 => Some(Self::Stage),
            15 => Some(Self::Forum),
            _ => None,
        }
    }
}

/// One channel.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct DirectoryChannel {
    #[schema(value_type = schema::Snowflake)]
    id: ChannelId,
    name: String,
    kind: ChannelKind,
    #[schema(value_type = Option<schema::Snowflake>)]
    parent_id: Option<ChannelId>,
    /// The channel's place in Discord's list.
    position: u32,
}

/// Route 10's body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct DirectoryChannels {
    channels: Vec<DirectoryChannel>,
}

/// The server's channels (not threads).
#[utoipa::path(get, path = "/api/guilds/{guild_id}/directory/channels", tag = "directory",
    params(("guild_id" = String, Path, description = "The server")),
    responses((status = 200, body = DirectoryChannels), (status = 502, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn channels(
    State(state): State<ApiState>,
    staff: Staff<ManageSettings>,
) -> ApiResult<Json<DirectoryChannels>> {
    let all = discord(state.engine().discord().guild_channels(staff.guild()).await)?;
    let channels = all
        .into_iter()
        .filter_map(|c| ChannelKind::from_discord(c.kind).map(|kind| (c, kind)))
        .zip(0_u32..)
        .map(|((c, kind), position)| DirectoryChannel {
            id: c.id,
            name: c.name,
            kind,
            parent_id: c.parent_id,
            position,
        })
        .collect();
    Ok(Json(DirectoryChannels { channels }))
}

/// One role.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct DirectoryRole {
    #[schema(value_type = schema::Snowflake)]
    id: RoleId,
    name: String,
    /// The role's color (0 when Quack doesn't know it).
    color: u32,
    position: u32,
    /// An integration manages it.
    managed: bool,
}

/// Route 11's body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct DirectoryRoles {
    roles: Vec<DirectoryRole>,
}

/// The server's roles, highest first, without @everyone.
#[utoipa::path(get, path = "/api/guilds/{guild_id}/directory/roles", tag = "directory",
    params(("guild_id" = String, Path, description = "The server")),
    responses((status = 200, body = DirectoryRoles), (status = 502, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn roles(
    State(state): State<ApiState>,
    staff: Staff<ManageSettings>,
) -> ApiResult<Json<DirectoryRoles>> {
    let guild = staff.guild();
    let facts = discord(state.engine().discord().guild_facts(guild).await)?;
    let mut roles: Vec<DirectoryRole> = facts
        .roles
        .into_iter()
        .filter(|r| r.id != guild.everyone_role())
        .map(|r| DirectoryRole {
            id: r.id,
            name: r.name,
            color: 0,
            position: r.position,
            managed: r.managed,
        })
        .collect();
    roles.sort_by_key(|r| std::cmp::Reverse(r.position));
    Ok(Json(DirectoryRoles { roles }))
}
