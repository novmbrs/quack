//! Member-facing action sentences (§12.6, "you"), for the member's own case
//! page on the dashboard. They never name staff or failure details.

use quack_core::action::{ActionKind, ActionSkipReason, ActionState};

use super::ActionView;
use crate::model::{Text, TimeStyle};

/// `member.action.none`.
pub fn member_none() -> Text {
    Text::fixed("This case is a warning. No other action was taken.")
}

fn not_applied(kind: ActionKind) -> Text {
    Text::fixed(match kind {
        ActionKind::Timeout => "The timeout wasn't applied.",
        ActionKind::Kick => "You weren't removed from the server.",
        ActionKind::Ban => "The ban wasn't applied.",
        ActionKind::RemoveTimeout => {
            "Your timeout hasn't been removed yet. The moderators can see why."
        }
        ActionKind::Unban => "Your ban hasn't been removed yet. The moderators can see why.",
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
    match a.state {
        ActionState::Pending | ActionState::Running => match k {
            ActionKind::Timeout => until(a, "Quack is timing you out"),
            ActionKind::Kick => Text::fixed("Quack is removing you from the server."),
            ActionKind::Ban => Text::fixed("Quack is banning you from the server."),
            ActionKind::RemoveTimeout => Text::fixed("Quack is removing your timeout."),
            ActionKind::Unban => Text::fixed("Quack is unbanning you."),
        },
        ActionState::Done => match (k, a.no_change) {
            (ActionKind::Timeout, _) => until(a, "You're timed out"),
            // They had already left, so this case didn't remove them.
            (ActionKind::Kick, true) => not_applied(k),
            (ActionKind::Kick, false) => Text::fixed("You were removed from the server."),
            (ActionKind::Ban, _) => Text::fixed("You're banned from the server."),
            (ActionKind::RemoveTimeout, true) => Text::fixed("Your timeout had already ended."),
            (ActionKind::RemoveTimeout, false) => Text::fixed("Your timeout was removed."),
            (ActionKind::Unban, _) => Text::fixed("You're no longer banned."),
        },
        ActionState::Failed if a.unconfirmed() => {
            Text::fixed("Quack couldn't confirm whether Discord applied this action.")
        }
        ActionState::Failed | ActionState::Cancelled => not_applied(k),
        ActionState::Skipped => match (k, a.skip) {
            (ActionKind::RemoveTimeout, Some(ActionSkipReason::ChangedOutsideCase)) => {
                Text::fixed("Your timeout stays. The moderators can see why.")
            }
            (ActionKind::RemoveTimeout, _) => {
                Text::fixed("Your timeout stays because another case still applies.")
            }
            (ActionKind::Unban, Some(ActionSkipReason::ChangedOutsideCase)) => {
                Text::fixed("Your ban stays. The moderators can see why.")
            }
            (ActionKind::Unban, _) => {
                Text::fixed("Your ban stays because another case still applies.")
            }
            // Originals are never skipped; this only keeps the match total.
            (ActionKind::Timeout | ActionKind::Kick | ActionKind::Ban, _) => not_applied(k),
        },
    }
}
