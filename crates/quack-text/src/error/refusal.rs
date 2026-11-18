//! Member refusals when applying a rule (§12.3 `refusal.*`).

use quack_core::action::ActionKind;
use quack_core::permissions::{DiscordPermission, MemberRefusal};

use crate::action::permission;
use crate::model::{Person, Text};

/// `{action_verb}`: "time out", "kick", or "ban". Undo kinds never reach a
/// refusal; they read as their original.
const fn action_verb(kind: ActionKind) -> &'static str {
    match kind {
        ActionKind::Timeout | ActionKind::RemoveTimeout => "time out",
        ActionKind::Kick => "kick",
        ActionKind::Ban | ActionKind::Unban => "ban",
    }
}

/// The action a missing permission was needed for.
const fn kind_for(p: DiscordPermission) -> ActionKind {
    match p {
        DiscordPermission::ModerateMembers => ActionKind::Timeout,
        DiscordPermission::KickMembers => ActionKind::Kick,
        DiscordPermission::BanMembers => ActionKind::Ban,
    }
}

/// The refusal without its first sentence ("No case was added."), for the
/// log channel's honeypot failures and the API's field messages.
/// `kind` is the level's action, when one was chosen.
/// Without a member (an API error raised before the member was known) the
/// sentences say "the member".
pub fn refusal_reason(r: MemberRefusal, member: Option<&Person>, kind: Option<ActionKind>) -> Text {
    let t = Text::new();
    let who = |t: Text, start: bool| match member {
        Some(m) => t.person(m),
        None => t.s(if start { "The member" } else { "the member" }),
    };
    match r {
        MemberRefusal::QuackNotInServer => t.s("Quack isn't in this server."),
        MemberRefusal::ActorNotInServer => t.s("You're not in this server anymore."),
        MemberRefusal::MemberNotInServer => who(t, true)
            .s(" isn't in this server. Choose a current member."),
        MemberRefusal::MemberIsSelf => {
            t.s("You can't add a case about yourself. Ask another moderator.")
        }
        MemberRefusal::MemberIsBot => who(t, true).s(" is a bot, and cases are only for people."),
        MemberRefusal::MemberIsOwner => who(t, true).s(" owns the server."),
        MemberRefusal::MemberRoleTooHigh => who(t, true)
            .s("'s highest role is at or above yours. Ask a moderator with a higher role."),
        MemberRefusal::QuackRoleTooLow => who(t, true).s(
            "'s highest role is at or above Quack's. Ask an administrator to move Quack's role above it.",
        ),
        MemberRefusal::QuackMissingPermission(p) => {
            who(t.s(format!("This rule would {} ", action_verb(kind.unwrap_or(kind_for(p))))), false)
                .s(format!(
                    ", but Quack doesn't have the {} permission. Ask an administrator to give it to Quack's role.",
                    permission(p.permission())
                ))
        }
        MemberRefusal::MemberIsAdministrator => who(t.s("This rule would time out "), false)
            .s(", but Discord doesn't allow timeouts for members with the Administrator permission."),
    }
}

/// `refusal.*`: why no case was added.
pub fn refusal(r: MemberRefusal, member: Option<&Person>, kind: Option<ActionKind>) -> Text {
    Text::fixed("No case was added.").sentence(refusal_reason(r, member, kind))
}
