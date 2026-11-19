//! The staff extractor: a session, membership in an installed server
//! (current facts, §2), and the route's capability with the 2FA rule —
//! all checked before the handler runs, so an operation-key replay is
//! never served to someone who lost access.

use std::marker::PhantomData;

use axum::extract::{FromRequestParts, RawPathParams};
use axum::http::request::Parts;
use quack_core::DomainError;
use quack_core::ids::GuildId;
use quack_core::permissions::{Capabilities, Capability};
use quack_engine::{Origin, Principal};
use quack_text::api::ApiCode;
use quack_text::error::{ErrorFacts, Scene};

use super::{SessionAccount, snowflake};
use crate::error::{ApiError, map};
use crate::state::ApiState;

/// What a staff route requires of the principal's capabilities.
pub trait Requirement: Send + Sync + 'static {
    /// `Ok` when `caps` (with the 2FA rule) allow the route.
    fn check(caps: Capabilities) -> Result<(), DomainError>;
}

/// `staff:moderate`.
#[derive(Debug)]
pub struct Moderate;
/// `staff:manage_rules`.
#[derive(Debug)]
pub struct ManageRules;
/// `staff:manage_settings`.
#[derive(Debug)]
pub struct ManageSettings;
/// `staff:moderate or manage_rules` (reading rules).
#[derive(Debug)]
pub struct ReadRules;
/// Only membership of the server (route 6).
#[derive(Debug)]
pub struct InServer;

fn one(caps: Capabilities, cap: Capability) -> Result<(), DomainError> {
    caps.check(cap).map_err(DomainError::from)
}

impl Requirement for Moderate {
    fn check(caps: Capabilities) -> Result<(), DomainError> {
        one(caps, Capability::Moderate)
    }
}

impl Requirement for ManageRules {
    fn check(caps: Capabilities) -> Result<(), DomainError> {
        one(caps, Capability::ManageRules)
    }
}

impl Requirement for ManageSettings {
    fn check(caps: Capabilities) -> Result<(), DomainError> {
        one(caps, Capability::ManageSettings)
    }
}

impl Requirement for ReadRules {
    fn check(caps: Capabilities) -> Result<(), DomainError> {
        if caps.holds(Capability::ManageRules) && !caps.holds(Capability::Moderate) {
            one(caps, Capability::ManageRules)
        } else {
            one(caps, Capability::Moderate)
        }
    }
}

impl Requirement for InServer {
    fn check(_: Capabilities) -> Result<(), DomainError> {
        Ok(())
    }
}

/// A staff member of the path's server, holding requirement `R`.
#[derive(Debug)]
pub struct Staff<R> {
    /// The principal, built from current facts for this request.
    pub principal: Principal,
    /// Their capabilities now (effective after the 2FA rule via
    /// `mfa_ok`).
    pub capabilities: Capabilities,
    /// The signed-in session.
    pub session: SessionAccount,
    _requirement: PhantomData<R>,
}

impl<R> Staff<R> {
    /// The server.
    pub fn guild(&self) -> GuildId {
        self.principal.guild
    }
}

/// The `{guild_id}` of the request's path.
pub(crate) async fn path_guild(parts: &mut Parts) -> Result<GuildId, ApiError> {
    let params = RawPathParams::from_request_parts(parts, &())
        .await
        .map_err(|_| ApiError::not_found())?;
    let raw = params
        .iter()
        .find(|(name, _)| *name == "guild_id")
        .map(|(_, value)| value.to_owned())
        .ok_or_else(ApiError::not_found)?;
    snowflake(&raw, "guild_id")
}

/// Builds the principal for `account` in `guild`: the server must be
/// installed and the account a member of it, else `404 server_not_found`
/// (which doesn't say which).
pub(crate) async fn principal(
    state: &ApiState,
    guild: GuildId,
    account: quack_core::ids::AccountId,
) -> Result<Principal, ApiError> {
    {
        let mut conn = state.engine().db().conn().await?;
        let installed = quack_db::guilds::get(&mut conn, guild)
            .await?
            .is_some_and(|g| g.installed);
        if !installed {
            return Err(ApiError::api(ApiCode::ServerNotFound));
        }
    }
    let principal = state
        .engine()
        .principal(guild, account, Origin::Dashboard)
        .await
        .map_err(map::principal)?;
    if principal.member.is_none() {
        return Err(ApiError::api(ApiCode::ServerNotFound));
    }
    Ok(principal)
}

impl<R: Requirement> FromRequestParts<ApiState> for Staff<R> {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &ApiState) -> Result<Self, ApiError> {
        let session = SessionAccount::from_request_parts(parts, state).await?;
        let guild = path_guild(parts).await?;
        let principal = principal(state, guild, session.account).await?;
        let capabilities = {
            let mut conn = state.engine().db().conn().await?;
            principal.current_capabilities(&mut conn).await?
        };
        R::check(capabilities).map_err(|err| {
            ApiError::domain(
                &err,
                &ErrorFacts {
                    scene: Scene::Dashboard,
                    ..ErrorFacts::default()
                },
            )
        })?;
        Ok(Self {
            principal,
            capabilities,
            session,
            _requirement: PhantomData,
        })
    }
}
