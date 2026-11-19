//! Who is clicking (§2): the staff principal built from the interaction
//! payload, and the payload-only capability check for answers that must be
//! immediate.
//!
//! Every handler builds the principal after the ack and lets the engine
//! check the capability (with 2FA) before any staff data is read or any
//! write; a custom id never carries authority.

use quack_core::DomainError;
use quack_core::ids::GuildId;
use quack_core::permissions::{
    ActorFacts, Capabilities, Capability, StaffRoles, derive_capabilities,
};
use quack_discord::interaction::{Interaction, Person};
use quack_discord::model::Member;
use quack_discord::permissions::member_facts;
use quack_engine::port::Outcome;
use quack_engine::{Engine, GuildConfig, Origin, Principal};
use quack_telemetry::labels::Reason;
use quack_text::error::ErrorFacts;
use time::OffsetDateTime;

use crate::ack::Failure;
use crate::errors;

/// The capabilities the payload alone shows (the invoker's roles and
/// permissions, the cached staff roles), ignoring ownership and 2FA. Only
/// for answers that must be immediate (autocomplete, opening a modal);
/// every write checks again from current facts.
pub(crate) fn payload_capabilities(ix: &Interaction, config: Option<&GuildConfig>) -> Capabilities {
    let Some(member) = ix.invoker.as_ref().and_then(|p| p.member.as_ref()) else {
        return Capabilities::default();
    };
    let actor = ActorFacts {
        is_owner: false,
        permissions: member.permissions.unwrap_or_default(),
        role_ids: member.role_ids.clone(),
        mfa_enabled: true,
    };
    let roles = config.map_or_else(StaffRoles::default, |c| StaffRoles {
        moderator_role_ids: c.settings.moderator_role_ids.clone(),
        rules_manager_role_ids: c.settings.rules_manager_role_ids.clone(),
    });
    derive_capabilities(&actor, &roles, false)
}

/// The refusal for a missing capability, from the catalog.
pub(crate) fn missing(cap: Capability) -> Failure {
    errors::domain(DomainError::MissingCapability(cap), &ErrorFacts::default())
}

/// The payload member as Discord's member model, for permission math.
fn as_member(person: &Person) -> Option<Member> {
    let m = person.member.as_ref()?;
    Some(Member {
        id: person.account.id.member(),
        bot: person.account.bot,
        username: person.account.username.clone(),
        global_name: person.account.global_name.clone(),
        avatar_hash: person.account.avatar_hash.clone(),
        nick: m.nick.clone(),
        role_ids: m.role_ids.clone(),
        joined_at: m.joined_at,
        timeout_until: m.timeout_until,
    })
}

/// The server an interaction happened in; staff interactions outside one
/// can't be answered with staff data.
pub(crate) fn guild_of(ix: &Interaction) -> Result<GuildId, Failure> {
    ix.guild_id.ok_or_else(|| missing(Capability::Moderate))
}

/// The invoker's principal (§2): the server's facts from the `Discord`
/// port (the guild cache while it is exact), the invoker's roles from the
/// payload, and Quack's record of their 2FA (read only when the server
/// requires it).
pub(crate) async fn principal(engine: &Engine, ix: &Interaction) -> Result<Principal, Failure> {
    let guild = guild_of(ix)?;
    let person = ix
        .invoker
        .as_ref()
        .ok_or_else(|| missing(Capability::Moderate))?;
    let facts = match engine.discord().guild_facts(guild).await {
        Outcome::Done(facts) => facts,
        Outcome::Refused(_) => {
            return Err(Failure::refused(
                quack_text::error::quack_not_in_server(),
                Reason::MissingPermission,
            ));
        }
        _ => return Err(errors::discord_unavailable()),
    };
    let member = as_member(person).map(|m| member_facts(&facts, &m, OffsetDateTime::now_utc()));
    let mfa_enabled = if facts.mfa_required {
        let mut conn = engine.db().conn().await.map_err(|e| errors::db(&e))?;
        quack_db::accounts::get(&mut conn, person.account.id)
            .await
            .map_err(|e| errors::db(&e))?
            .is_some_and(|a| a.mfa_enabled)
    } else {
        false
    };
    Ok(Principal::from_facts(
        &facts,
        person.account.id,
        Origin::Discord,
        member.as_ref(),
        mfa_enabled,
    ))
}

/// Checks `cap` (and the 2FA rule) for operations that read staff data
/// without an engine use case doing it (pickers, panels).
pub(crate) async fn require(
    engine: &Engine,
    principal: &Principal,
    cap: Capability,
) -> Result<Capabilities, Failure> {
    let mut conn = engine.db().conn().await.map_err(|e| errors::db(&e))?;
    match principal.require(&mut conn, cap).await {
        Ok(Ok(caps)) => Ok(caps),
        Ok(Err(refused)) => Err(errors::domain(refused, &ErrorFacts::default())),
        Err(e) => Err(errors::db(&e)),
    }
}
