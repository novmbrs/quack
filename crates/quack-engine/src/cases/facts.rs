//! Gathering the Discord facts the member checks take (§2.3, §4.2 step 5).

use quack_core::ids::{GuildId, MemberId};
use quack_core::permissions::{ActorPresence, ApplyFacts, MemberSubject};
use time::OffsetDateTime;

use crate::port::{Discord, Fresh, Membership, Outcome, RefusalKind};

/// The facts one application of a rule was checked against.
#[derive(Clone, Debug)]
pub(crate) struct GatheredFacts {
    /// Owner, Quack, actor, member.
    pub apply: ApplyFacts,
    /// The member's `joined_at`, when present.
    pub joined_at: Option<OffsetDateTime>,
}

/// Why facts could not be gathered.
pub(crate) struct Unavailable;

/// Gathers `ApplyFacts` from the port. Quack not being in the server is a
/// fact (check 1), not an error.
pub(crate) async fn gather(
    discord: &dyn Discord,
    guild: GuildId,
    member: MemberId,
    actor: ActorPresence,
) -> Result<GatheredFacts, Unavailable> {
    let facts = match discord.guild_facts(guild).await {
        Outcome::Done(facts) => Some(facts),
        Outcome::Refused(r)
            if matches!(
                r.kind,
                RefusalKind::UnknownGuild | RefusalKind::MissingAccess
            ) =>
        {
            None
        }
        _ => return Err(Unavailable),
    };
    let Some(facts) = facts else {
        return Ok(GatheredFacts {
            apply: ApplyFacts {
                owner_id: member.account(),
                quack: None,
                actor,
                member: MemberSubject {
                    id: member,
                    bot: false,
                    present: None,
                },
            },
            joined_at: None,
        });
    };
    let (subject, joined_at) = match discord.member(guild, member, Fresh::Cached).await {
        Outcome::Done(Membership::Present(m)) => (m.subject(), m.joined_at),
        Outcome::Done(Membership::Absent) => (absent(discord, member).await?, None),
        _ => return Err(Unavailable),
    };
    Ok(GatheredFacts {
        apply: ApplyFacts {
            owner_id: facts.owner_id,
            quack: facts
                .quack
                .as_ref()
                .map(quack_core::permissions::MemberFacts::present),
            actor,
            member: subject,
        },
        joined_at,
    })
}

/// The account facts of a member who is not in the server (only the bot
/// flag matters to the checks).
async fn absent(discord: &dyn Discord, member: MemberId) -> Result<MemberSubject, Unavailable> {
    let ids = [member.account()];
    match discord.accounts(&ids).await {
        Outcome::Done(accounts) => Ok(accounts.first().map_or(
            MemberSubject {
                id: member,
                bot: false,
                present: None,
            },
            quack_core::permissions::AccountFacts::absent_subject,
        )),
        _ => Err(Unavailable),
    }
}
