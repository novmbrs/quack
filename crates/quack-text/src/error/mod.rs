//! Errors and refusals (§12.3), shown ephemerally in Discord and used as
//! the API's `message` for the matching error code (§14.6).

mod domain;
mod refusal;

use quack_core::action::ActionKind;
use quack_core::permissions::Capability;

pub use domain::{ErrorFacts, Scene, for_domain};
pub use refusal::{refusal, refusal_reason};

use crate::model::{Command, Person, Text};
use crate::word;

/// `error.missing_capability.*`.
pub fn missing_capability(c: Capability) -> Text {
    Text::fixed(match c {
        Capability::Moderate => {
            "Only moderators can do this. Ask a server administrator to give you a moderator role."
        }
        Capability::ManageRules => {
            "You need the Manage Server permission or a rules manager role to manage rules."
        }
        Capability::ManageSettings => {
            "You need the Manage Server permission to change Quack's settings."
        }
        Capability::ManageStaffRoles => {
            "Only the server owner and administrators can change the moderator roles."
        }
    })
}

/// `error.mfa_required`.
pub fn mfa_required() -> Text {
    Text::fixed(
        "This server requires two-factor authentication for moderation. Turn on 2FA in your Discord settings, then sign in to the Quack dashboard once so Quack can confirm it.",
    )
}

/// `error.actor_not_in_server`.
pub fn actor_not_in_server() -> Text {
    Text::fixed("You're not in this server anymore.")
}

/// `error.quack_not_in_server`.
pub fn quack_not_in_server() -> Text {
    Text::fixed("Quack isn't in this server. Ask an administrator to add Quack again.")
}

/// `error.discord_unavailable` (also `api.discord_unavailable`).
pub fn discord_unavailable() -> Text {
    Text::fixed("Quack couldn't reach Discord, so nothing changed. Try again in a moment.")
}

/// `error.internal`.
pub fn internal() -> Text {
    Text::fixed("Something went wrong inside Quack, so nothing was saved. Try again in a moment.")
}

/// `error.unconfirmed`.
pub fn unconfirmed() -> Text {
    Text::fixed("Quack couldn't confirm whether that was saved. Check before you try again.")
}

/// `error.render_failed`.
pub fn render_failed(number: u64) -> Text {
    Text::fixed(format!(
        "Case #{number} is saved, but Quack couldn't show it here. Use "
    ))
    .command(Command::CaseView)
    .s(format!(" number:{number} to see it."))
}

/// `error.case_not_found`.
pub fn case_not_found(number: u64) -> Text {
    Text::fixed(format!("There's no case #{number} in this server."))
}

/// `error.action_not_found`.
pub fn action_not_found() -> Text {
    Text::fixed(
        "Quack couldn't find that action in this server. Open the case again to see its actions.",
    )
}

/// `error.context_changed`.
pub fn context_changed() -> Text {
    Text::fixed("Someone else changed this case's context. Open it again before you save.")
}

/// `error.rule_changed`: the rule changed while a case was being added.
pub fn rule_changed(rule: &str) -> Text {
    Text::fixed("The rule ")
        .bold_user(rule)
        .s(" changed while you were adding the case. Open the form again to use the new version.")
}

/// `error.rule_not_found`.
pub fn rule_not_found() -> Text {
    Text::fixed("That rule doesn't exist anymore. Choose another rule.")
}

/// `error.rule_archived`.
pub fn rule_archived(rule: &str) -> Text {
    Text::fixed("The rule ")
        .bold_user(rule)
        .s(" is archived, so it can't be applied. Choose an active rule.")
}

/// `error.case_already_voided`.
pub fn case_already_voided(number: u64) -> Text {
    Text::fixed(format!("Case #{number} is already voided."))
}

/// `error.case_voided`: a retry of an original action on a voided case.
pub fn case_voided(number: u64, kind: ActionKind) -> Text {
    Text::fixed(format!(
        "Case #{number} is voided, so Quack won't run its {}. Add a new case if it still applies.",
        word::action_noun(kind)
    ))
}

/// `error.action_not_retryable`.
pub fn action_not_retryable(kind: ActionKind) -> Text {
    Text::fixed(format!(
        "This {} can't be retried: it isn't failed anymore, or it was dismissed. Open the case to see where it stands.",
        word::action_noun(kind)
    ))
}

/// `error.action_not_dismissable`.
pub fn action_not_dismissable(kind: ActionKind) -> Text {
    Text::fixed(format!(
        "This {} isn't failed anymore, or it was already dismissed, so there's nothing to dismiss.",
        word::action_noun(kind)
    ))
}

/// `error.action_not_undoable`.
pub fn action_not_undoable(kind: ActionKind) -> Text {
    Text::fixed(format!(
        "There's nothing to undo: the {} isn't in place, or Quack is already undoing it. Open the case to see where it stands.",
        word::action_noun(kind)
    ))
}

/// `error.timeout_window_passed`.
pub fn timeout_window_passed(member: &Person) -> Text {
    Text::fixed("The timeout's end time has passed. Add a new case if ")
        .person(member)
        .s(" still needs a timeout.")
}

/// `error.appeal_not_found`.
pub fn appeal_not_found() -> Text {
    Text::fixed("That appeal doesn't exist in this server.")
}

/// `error.appeal_changed`.
pub fn appeal_changed() -> Text {
    Text::fixed("This appeal changed since you opened it. Open it again to see where it stands.")
}

/// `error.evidence_limit_reached`.
pub fn evidence_limit_reached(number: u64) -> Text {
    Text::fixed(format!(
        "Case #{number} would have more than 50 evidence items. Add fewer message links."
    ))
}

/// `error.too_many_links`.
pub fn too_many_links() -> Text {
    Text::fixed("Add at most 10 message links at a time.")
}

/// `error.replacement_invalid`.
pub fn replacement_invalid(number: u64) -> Text {
    Text::fixed(format!(
        "Case #{number} can't be replaced: only a voided case in this server that has no replacement yet can be."
    ))
}

/// `error.case_numbers_exhausted`: the server has used every case number
/// Quack can store (§4.4).
pub fn case_numbers_exhausted() -> Text {
    Text::fixed(
        "No case was added. This server has reached the highest case number Quack can store.",
    )
}

/// `error.honeypot_policy_changed`.
pub fn honeypot_policy_changed() -> Text {
    Text::fixed("The honeypot settings changed, so Quack didn't add a case for this post.")
}

/// `interaction.busy`.
pub fn busy() -> Text {
    Text::fixed("Quack is busy right now. Try again in a moment.")
}

/// `interaction.expired_button`.
pub fn expired_button() -> Text {
    Text::fixed("This button has expired. Run the command again.")
}

/// `interaction.legacy_button`.
pub fn legacy_button() -> Text {
    Text::fixed(
        "This button is from an older version of Quack and no longer works. Use the commands or the dashboard instead.",
    )
}
