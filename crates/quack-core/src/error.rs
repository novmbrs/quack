//! Domain errors shared across modules.

use crate::permissions::{Capability, MemberRefusal};

/// A string that is not one of a closed enum's names.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{value:?} is not a valid {type_name}")]
pub struct UnknownValue {
    /// The enum's Rust name.
    pub type_name: &'static str,
    /// The rejected input.
    pub value: String,
}

/// A state machine refused an event that its transition table does not list
/// for the current state.
///
/// This is a programming or concurrency error (the caller loaded a stale
/// state), never something a person caused, so it has no user-facing text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{machine}: no transition from {from} on {event}")]
pub struct InvalidTransition {
    /// Which state machine, e.g. `"action"`.
    pub machine: &'static str,
    /// The current state's name.
    pub from: &'static str,
    /// The refused event's name.
    pub event: &'static str,
}

/// A refusal a person can cause, each mapped to one API error code and one
/// message in `quack-text`.
///
/// Field-level validation problems ([`crate::rule::RuleProblem`],
/// [`crate::case::ContextProblem`]) are reported separately because they come
/// in lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DomainError {
    /// The actor lacks the capability the operation needs.
    #[error("missing capability {0:?}")]
    MissingCapability(Capability),
    /// The server requires 2FA and Quack has not recorded it for the actor.
    #[error("two-factor authentication required")]
    MfaRequired,
    /// A member check refused the operation (§2.3).
    #[error("member check refused: {0:?}")]
    MemberRefused(MemberRefusal),
    /// No such case in this server (or not the member's own case).
    #[error("case not found")]
    CaseNotFound,
    /// The case is already voided.
    #[error("case already voided")]
    CaseAlreadyVoided,
    /// The case is voided, so the operation no longer applies.
    #[error("case voided")]
    CaseVoided,
    /// The case named as replaced is not a voided, unreplaced case here.
    #[error("replacement invalid")]
    ReplacementInvalid,
    /// No such rule in this server.
    #[error("rule not found")]
    RuleNotFound,
    /// The rule is archived and cannot be applied.
    #[error("rule archived")]
    RuleArchived,
    /// The rule changed since the editor loaded it (`expected_version`).
    #[error("rule changed")]
    RuleChanged,
    /// Another active rule in the server has this name.
    #[error("rule name taken")]
    RuleNameTaken,
    /// The server already has the maximum number of active rules.
    #[error("too many rules")]
    TooManyRules,
    /// The honeypot uses this rule, so it cannot be archived.
    #[error("rule used by honeypot")]
    RuleUsedByHoneypot,
    /// The honeypot uses this rule, so no field may be required.
    #[error("required field on honeypot rule")]
    RequiredFieldOnHoneypotRule,
    /// The action is not failed (or was dismissed), so it cannot be retried.
    #[error("action not retryable")]
    ActionNotRetryable,
    /// The action is not failed (or was dismissed), so it cannot be dismissed.
    #[error("action not dismissable")]
    ActionNotDismissable,
    /// There is nothing to undo.
    #[error("action not undoable")]
    ActionNotUndoable,
    /// The timeout's fixed end has passed.
    #[error("timeout window passed")]
    TimeoutWindowPassed,
    /// No such appeal in this server (or not the member's own).
    #[error("appeal not found")]
    AppealNotFound,
    /// The appeal changed since it was loaded, or its state does not allow
    /// the transition.
    #[error("appeal changed")]
    AppealChanged,
    /// The case already has an appeal.
    #[error("appeal exists")]
    AppealExists,
    /// The case cannot be appealed.
    #[error("appeal not allowed")]
    AppealNotAllowed,
    /// The case already holds the maximum evidence; `count` is how many it has.
    #[error("evidence limit reached")]
    EvidenceLimitReached {
        /// Evidence items the case already has.
        count: u32,
    },
    /// More message links than one add-evidence call accepts.
    #[error("too many links")]
    TooManyLinks,
    /// The settings changed since the editor loaded them.
    #[error("settings changed")]
    SettingsChanged,
    /// The ticket is not open.
    #[error("ticket not open")]
    TicketNotOpen,
}

impl DomainError {
    /// The API error code (§14.6) this refusal is reported with.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::MissingCapability(_) => "missing_capability",
            Self::MfaRequired => "mfa_required",
            Self::MemberRefused(_) => "member_refused",
            Self::CaseNotFound | Self::RuleNotFound | Self::AppealNotFound => "not_found",
            Self::CaseAlreadyVoided => "case_already_voided",
            Self::CaseVoided => "case_voided",
            Self::ReplacementInvalid => "replacement_invalid",
            Self::RuleArchived => "rule_archived",
            Self::RuleChanged => "rule_changed",
            Self::RuleNameTaken => "rule_name_taken",
            Self::TooManyRules => "too_many_rules",
            Self::RuleUsedByHoneypot => "rule_used_by_honeypot",
            Self::RequiredFieldOnHoneypotRule => "required_field_on_honeypot_rule",
            Self::ActionNotRetryable | Self::ActionNotDismissable => "action_not_retryable",
            Self::ActionNotUndoable => "action_not_undoable",
            Self::TimeoutWindowPassed => "timeout_window_passed",
            Self::AppealChanged => "appeal_changed",
            Self::AppealExists => "appeal_exists",
            Self::AppealNotAllowed => "appeal_not_allowed",
            Self::EvidenceLimitReached { .. } => "evidence_limit_reached",
            Self::TooManyLinks => "validation_failed",
            Self::SettingsChanged => "settings_changed",
            Self::TicketNotOpen => "ticket_not_open",
        }
    }

    /// The HTTP status the API answers this refusal with (§14.6).
    pub const fn http_status(&self) -> u16 {
        match self {
            Self::MissingCapability(_) | Self::MfaRequired => 403,
            Self::CaseNotFound | Self::RuleNotFound | Self::AppealNotFound => 404,
            Self::MemberRefused(_)
            | Self::RuleArchived
            | Self::AppealNotAllowed
            | Self::TimeoutWindowPassed
            | Self::EvidenceLimitReached { .. } => 422,
            Self::TooManyLinks => 400,
            _ => 409,
        }
    }
}

impl From<MemberRefusal> for DomainError {
    fn from(refusal: MemberRefusal) -> Self {
        Self::MemberRefused(refusal)
    }
}
