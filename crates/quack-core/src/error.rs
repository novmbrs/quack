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
    /// More files than one upload accepts (10).
    #[error("too many files")]
    TooManyFiles,
    /// The case's uploaded files would pass the per-case cap (100); `count`
    /// is how many files its items already have.
    #[error("evidence file limit reached")]
    EvidenceFileLimitReached {
        /// Files the case's items (not removed) already have.
        count: u32,
    },
    /// Uploading needs an evidence channel, and none is set (or it is gone).
    #[error("no evidence channel")]
    EvidenceChannelMissing,
    /// Uploading needs a staff-only evidence channel, and this one isn't.
    #[error("evidence channel not staff-only")]
    EvidenceChannelNotStaffOnly,
    /// No such evidence item on this case.
    #[error("evidence not found")]
    EvidenceNotFound,
    /// The evidence item was removed, so it can't be changed.
    #[error("evidence removed")]
    EvidenceRemoved,
    /// Only uploaded files can be shown to or hidden from the member.
    #[error("evidence not an upload")]
    EvidenceNotUpload,
    /// The note changed since the editor loaded it.
    #[error("note changed")]
    NoteChanged,
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
            Self::CaseNotFound
            | Self::RuleNotFound
            | Self::AppealNotFound
            | Self::EvidenceNotFound => "not_found",
            Self::CaseAlreadyVoided => "case_already_voided",
            Self::CaseVoided => "case_voided",
            Self::ReplacementInvalid => "replacement_invalid",
            Self::RuleArchived => "rule_archived",
            Self::RuleChanged => "rule_changed",
            Self::RuleNameTaken => "rule_name_taken",
            Self::TooManyRules => "too_many_rules",
            Self::RuleUsedByHoneypot => "rule_used_by_honeypot",
            Self::RequiredFieldOnHoneypotRule => "required_field_on_honeypot_rule",
            Self::ActionNotRetryable => "action_not_retryable",
            Self::ActionNotDismissable => "action_not_dismissable",
            Self::ActionNotUndoable => "action_not_undoable",
            Self::TimeoutWindowPassed => "timeout_window_passed",
            Self::AppealChanged => "appeal_changed",
            Self::AppealExists => "appeal_exists",
            Self::AppealNotAllowed => "appeal_not_allowed",
            Self::EvidenceLimitReached { .. } | Self::EvidenceFileLimitReached { .. } => {
                "evidence_limit_reached"
            }
            Self::TooManyLinks => "too_many_links",
            Self::TooManyFiles => "too_many_files",
            Self::EvidenceChannelMissing | Self::EvidenceChannelNotStaffOnly => {
                "evidence_channel_unavailable"
            }
            Self::EvidenceRemoved => "evidence_removed",
            Self::EvidenceNotUpload => "evidence_not_upload",
            Self::NoteChanged => "note_changed",
            Self::SettingsChanged => "settings_changed",
            Self::TicketNotOpen => "ticket_not_open",
        }
    }

    /// The HTTP status the API answers this refusal with (§14.6).
    pub const fn http_status(&self) -> u16 {
        match self {
            Self::MissingCapability(_) | Self::MfaRequired => 403,
            Self::CaseNotFound
            | Self::RuleNotFound
            | Self::AppealNotFound
            | Self::EvidenceNotFound => 404,
            Self::MemberRefused(_)
            | Self::RuleArchived
            | Self::AppealNotAllowed
            | Self::TimeoutWindowPassed
            | Self::EvidenceLimitReached { .. }
            | Self::EvidenceFileLimitReached { .. }
            | Self::TooManyLinks
            | Self::TooManyFiles => 422,
            _ => 409,
        }
    }
}

impl From<MemberRefusal> for DomainError {
    fn from(refusal: MemberRefusal) -> Self {
        Self::MemberRefused(refusal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_codes_and_statuses() {
        let cases = [
            (DomainError::ActionNotRetryable, "action_not_retryable", 409),
            (
                DomainError::ActionNotDismissable,
                "action_not_dismissable",
                409,
            ),
            (DomainError::ActionNotUndoable, "action_not_undoable", 409),
            (DomainError::CaseVoided, "case_voided", 409),
            (
                DomainError::TimeoutWindowPassed,
                "timeout_window_passed",
                422,
            ),
            (DomainError::TooManyLinks, "too_many_links", 422),
            (
                DomainError::EvidenceLimitReached { count: 50 },
                "evidence_limit_reached",
                422,
            ),
            (DomainError::AppealChanged, "appeal_changed", 409),
            (DomainError::TooManyFiles, "too_many_files", 422),
            (
                DomainError::EvidenceFileLimitReached { count: 95 },
                "evidence_limit_reached",
                422,
            ),
            (
                DomainError::EvidenceChannelNotStaffOnly,
                "evidence_channel_unavailable",
                409,
            ),
            (DomainError::EvidenceNotFound, "not_found", 404),
            (DomainError::EvidenceRemoved, "evidence_removed", 409),
            (DomainError::EvidenceNotUpload, "evidence_not_upload", 409),
            (DomainError::NoteChanged, "note_changed", 409),
            (DomainError::CaseNotFound, "not_found", 404),
            (DomainError::MfaRequired, "mfa_required", 403),
            (
                DomainError::MemberRefused(MemberRefusal::MemberIsBot),
                "member_refused",
                422,
            ),
        ];
        for (err, code, status) in cases {
            assert_eq!((err.code(), err.http_status()), (code, status), "{err:?}");
        }
    }
}
