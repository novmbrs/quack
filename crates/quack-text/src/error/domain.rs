//! Turning a `quack_core::DomainError` into its message (§14.6).
//!
//! A domain error carries no names or numbers, so the caller passes the
//! facts it has; a message whose facts are missing falls back to a
//! sentence that names "this case" or "that rule" instead.

use quack_core::DomainError;
use quack_core::action::ActionKind;

use crate::model::{Person, Text};
use crate::{api, appeal, ticket};

/// Where the error happened, when that changes the wording.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scene {
    /// A staff command or button in Discord.
    #[default]
    Staff,
    /// A staff page of the dashboard (no slash commands to point at).
    Dashboard,
    /// Adding a case (the form or the command).
    AddingCase,
    /// Editing a rule on the dashboard.
    EditingRule,
    /// A member's own case or appeal.
    Member,
}

/// What the caller knows about the thing the error is about.
#[derive(Clone, Copy, Debug, Default)]
pub struct ErrorFacts<'a> {
    /// Where the error happened.
    pub scene: Scene,
    /// The case's number.
    pub case_number: Option<u64>,
    /// The rule's name.
    pub rule: Option<&'a str>,
    /// The action's kind.
    pub action: Option<ActionKind>,
    /// The member the case is about.
    pub member: Option<&'a Person>,
}

fn case_or(f: &ErrorFacts<'_>, numbered: impl FnOnce(u64) -> Text, fallback: &str) -> Text {
    f.case_number
        .map_or_else(|| Text::fixed(fallback), numbered)
}

fn action_or(f: &ErrorFacts<'_>, with: impl FnOnce(ActionKind) -> Text, fallback: &str) -> Text {
    f.action.map_or_else(|| Text::fixed(fallback), with)
}

/// The message for a domain error.
#[allow(clippy::too_many_lines, reason = "one arm per domain error")]
pub fn for_domain(err: &DomainError, f: &ErrorFacts<'_>) -> Text {
    match err {
        DomainError::MissingCapability(c) => super::missing_capability(*c),
        DomainError::MfaRequired => super::mfa_required(),
        DomainError::MemberRefused(r) => super::refusal(*r, f.member, f.action),
        DomainError::CaseNotFound | DomainError::AppealNotFound if f.scene == Scene::Member => {
            api::not_found()
        }
        DomainError::CaseNotFound if f.scene == Scene::Dashboard => case_or(
            f,
            super::case_not_found_dashboard,
            "Quack couldn't find that case in this server.",
        ),
        DomainError::CaseNotFound => case_or(
            f,
            super::case_not_found,
            "Quack couldn't find that case in this server.",
        ),
        DomainError::CaseAlreadyVoided => case_or(
            f,
            super::case_already_voided,
            "This case is already voided and no longer counts.",
        ),
        DomainError::CaseVoided if f.scene == Scene::Member => case_or(
            f,
            appeal::submit_voided,
            "This case was voided and no longer counts. You can still review it on the dashboard.",
        ),
        DomainError::CaseVoided => match (f.case_number, f.action) {
            (Some(n), Some(kind)) => super::case_voided(n, kind),
            _ => Text::fixed("This case is voided. Quack won't retry its action."),
        },
        DomainError::ReplacementInvalid => case_or(
            f,
            super::replacement_invalid,
            "That case can't be replaced. Choose a voided case in this server that doesn't already have a replacement.",
        ),
        DomainError::RuleNotFound => super::rule_not_found(),
        DomainError::RuleArchived => f.rule.map_or_else(
            || Text::fixed("That rule is archived, so it can't be applied. Choose an active rule."),
            super::rule_archived,
        ),
        DomainError::RuleChanged => match (f.scene, f.rule) {
            (Scene::AddingCase, Some(rule)) => super::rule_changed(rule),
            (Scene::AddingCase, None) => {
                Text::fixed("The rule changed while you were adding the case. Run ")
                    .command(crate::model::Command::CaseAdd)
                    .s(" again to use the new version.")
            }
            (Scene::Staff | Scene::Dashboard | Scene::EditingRule | Scene::Member, _) => {
                api::rule_changed()
            }
        },
        DomainError::RuleNameTaken => f.rule.map_or_else(
            || Text::fixed("Another active rule already has this name. Choose a different name."),
            api::rule_name_taken,
        ),
        DomainError::TooManyRules => api::too_many_rules(),
        DomainError::RuleUsedByHoneypot => api::rule_used_by_honeypot(),
        DomainError::RequiredFieldOnHoneypotRule => api::required_field_on_honeypot_rule(),
        DomainError::ActionNotRetryable => action_or(
            f,
            super::action_not_retryable,
            "This action can no longer be retried. Open the case to see its current outcome.",
        ),
        DomainError::ActionNotDismissable => action_or(
            f,
            super::action_not_dismissable,
            "This failure can no longer be dismissed. Open the case to see its current outcome.",
        ),
        DomainError::ActionNotUndoable => action_or(
            f,
            super::action_not_undoable,
            "This action can't be undone from the case. Open the case to see its current outcome.",
        ),
        DomainError::TimeoutWindowPassed => f.member.map_or_else(
            || Text::fixed("The timeout's end time has passed, so Quack won't retry it."),
            super::timeout_window_passed,
        ),
        DomainError::AppealNotFound if f.scene == Scene::Dashboard => {
            super::appeal_not_found_dashboard()
        }
        DomainError::AppealNotFound => super::appeal_not_found(),
        DomainError::AppealChanged if f.scene == Scene::Member => appeal::information_outdated(),
        DomainError::AppealChanged => super::appeal_changed(),
        DomainError::AppealExists => f
            .case_number
            .map_or_else(api::appeal_exists, appeal::submit_exists),
        DomainError::AppealNotAllowed => f
            .case_number
            .map_or_else(api::appeal_not_allowed, appeal::submit_not_allowed),
        DomainError::EvidenceLimitReached { .. } => case_or(
            f,
            super::evidence_limit_reached,
            "This case would have more than 50 evidence items. Add fewer message links.",
        ),
        DomainError::TooManyLinks => super::too_many_links(),
        DomainError::TooManyFiles
        | DomainError::EvidenceFileLimitReached { .. }
        | DomainError::EvidenceChannelMissing
        | DomainError::EvidenceChannelNotStaffOnly
        | DomainError::EvidenceNotFound
        | DomainError::EvidenceRemoved
        | DomainError::EvidenceNotUpload
        | DomainError::NoteChanged => evidence(*err),
        DomainError::SettingsChanged => api::settings_changed(),
        DomainError::TicketNotOpen => ticket::close_not_open(),
    }
}

/// The evidence refusals (§7), which need no facts.
fn evidence(err: DomainError) -> Text {
    match err {
        DomainError::TooManyFiles => super::too_many_files(),
        DomainError::EvidenceFileLimitReached { count } => {
            super::evidence_file_limit_reached(count)
        }
        DomainError::EvidenceChannelMissing => super::evidence_channel_missing(),
        DomainError::EvidenceChannelNotStaffOnly => super::evidence_channel_not_staff_only(),
        DomainError::EvidenceNotFound => super::evidence_not_found(),
        DomainError::EvidenceRemoved => super::evidence_removed(),
        DomainError::EvidenceNotUpload => super::evidence_not_upload(),
        _ => super::note_changed(),
    }
}
