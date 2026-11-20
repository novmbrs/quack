//! The moderation log (§9): Quack's append-only record of who did what.
//!
//! Each entry has a kind, a result, typed links (case, member, appeal,
//! action, rule, ticket), and a typed `detail`. [`ModLogEntry`] has one
//! variant per kind; its detail structs hold no message content, tokens, or
//! Discord error bodies, so those cannot enter by construction.

mod entry;
mod links;
mod redact;

pub use entry::{
    ActionDetail, ActionDoneDetail, ActionFailedDetail, AppealClosedDetail,
    CaseContextEditedDetail, CaseCreateRefusedDetail, CaseCreatedDetail, CaseNumberDetail,
    CaseVoidedDetail, ChannelSettingClearedDetail, DmFailedDetail, DmFailedState, DmSentDetail,
    Empty, EvidenceAddedDetail, EvidenceRemovedDetail, EvidenceVisibilityDetail, FieldsDetail,
    HoneypotCaseFailedDetail, LegacyDetail, LogChannelFailedDetail, ModLogEntry, ModuleDetail,
    ModuleDisabledDetail, ModuleSettingsChangedDetail, QuackJoinedDetail, RuleNameDetail,
    RuleVersionDetail, TicketClosedDetail, UndoSkippedDetail,
};
pub use links::{Link, LinkProblem, LinkRule, ModLogLinks};
pub use redact::{REDACTED, redact, redact_failure_reason};

str_enum! {
    /// What an entry records (`mod_log_kind`).
    pub enum ModLogKind {
        /// A case was created.
        CaseCreated = "case_created",
        /// Quack's own access stopped a case from being created.
        CaseCreateRefused = "case_create_refused",
        /// A case was voided.
        CaseVoided = "case_voided",
        /// A case's context was edited.
        CaseContextEdited = "case_context_edited",
        /// Evidence was added to a case.
        EvidenceAdded = "evidence_added",
        /// An action was done.
        ActionDone = "action_done",
        /// An action failed.
        ActionFailed = "action_failed",
        /// Staff retried a failed action.
        ActionRetryRequested = "action_retry_requested",
        /// Staff dismissed a failed action.
        ActionDismissed = "action_dismissed",
        /// An undo action was requested.
        ActionUndoRequested = "action_undo_requested",
        /// An undo was left in place because another case still applies.
        ActionUndoSkipped = "action_undo_skipped",
        /// A DM was sent.
        DmSent = "dm_sent",
        /// A DM failed or couldn't be confirmed.
        DmFailed = "dm_failed",
        /// A member appealed.
        AppealSubmitted = "appeal_submitted",
        /// Staff asked for more information.
        AppealInformationRequested = "appeal_information_requested",
        /// The member replied with information.
        AppealInformationProvided = "appeal_information_provided",
        /// Staff accepted an appeal.
        AppealAccepted = "appeal_accepted",
        /// Staff rejected an appeal.
        AppealRejected = "appeal_rejected",
        /// Staff or Quack closed an appeal.
        AppealClosed = "appeal_closed",
        /// Staff reopened an appeal.
        AppealReopened = "appeal_reopened",
        /// A rule was created.
        RuleCreated = "rule_created",
        /// A rule was edited (new version).
        RuleEdited = "rule_edited",
        /// A rule was archived.
        RuleArchived = "rule_archived",
        /// A rule was restored.
        RuleRestored = "rule_restored",
        /// A rule was imported from a file.
        RuleImported = "rule_imported",
        /// Settings changed.
        SettingsChanged = "settings_changed",
        /// A module was turned on.
        ModuleEnabled = "module_enabled",
        /// A module was turned off.
        ModuleDisabled = "module_disabled",
        /// A module's settings changed.
        ModuleSettingsChanged = "module_settings_changed",
        /// Quack joined the server.
        QuackJoined = "quack_joined",
        /// Quack left the server.
        QuackLeft = "quack_left",
        /// A channel setting was cleared because the channel is gone.
        ChannelSettingCleared = "channel_setting_cleared",
        /// Quack couldn't post to the log channel.
        LogChannelFailed = "log_channel_failed",
        /// A ticket was opened.
        TicketOpened = "ticket_opened",
        /// A ticket was closed.
        TicketClosed = "ticket_closed",
        /// The honeypot couldn't add a case.
        HoneypotCaseFailed = "honeypot_case_failed",
        /// An entry imported from the Go version with no new kind.
        Legacy = "legacy",
        /// Staff removed an evidence item (hidden, never deleted).
        EvidenceRemoved = "evidence_removed",
        /// Staff showed an uploaded-files item to the member, or hid it.
        EvidenceVisibilityChanged = "evidence_visibility_changed",
    }
}

str_enum! {
    /// How the recorded operation ended (`mod_log_result`).
    pub enum ModLogResult {
        /// It happened.
        Ok = "ok",
        /// Quack tried and it didn't work.
        Failed = "failed",
        /// Quack's access stopped it.
        Refused = "refused",
    }
}

str_enum! {
    /// Where the recorded operation came from (`mod_log_source`).
    pub enum ModLogSource {
        /// A Discord command or button.
        Discord = "discord",
        /// The dashboard.
        Dashboard = "dashboard",
        /// Quack on its own (jobs, gateway events).
        Quack = "quack",
        /// The MySQL importer.
        Imported = "imported",
    }
}

impl ModLogKind {
    /// The result entries of this kind always have; `None` for `legacy`,
    /// which keeps the imported result.
    pub const fn result(self) -> Option<ModLogResult> {
        match self {
            Self::CaseCreateRefused => Some(ModLogResult::Refused),
            Self::ActionFailed
            | Self::DmFailed
            | Self::LogChannelFailed
            | Self::HoneypotCaseFailed => Some(ModLogResult::Failed),
            Self::Legacy => None,
            _ => Some(ModLogResult::Ok),
        }
    }

    /// Whether entries of this kind are posted to the log channel when one
    /// is set (§9.2).
    pub const fn is_posted(self) -> bool {
        !matches!(
            self,
            Self::EvidenceAdded
                | Self::EvidenceRemoved
                | Self::EvidenceVisibilityChanged
                | Self::DmSent
                | Self::QuackLeft
                | Self::LogChannelFailed
                | Self::Legacy
        )
    }
}

#[cfg(test)]
mod tests;
