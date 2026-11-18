//! Member refusals when applying a rule (§12.3 `refusal.*`).

use quack_core::action::ActionKind;
use quack_core::permissions::{DiscordPermission, MemberRefusal};

use crate::access::{self, Reader};
use crate::model::{Actor, Command, Person, Text};

/// The action a missing permission was needed for.
const fn kind_for(p: DiscordPermission) -> ActionKind {
    match p {
        DiscordPermission::ModerateMembers => ActionKind::Timeout,
        DiscordPermission::KickMembers => ActionKind::Kick,
        DiscordPermission::BanMembers => ActionKind::Ban,
    }
}

/// "This rule calls for a ban."
const fn calls_for(kind: ActionKind) -> &'static str {
    match kind {
        ActionKind::Timeout | ActionKind::RemoveTimeout => "This rule calls for a timeout.",
        ActionKind::Kick => "This rule calls for a kick.",
        ActionKind::Ban | ActionKind::Unban => "This rule calls for a ban.",
    }
}

/// Who a refusal is written to: the moderator who tried ("you"), or a log
/// reader, who is told who tried.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Voice<'a> {
    /// The moderator who tried.
    You,
    /// A log reader; the actor tried.
    Log(&'a Actor),
}

/// The refusal without its first sentence ("No case was added."), for the
/// API's field messages. `kind` is the level's action, when one was
/// chosen. Without a member (an API error raised before the member was
/// known) the sentences say "the member".
pub fn refusal_reason(r: MemberRefusal, member: Option<&Person>, kind: Option<ActionKind>) -> Text {
    refusal_in(r, member, kind, Voice::You)
}

pub(crate) fn refusal_in(
    r: MemberRefusal,
    member: Option<&Person>,
    kind: Option<ActionKind>,
    voice: Voice<'_>,
) -> Text {
    let t = Text::new();
    let who = |t: Text, start: bool| match member {
        Some(m) => t.person(m),
        None => t.s(if start { "The member" } else { "the member" }),
    };
    // A refusal about the actor: Quack never acts as itself on these, so a
    // honeypot entry points staff at the post instead.
    let quack_fallback = || {
        Text::fixed("Review the post and use ")
            .command(Command::CaseAdd)
            .s(" if a case is needed.")
    };
    match (r, voice) {
        (MemberRefusal::QuackNotInServer, _)
        | (MemberRefusal::ActorNotInServer, Voice::Log(Actor::Quack)) => {
            access::quack_not_in_server()
        }
        (MemberRefusal::ActorNotInServer, Voice::You) => t.s("You're not in this server anymore."),
        (MemberRefusal::ActorNotInServer, Voice::Log(a @ Actor::Person(_))) => {
            t.actor(a).s(" isn't in this server anymore.")
        }
        (MemberRefusal::MemberNotInServer, Voice::You) => {
            who(t, true).s(" isn't in this server. Choose a current member.")
        }
        (MemberRefusal::MemberNotInServer, Voice::Log(_)) => {
            who(t, true).s(" isn't in the server anymore.")
        }
        (MemberRefusal::MemberIsSelf, Voice::You) => {
            t.s("You can't add a case about yourself. Ask another moderator.")
        }
        (MemberRefusal::MemberIsSelf, Voice::Log(a @ Actor::Person(_))) => {
            t.actor(a).s(" can't add a case about themselves.")
        }
        (
            MemberRefusal::MemberIsSelf | MemberRefusal::MemberRoleTooHigh,
            Voice::Log(Actor::Quack),
        ) => quack_fallback(),
        (MemberRefusal::MemberIsBot, _) => {
            who(t, true).s(" is a bot, and cases are only for people.")
        }
        (MemberRefusal::MemberIsOwner, _) => {
            who(t, true).s(" owns this server. Quack doesn't add cases about the server owner.")
        }
        (MemberRefusal::MemberRoleTooHigh, Voice::You) => access::your_role_too_low(member),
        (MemberRefusal::MemberRoleTooHigh, Voice::Log(a @ Actor::Person(_))) => {
            let t = t.actor(a).s("'s highest role must be above ");
            who(t, false).s("'s highest role.")
        }
        (MemberRefusal::QuackRoleTooLow, _) => access::quack_role_too_low(member),
        (MemberRefusal::QuackMissingPermission(p), _) => {
            Text::fixed(calls_for(kind.unwrap_or(kind_for(p))))
                .sentence(access::quack_role_needs(&[p.permission()], Reader::Staff))
        }
        (MemberRefusal::MemberIsAdministrator, _) => Text::fixed(
            "This rule calls for a timeout, and Discord doesn't allow timeouts for members with Administrator.",
        ),
    }
}

/// `refusal.*`: why no case was added.
pub fn refusal(r: MemberRefusal, member: Option<&Person>, kind: Option<ActionKind>) -> Text {
    Text::fixed("No case was added.").sentence(refusal_reason(r, member, kind))
}
