//! Who is acting (§2): a staff principal built from current facts, and the
//! capability checks every staff operation runs.
//!
//! Capabilities are derived from Discord facts (owner, permissions, roles,
//! MFA level), Quack's record of the account's 2FA, and the server's staff
//! role lists. The role lists are read from `guild_settings` at the moment
//! of the check, and writes check again inside their transaction (§2), so
//! a role removed a moment ago stops working at once.

use quack_core::DomainError;
use quack_core::case::CaseSource;
use quack_core::ids::{AccountId, GuildId, RoleId};
use quack_core::modlog::ModLogSource;
use quack_core::permissions::{
    ActorFacts, ActorPresence, Capabilities, Capability, GuildFacts, MemberFacts, PresentMember,
    StaffRoles, derive_capabilities,
};
use quack_db::{DbError, PgConnection, accounts, settings};

use crate::Engine;
use crate::port::{Fresh, Membership, Outcome};

/// Where a staff operation came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// A Discord command, button, select, or modal.
    Discord,
    /// The dashboard.
    Dashboard,
}

impl Origin {
    /// The moderation log's `source` for this origin.
    pub const fn mod_log_source(self) -> ModLogSource {
        match self {
            Self::Discord => ModLogSource::Discord,
            Self::Dashboard => ModLogSource::Dashboard,
        }
    }

    /// The case's `source` for this origin.
    pub const fn case_source(self) -> CaseSource {
        match self {
            Self::Discord => CaseSource::Discord,
            Self::Dashboard => CaseSource::Dashboard,
        }
    }
}

/// A staff member acting in one server, with the facts their capabilities
/// are derived from. Build one per operation; never cache it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Principal {
    /// The server.
    pub guild: GuildId,
    /// The account acting.
    pub account: AccountId,
    /// Where the operation came from.
    pub origin: Origin,
    /// Their role facts in the server, or `None` when they are not in it.
    pub member: Option<PresentMember>,
    /// Owner, permissions, roles, and recorded 2FA.
    pub actor: ActorFacts,
    /// The server's current roles (configured roles it no longer has are
    /// ignored, §2.1 rule 1).
    pub guild_role_ids: Vec<RoleId>,
    /// Whether the server requires 2FA for moderation (§2.2).
    pub mfa_required: bool,
}

impl Principal {
    /// Builds a principal from facts the caller already holds (an
    /// interaction payload plus the guild cache), and the account's 2FA
    /// record.
    pub fn from_facts(
        guild: &GuildFacts,
        account: AccountId,
        origin: Origin,
        member: Option<&MemberFacts>,
        mfa_enabled: bool,
    ) -> Self {
        Self {
            guild: guild.guild_id,
            account,
            origin,
            member: member.map(MemberFacts::present),
            actor: ActorFacts {
                is_owner: guild.owner_id == account,
                permissions: member.map_or_default(|m| m.permissions),
                role_ids: member.map_or_default(|m| m.role_ids.clone()),
                mfa_enabled,
            },
            guild_role_ids: guild.role_ids(),
            mfa_required: guild.mfa_required,
        }
    }

    /// The capabilities these facts give under `roles`.
    pub fn capabilities(&self, roles: &StaffRoles) -> Capabilities {
        let roles = roles.clone().existing(&self.guild_role_ids);
        derive_capabilities(&self.actor, &roles, self.mfa_required)
    }

    /// Reads the server's staff roles and derives the capabilities now
    /// (route 6, `/setup` panels, buttons to show).
    pub async fn current_capabilities(
        &self,
        conn: &mut PgConnection,
    ) -> Result<Capabilities, DbError> {
        let roles = staff_roles(conn, self.guild).await?;
        Ok(self.capabilities(&roles))
    }

    /// Checks `cap` (and the 2FA rule) against the staff roles read now,
    /// on `conn`: pass the write's transaction so the check holds at commit.
    pub async fn require(
        &self,
        conn: &mut PgConnection,
        cap: Capability,
    ) -> Result<Result<Capabilities, DomainError>, DbError> {
        let caps = self.current_capabilities(conn).await?;
        Ok(caps.check(cap).map(|()| caps).map_err(DomainError::from))
    }

    /// Who acts, for member checks.
    pub const fn presence(&self) -> ActorPresence {
        ActorPresence::Staff {
            id: self.account,
            member: self.member,
        }
    }
}

/// The server's staff role lists (empty when the server has no settings).
pub(crate) async fn staff_roles(
    conn: &mut PgConnection,
    guild: GuildId,
) -> Result<StaffRoles, DbError> {
    Ok(settings::get(conn, guild)
        .await?
        .map_or_default(|s| StaffRoles {
            moderator_role_ids: s.moderator_role_ids,
            rules_manager_role_ids: s.rules_manager_role_ids,
        }))
}

/// Why a principal could not be built.
#[derive(Debug, thiserror::Error)]
pub enum PrincipalError {
    /// Discord didn't answer the facts the principal needs.
    #[error("Discord unavailable")]
    DiscordUnavailable,
    /// Quack is not in the server.
    #[error("Quack is not in the server")]
    QuackNotInServer,
    /// The database failed.
    #[error(transparent)]
    Db(#[from] DbError),
}

impl Engine {
    /// Builds a principal from current facts (§2): the server's facts and
    /// the actor's member from the `Discord` port (the member cache only
    /// under §2's freshness rules), and the account's 2FA record. The
    /// dashboard uses this; Discord interactions may build one from their
    /// payload with [`Principal::from_facts`].
    pub async fn principal(
        &self,
        guild: GuildId,
        account: AccountId,
        origin: Origin,
    ) -> Result<Principal, PrincipalError> {
        let facts = match self.discord().guild_facts(guild).await {
            Outcome::Done(facts) => facts,
            Outcome::Refused(_) => return Err(PrincipalError::QuackNotInServer),
            _ => return Err(PrincipalError::DiscordUnavailable),
        };
        let member = match self
            .discord()
            .member(guild, account.member(), Fresh::Cached)
            .await
        {
            Outcome::Done(Membership::Present(m)) => Some(m),
            Outcome::Done(Membership::Absent) => None,
            _ => return Err(PrincipalError::DiscordUnavailable),
        };
        let mut conn = self.db().conn().await?;
        let mfa = accounts::get(&mut conn, account)
            .await?
            .is_some_and(|a| a.mfa_enabled);
        Ok(Principal::from_facts(
            &facts,
            account,
            origin,
            member.as_ref(),
            mfa,
        ))
    }
}
