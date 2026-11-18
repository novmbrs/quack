//! Errors and refusals (§12.3), shown ephemerally in Discord and used as
//! the API's `message` for the matching error code (§14.6).

mod domain;
mod refusal;

use quack_core::action::ActionKind;
use quack_core::permissions::Capability;

pub use domain::{ErrorFacts, Scene, for_domain};
pub(crate) use refusal::{Voice, refusal_in};
pub use refusal::{refusal, refusal_reason};

use crate::fmt;
use crate::model::{Command, Person, Text};
use crate::word;

/// `error.missing_capability.*`.
pub fn missing_capability(c: Capability) -> Text {
    Text::fixed(match c {
        Capability::Moderate => {
            "Only moderators can do this. Ask a server administrator to check the moderator roles in the Quack dashboard's server settings."
        }
        Capability::ManageRules => {
            "To manage rules, you need Manage Server in Discord or a rules manager role in Quack. Ask an administrator to check your roles in Server Settings → Roles, or the rules manager roles in the Quack dashboard's server settings."
        }
        Capability::ManageSettings => {
            "To change Quack's settings, you need Manage Server in Discord. Ask an administrator to check your roles in Server Settings → Roles."
        }
        Capability::ManageStaffRoles => {
            "Only the server owner or a member with Administrator can change Quack's moderator roles. Ask one of them to change them in the Quack dashboard's server settings."
        }
    })
}

/// `error.mfa_required`.
pub fn mfa_required() -> Text {
    Text::fixed(
        "This server requires two-factor authentication for moderation. In Discord, open User Settings → My Account and turn on two-factor authentication. Then sign in to the Quack dashboard with Discord and try again.",
    )
}

/// `error.actor_not_in_server`.
pub fn actor_not_in_server() -> Text {
    Text::fixed("You're not in this server anymore.")
}

/// `error.quack_not_in_server`.
pub fn quack_not_in_server() -> Text {
    crate::access::quack_not_in_server()
}

/// `error.discord_unavailable` (also `api.discord_unavailable`).
pub fn discord_unavailable() -> Text {
    Text::fixed("Quack couldn't reach Discord, so nothing changed. Try again in a moment.")
}

/// `error.internal`.
pub fn internal() -> Text {
    Text::fixed("Something went wrong inside Quack, so nothing changed. Try again in a moment.")
}

/// `error.unconfirmed`.
pub fn unconfirmed() -> Text {
    Text::fixed(
        "Quack couldn't confirm whether that was saved. Open it again to check before you try again.",
    )
}

/// `error.render_failed`.
pub fn render_failed(number: u64) -> Text {
    Text::fixed(format!(
        "Case #{number} is saved. Quack couldn't show it here. Run "
    ))
    .command(Command::CaseView)
    .s(format!(" and enter {number} as the number."))
}

/// `error.case_not_found`.
pub fn case_not_found(number: u64) -> Text {
    Text::fixed(format!(
        "Case #{number} wasn't found in this server. Check the number or use "
    ))
    .command(Command::CaseList)
    .s(".")
}

/// `error.action_not_found`.
pub fn action_not_found() -> Text {
    Text::fixed(
        "Quack couldn't find that action in this server. Open the case again to see its actions.",
    )
}

/// `error.context_changed`.
pub fn context_changed() -> Text {
    Text::fixed(
        "Someone else changed this case's context. Open it again to see their changes before you save.",
    )
}

/// `error.rule_changed`: the rule changed while a case was being added.
pub fn rule_changed(rule: &str) -> Text {
    Text::fixed("The rule ")
        .bold_user(rule)
        .s(" changed while you were adding the case. Run ")
        .command(Command::CaseAdd)
        .s(" again to use the new version.")
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
    Text::fixed(format!(
        "Case #{number} is already voided and no longer counts."
    ))
}

/// `error.case_voided`: a retry of an original action on a voided case.
pub fn case_voided(number: u64, kind: ActionKind) -> Text {
    match kind {
        ActionKind::Timeout | ActionKind::Kick | ActionKind::Ban => Text::fixed(format!(
            "Case #{number} is voided. Quack won't retry its {}.",
            word::action(kind)
        )),
        ActionKind::RemoveTimeout | ActionKind::Unban => Text::fixed(format!(
            "Case #{number} is voided. Quack won't retry this action."
        )),
    }
}

