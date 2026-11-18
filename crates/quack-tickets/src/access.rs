//! **Restore private access** (§13.2.4, component `tf`): reopens an
//! auto-archived thread, adds the member back, and removes everyone who
//! may not read the ticket. Not logged.

use quack_core::DomainError;
use quack_core::ids::{ChannelId, MemberId, TicketId};
use quack_core::modules::TicketState;
use quack_core::permissions::{ActorFacts, Capability, is_staff_for_access};
use quack_db::tickets;
use quack_engine::port::{Fresh, Membership, Outcome};
use quack_engine::{Engine, Principal};
use quack_text::Text;
use quack_text::model::Person;

use crate::TicketError;
use crate::open::channel;
use quack_engine::span;

/// How fixing access went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixAnswer {
    /// The member can open the thread; Quack removed this many people.
    Done {
        /// The ticket's member.
        member: MemberId,
        /// People removed from the thread.
        removed: u64,
    },
    /// Discord refused a change in the thread under this channel.
    Failed {
        /// The channel the thread is in.
        channel: ChannelId,
    },
}

impl FixAnswer {
    /// The ephemeral reply (`ticket.fix.*`).
    pub fn text(self) -> Text {
        match self {
            Self::Done { member, removed } => quack_text::ticket::fix_done(
                &Person {
                    id: member.account(),
                    name: None,
                },
                removed,
            ),
            Self::Failed { channel: c } => quack_text::ticket::fix_failed(&channel(c)),
        }
    }
}

/// Restores private access to an `open` ticket's thread (`moderate` + 2FA).
pub async fn fix_access(
    engine: &Engine,
    principal: &Principal,
    ticket: TicketId,
) -> Result<FixAnswer, TicketError> {
    let guild = principal.guild;
    span("fix_ticket_access", guild, async {
        let mut conn = engine.db().conn().await?;
        principal.require(&mut conn, Capability::Moderate).await??;
        let t = tickets::get(&mut conn, guild, ticket)
            .await?
            .ok_or(TicketError::NotFound)?;
        let roles = quack_db::settings::get(&mut conn, guild)
            .await?
            .map_or_default(|s| s.moderator_role_ids);
        drop(conn);
        let (TicketState::Open, Some(thread)) = (t.state, t.thread) else {
            return Err(DomainError::TicketNotOpen.into());
        };
        // An imported ticket may not know its entry channel; the thread
        // then names where the permission is needed.
        let failed = FixAnswer::Failed {
            channel: t.entry_channel.unwrap_or(thread),
        };
        let discord = engine.discord();
        let Outcome::Done(facts) = discord.guild_facts(guild).await else {
            return Err(TicketError::DiscordUnavailable);
        };
        for outcome in [
            discord.set_thread_state(thread, false, false).await,
            discord.add_thread_member(thread, t.member).await,
        ] {
            match outcome {
                Outcome::Done(()) => {}
                Outcome::Refused(_) => return Ok(failed),
                _ => return Err(TicketError::DiscordUnavailable),
            }
        }
        let members = match discord.thread_members(thread).await {
            Outcome::Done(m) => m,
            Outcome::Refused(_) => return Ok(failed),
            _ => return Err(TicketError::DiscordUnavailable),
        };
        let quack = facts.quack.as_ref().map(|q| q.id);
        let mut removed = 0;
        for m in members {
            if m == t.member || Some(m) == quack || m.account() == facts.owner_id {
                continue;
            }
            let staff = match discord.member(guild, m, Fresh::Cached).await {
                Outcome::Done(Membership::Present(f)) => is_staff_for_access(
                    &ActorFacts {
                        is_owner: false,
                        permissions: f.permissions,
                        role_ids: f.role_ids,
                        mfa_enabled: false,
                    },
                    &roles,
                ),
                Outcome::Done(Membership::Absent) => false,
                _ => return Err(TicketError::DiscordUnavailable),
            };
            if staff {
                continue;
            }
            match discord.remove_thread_member(thread, m).await {
                Outcome::Done(()) => removed += 1,
                Outcome::Refused(_) => return Ok(failed),
                _ => return Err(TicketError::DiscordUnavailable),
            }
        }
        Ok(FixAnswer::Done {
            member: t.member,
            removed,
        })
    })
    .await
}
