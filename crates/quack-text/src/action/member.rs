//! Member-facing action sentences (§12.6, "you"), for the member's own case
//! page on the dashboard. They never name staff or failure details.

use quack_core::action::{ActionKind, ActionSkipReason, ActionState};

use super::ActionView;
use crate::fmt;
use crate::model::{Text, TimeStyle};

/// `member.action.none`.
pub fn member_none() -> Text {
    Text::fixed("This case is a warning.")
}

/// "timed you out", "removed your timeout": what the action does to "you".
const fn to_you(kind: ActionKind, past: bool) -> &'static str {
    match (kind, past) {
        (ActionKind::Timeout, true) => "timed you out",
        (ActionKind::Timeout, false) => "time you out",
        (ActionKind::Kick, true) => "removed you from the server",
        (ActionKind::Kick, false) => "remove you from the server",
        (ActionKind::Ban, true) => "banned you",
        (ActionKind::Ban, false) => "ban you",
        (ActionKind::RemoveTimeout, true) => "removed your timeout",
        (ActionKind::RemoveTimeout, false) => "remove your timeout",
        (ActionKind::Unban, true) => "unbanned you",
        (ActionKind::Unban, false) => "unban you",
    }
}

fn not_applied(kind: ActionKind) -> Text {
    Text::fixed(match kind {
        ActionKind::Timeout => "Quack didn't time you out.",
        ActionKind::Kick => "Quack didn't remove you from the server.",
        ActionKind::Ban => "Quack didn't ban you.",
        ActionKind::RemoveTimeout => "Your timeout is still in place. The moderators can see why.",
        ActionKind::Unban => "Your ban is still in place. The moderators can see why.",
    })
}

fn until(a: &ActionView, s: &str) -> Text {
    let t = Text::fixed(s);
    match a.timeout_ends_at {
        Some(at) => t.s(" until ").time(at, TimeStyle::FullRelative).s("."),
        None => t.s("."),
    }
}

/// The member's sentence for one action (`member.action.*`).
pub fn member_sentence(a: &ActionView) -> Text {
    let k = a.kind;
    if a.uncertain() {
        let t = Text::fixed(format!(
            "Quack couldn't confirm whether Discord {}.",
            to_you(k, true)
        ));
        return match (a.state, k) {
            (ActionState::Cancelled, ActionKind::Timeout) => {
                t.s(" The case was voided, so Quack will check whether the timeout can be removed.")
            }
            (ActionState::Cancelled, ActionKind::Ban) => {
                t.s(" The case was voided, so Quack will check whether the ban can be removed.")
            }
            (ActionState::Cancelled, ActionKind::Kick) => {
                t.s(" The case was voided. A kick can't be undone.")
            }
            _ => t,
        };
    }
    match a.state {
        ActionState::Pending => match k {
            ActionKind::Timeout => until(a, "Quack will time you out"),
            _ => Text::fixed(format!("Quack will {}.", to_you(k, false))),
        },
        ActionState::Running => match k {
            ActionKind::Timeout => until(a, "Quack is timing you out"),
            ActionKind::Kick => Text::fixed("Quack is removing you from the server."),
            ActionKind::Ban => Text::fixed("Quack is banning you."),
            ActionKind::RemoveTimeout => Text::fixed("Quack is removing your timeout."),
            ActionKind::Unban => Text::fixed("Quack is unbanning you."),
        },
        ActionState::Done => match (k, a.no_change) {
            (ActionKind::Timeout, false) => match a.timeout_ends_at {
                Some(at) => Text::fixed("Quack set your timeout to end ")
                    .time(at, TimeStyle::FullRelative)
                    .s("."),
                None => Text::fixed("Quack timed you out."),
            },
            (ActionKind::Timeout, true) => {
                Text::fixed("You already had a timeout. Quack left it in place.")
            }
            (ActionKind::Kick, true) => Text::fixed(
                "You left the server after this case was added, so Quack didn't remove you.",
            ),
            (ActionKind::Kick, false) => Text::fixed("Quack removed you from the server."),
            (ActionKind::Ban, true) => Text::fixed("You were already banned when Quack checked."),
            (ActionKind::Ban, false) => match a.ban_delete_message_seconds {
                Some(secs) if secs > 0 => Text::fixed(format!(
                    "Quack banned you and deleted your messages from the last {}.",
                    fmt::last(u64::from(secs))
                )),
                _ => Text::fixed("Quack banned you."),
            },
            (ActionKind::RemoveTimeout, true) => {
                Text::fixed("You had no active timeout when Quack checked.")
            }
            (ActionKind::RemoveTimeout, false) => Text::fixed("Quack removed your timeout."),
            (ActionKind::Unban, true) => Text::fixed("You weren't banned when Quack checked."),
            (ActionKind::Unban, false) => Text::fixed("Quack unbanned you."),
        },
        ActionState::Failed => not_applied(k),
        ActionState::Cancelled => Text::fixed(format!(
            "The case was voided before Quack {}.",
            to_you(k, true)
        )),
        ActionState::Skipped => match (k, a.skip) {
            (ActionKind::RemoveTimeout, Some(ActionSkipReason::ChangedOutsideCase)) => Text::fixed(
                "Quack left your timeout in place. Ask the moderators if you need help.",
            ),
            (ActionKind::RemoveTimeout, _) => {
                Text::fixed("Your timeout stays in place because another case still applies.")
            }
            (ActionKind::Unban, Some(ActionSkipReason::ChangedOutsideCase)) => {
                Text::fixed("Quack left your ban in place. Ask the moderators if you need help.")
            }
            (ActionKind::Unban, _) => {
                Text::fixed("Your ban stays in place because another case still applies.")
            }
            // Originals are never skipped; this only keeps the match total.
            (ActionKind::Timeout | ActionKind::Kick | ActionKind::Ban, _) => not_applied(k),
        },
    }
}