/// `error.action_not_retryable`.
pub fn action_not_retryable(kind: ActionKind) -> Text {
    Text::fixed(format!(
        "{} can no longer be retried. Open the case to see its current outcome.",
        fmt::upper_first(crate::action::retry_noun(kind))
    ))
}

/// `error.action_not_dismissable`.
pub fn action_not_dismissable(kind: ActionKind) -> Text {
    Text::fixed(format!(
        "{} can no longer be dismissed. Open the case to see its current outcome.",
        fmt::upper_first(&crate::action::failure_noun(kind))
    ))
}

/// `error.action_not_undoable`.
pub fn action_not_undoable(kind: ActionKind) -> Text {
    Text::fixed(match kind {
        ActionKind::Kick => "A kick can't be undone. Open the case to see what happened.",
        ActionKind::Timeout => {
            "This timeout can't be undone from the case now. Open the case to see its current outcome."
        }
        ActionKind::Ban => {
            "This ban can't be undone from the case now. Open the case to see its current outcome."
        }
        ActionKind::RemoveTimeout | ActionKind::Unban => {
            "This action can't be undone from the case. Open the case to see its current outcome."
        }
    })
}

/// `error.timeout_window_passed`.
pub fn timeout_window_passed(member: &Person) -> Text {
    Text::fixed("The timeout's end time has passed, so Quack won't retry it. Check ")
        .person(member)
        .s("'s case history before adding another case.")
}

/// `error.case_not_found_dashboard`: [`case_not_found`] on the dashboard,
/// which has no slash commands.
pub fn case_not_found_dashboard(number: u64) -> Text {
    Text::fixed(format!(
        "Case #{number} wasn't found in this server. Check the number or find the case in the case list."
    ))
}

/// `error.appeal_not_found_dashboard`: [`appeal_not_found`] on the
/// dashboard.
pub fn appeal_not_found_dashboard() -> Text {
    Text::fixed("Quack couldn't find this appeal. Open the case, or go back to the appeals list.")
}

/// `error.reason_empty`: a void or appeal decision without a reason.
pub fn reason_empty() -> Text {
    Text::fixed("Add a reason before you save. The member can see it.")
}

/// `error.reason_too_long`: a void or appeal decision reason over `max`
/// characters.
pub fn reason_too_long(max: usize) -> Text {
    Text::fixed(format!(
        "This reason is too long. Shorten it to {} characters or fewer.",
        fmt::count(u64::try_from(max).unwrap_or(u64::MAX))
    ))
}

/// `error.appeal_text_empty`: an appeal or added information with no text.
pub fn appeal_text_empty() -> Text {
    Text::fixed("Write what you want the moderators to know before you send it.")
}

/// `error.appeal_text_too_long`: an appeal or added information over `max`
/// characters.
pub fn appeal_text_too_long(max: usize) -> Text {
    Text::fixed(format!(
        "Your message is too long. Shorten it to {} characters or fewer.",
        fmt::count(u64::try_from(max).unwrap_or(u64::MAX))
    ))
}

/// `error.no_links`: adding evidence without a message link.
pub fn no_links() -> Text {
    Text::fixed("No evidence was added. Paste at least one message link and try again.")
}

/// `error.appeal_not_found`.
pub fn appeal_not_found() -> Text {
    Text::fixed("Quack couldn't find this appeal. Open the case, or use ")
        .command(Command::Appeals)
        .s(" to see appeals waiting for a decision.")
}

/// `error.appeal_changed`.
pub fn appeal_changed() -> Text {
    Text::fixed("This appeal changed since you opened it. Open it again to see the latest update.")
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
        "Case #{number} can't be replaced. Choose a voided case in this server that doesn't already have a replacement."
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
    Text::fixed("This button no longer works. Run the command again, or use the dashboard.")
}

/// `interaction.legacy_button`.
pub fn legacy_button() -> Text {
    Text::fixed(
        "This button is from an older version of Quack and no longer works. Run the command again, or use the dashboard.",
    )
}
