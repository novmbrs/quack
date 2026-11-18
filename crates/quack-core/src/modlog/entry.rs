//! [`ModLogEntry`]: one variant per kind, each with its typed `detail`.

use serde::{Deserialize, Serialize};

use super::{ModLogKind, ModLogResult};
use crate::action::{ActionFailure, ActionKind, ActionSkipReason};
use crate::case::{CaseSource, VoidCause};
use crate::dm::{DmFailure, DmKind};
use crate::ids::{ActionId, ModLogId};
use crate::modules::{
    ChannelSetting, HoneypotRefusal, LogChannelFailure, ModuleDisabledReason, ModuleName,
};
use crate::permissions::{DiscordPermission, MemberRefusal, MemberRefusalCode};
use crate::rule::FieldKey;

macro_rules! entries {
    ($( $(#[$meta:meta])* $variant:ident($detail:ty), )+) => {
        /// One moderation log entry's kind and typed detail. Serialize the
        /// detail with [`ModLogEntry::detail`]; the kind is its own column.
        #[derive(Clone, Debug, PartialEq, Eq)]
        pub enum ModLogEntry {
            $( $(#[$meta])* $variant($detail), )+
        }

        impl ModLogEntry {
            /// The entry's kind.
            pub const fn kind(&self) -> ModLogKind {
                match self {
                    $( Self::$variant(_) => ModLogKind::$variant, )+
                }
            }

            /// The `detail` JSON object.
            pub fn detail(&self) -> Result<serde_json::Value, serde_json::Error> {
                match self {
                    $( Self::$variant(d) => serde_json::to_value(d), )+
                }
            }

            /// Reads an entry back from its kind and `detail` column.
            pub fn from_detail(kind: ModLogKind, detail: serde_json::Value) -> Result<Self, serde_json::Error> {
                match kind {
                    $( ModLogKind::$variant => serde_json::from_value(detail).map(Self::$variant), )+
                }
            }
        }
    };
}

entries! {
    /// `case_created`.
    CaseCreated(CaseCreatedDetail),
    /// `case_create_refused`.
    CaseCreateRefused(CaseCreateRefusedDetail),
    /// `case_voided`.
    CaseVoided(CaseVoidedDetail),
    /// `case_context_edited`.
    CaseContextEdited(CaseContextEditedDetail),
    /// `evidence_added`.
    EvidenceAdded(EvidenceAddedDetail),
    /// `action_done`.
    ActionDone(ActionDoneDetail),
    /// `action_failed`.
    ActionFailed(ActionFailedDetail),
    /// `action_retry_requested`.
    ActionRetryRequested(ActionDetail),
    /// `action_dismissed`.
    ActionDismissed(ActionDetail),
    /// `action_undo_requested` (the undo's kind).
    ActionUndoRequested(ActionDetail),
    /// `action_undo_skipped`.
    ActionUndoSkipped(UndoSkippedDetail),
    /// `dm_sent`.
    DmSent(DmSentDetail),
    /// `dm_failed`.
    DmFailed(DmFailedDetail),
    /// `appeal_submitted`.
    AppealSubmitted(CaseNumberDetail),
    /// `appeal_information_requested`.
    AppealInformationRequested(CaseNumberDetail),
    /// `appeal_information_provided`.
    AppealInformationProvided(CaseNumberDetail),
    /// `appeal_accepted`.
    AppealAccepted(CaseNumberDetail),
    /// `appeal_rejected`.
    AppealRejected(CaseNumberDetail),
    /// `appeal_closed`.
    AppealClosed(AppealClosedDetail),
    /// `appeal_reopened`.
    AppealReopened(CaseNumberDetail),
    /// `rule_created`.
    RuleCreated(RuleVersionDetail),
    /// `rule_edited`.
    RuleEdited(RuleVersionDetail),
    /// `rule_archived`.
    RuleArchived(RuleNameDetail),
    /// `rule_restored`.
    RuleRestored(RuleNameDetail),
    /// `rule_imported`.
    RuleImported(RuleNameDetail),
    /// `settings_changed`.
    SettingsChanged(FieldsDetail),
    /// `module_enabled`.
    ModuleEnabled(ModuleDetail),
    /// `module_disabled`.
    ModuleDisabled(ModuleDisabledDetail),
    /// `module_settings_changed`.
    ModuleSettingsChanged(ModuleSettingsChangedDetail),
    /// `quack_joined`.
    QuackJoined(QuackJoinedDetail),
    /// `quack_left`.
    QuackLeft(Empty),
    /// `channel_setting_cleared`.
    ChannelSettingCleared(ChannelSettingClearedDetail),
    /// `log_channel_failed`.
    LogChannelFailed(LogChannelFailedDetail),
    /// `ticket_opened`.
    TicketOpened(Empty),
    /// `ticket_closed`.
    TicketClosed(TicketClosedDetail),
    /// `honeypot_case_failed`.
    HoneypotCaseFailed(HoneypotCaseFailedDetail),
    /// `legacy` (imported Go rows only, already redacted).
    Legacy(LegacyDetail),
}

impl ModLogEntry {
    /// The entry's result; `None` for `legacy`, which keeps the imported one.
    pub const fn result(&self) -> Option<ModLogResult> {
        self.kind().result()
    }

    /// Whether this entry is posted to the log channel: its kind is
    /// posted, except `module_disabled` because Quack left the server.
    pub const fn is_posted(&self) -> bool {
        match self {
            Self::ModuleDisabled(ModuleDisabledDetail {
                reason: Some(ModuleDisabledReason::QuackLeft),
                ..
            }) => false,
            _ => self.kind().is_posted(),
        }
    }
}

/// An empty detail object.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Empty {}

/// `case_created` detail: the case's decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaseCreatedDetail {
    /// Where the case came from.
    pub source: CaseSource,
    /// Its number.
    pub case_number: u64,
    /// 1-based level position.
    pub level_index: u8,
    /// That level's start.
    pub level_from_case_count: u32,
    /// Counted cases including this one.
    pub case_count: u32,
    /// The decided action, if any.
    pub action_kind: Option<ActionKind>,
    /// For a timeout.
    pub timeout_seconds: Option<u32>,
    /// For a ban.
    pub ban_delete_message_seconds: Option<u32>,
    /// Whether a DM is sent.
    pub sends_dm: bool,
    /// The voided case this one replaces.
    pub replaces_case_number: Option<u64>,
}

/// `case_create_refused` detail: which of Quack's own access checks failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaseCreateRefusedDetail {
    /// `quack_not_in_server`, `quack_role_too_low`, or `quack_missing_permission`.
    pub refusal: MemberRefusalCode,
    /// For `quack_missing_permission`.
    pub permission: Option<DiscordPermission>,
}

impl CaseCreateRefusedDetail {
    /// The detail for a refusal caused by Quack's access; `None` for other
    /// refusals, which are never logged.
    pub const fn from_refusal(refusal: MemberRefusal) -> Option<Self> {
        if refusal.is_quack_access() {
            Some(Self {
                refusal: refusal.code(),
                permission: refusal.permission(),
            })
        } else {
            None
        }
    }
}

/// `case_voided` detail.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaseVoidedDetail {
    /// The case's number.
    pub case_number: u64,
    /// Staff void or accepted appeal.
    pub cause: VoidCause,
    /// Pending originals the void cancelled.
    pub cancelled_action_ids: Vec<ActionId>,
    /// Undo actions the void queued.
    pub undo_action_ids: Vec<ActionId>,
}

/// `case_context_edited` detail.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaseContextEditedDetail {
    /// The case's number.
    pub case_number: u64,
    /// Keys whose answers changed (never the answers).
    pub changed_keys: Vec<FieldKey>,
}

/// `evidence_added` detail.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceAddedDetail {
    /// The case's number.
    pub case_number: u64,
    /// How many items were added.
    pub count: u32,
}

/// `action_done` detail.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionDoneDetail {
    /// The case's number.
    pub case_number: u64,
    /// The action's kind.
    pub kind: ActionKind,
    /// Attempts it took, across budgets.
    pub attempts: u32,
    /// Done because the desired state already held.
    pub no_change: bool,
}

/// `action_failed` detail. `reason` includes `unconfirmed`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionFailedDetail {
    /// The case's number.
    pub case_number: u64,
    /// The action's kind.
    pub kind: ActionKind,
    /// Why it failed.
    pub reason: ActionFailure,
    /// For `quack_missing_permission`.
    pub permission: Option<DiscordPermission>,
    /// For `discord_refused`.
    pub http_status: Option<u16>,
    /// For `discord_refused`.
    pub discord_code: Option<u32>,
    /// Attempts made, across budgets.
    pub attempts: u32,
}

/// Detail naming a case and an action kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionDetail {
    /// The case's number.
    pub case_number: u64,
    /// The action's kind.
    pub kind: ActionKind,
}

/// `action_undo_skipped` detail.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UndoSkippedDetail {
    /// The undo's case number.
    pub case_number: u64,
    /// The undo's kind.
    pub kind: ActionKind,
    /// Why the undo left Discord unchanged.
    pub skip_reason: ActionSkipReason,
    /// For `other_case_applies`: the case that still applies.
    pub still_applies_case_number: Option<u64>,
}

/// `dm_sent` detail.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DmSentDetail {
    /// What the DM was about.
    pub dm_kind: DmKind,
}

str_enum! {
    /// The settled state a `dm_failed` entry records.
    pub enum DmFailedState {
        /// Discord refused.
        Failed = "failed",
        /// Couldn't confirm.
        Unknown = "unknown",
    }
}

/// `dm_failed` detail.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DmFailedDetail {
    /// What the DM was about.
    pub dm_kind: DmKind,
    /// Failed or couldn't confirm.
    pub state: DmFailedState,
    /// For `failed`.
    pub failure: Option<DmFailure>,
    /// Discord's error code, when it sent one.
    pub discord_code: Option<u32>,
}

/// Detail naming only a case number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaseNumberDetail {
    /// The case's number.
    pub case_number: u64,
}

/// `appeal_closed` detail.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppealClosedDetail {
    /// The case's number.
    pub case_number: u64,
    /// Quack closed it because the case was voided directly.
    pub because_case_voided: bool,
}

/// `rule_created` and `rule_edited` detail.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleVersionDetail {
    /// The rule's name in that version.
    pub name: String,
    /// The version written.
    pub version: u32,
}

/// `rule_archived`, `rule_restored`, and `rule_imported` detail.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleNameDetail {
    /// The rule's name.
    pub name: String,
}

/// `settings_changed` detail: setting names, never values.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldsDetail {
    /// The changed setting names.
    pub fields: Vec<String>,
}

/// `module_enabled` detail.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleDetail {
    /// Which module.
    pub module: ModuleName,
}

/// `module_disabled` detail.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleDisabledDetail {
    /// Which module.
    pub module: ModuleName,
    /// `None` when staff turned it off; `channel_deleted` or `quack_left`
    /// when Quack did.
    pub reason: Option<ModuleDisabledReason>,
}

/// `module_settings_changed` detail.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleSettingsChangedDetail {
    /// Which module.
    pub module: ModuleName,
    /// The changed setting names.
    pub fields: Vec<String>,
}

/// `quack_joined` detail.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuackJoinedDetail {
    /// Whether this is the first time Quack joined the server.
    pub first_join: bool,
}

/// `channel_setting_cleared` detail.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelSettingClearedDetail {
    /// Which setting was cleared.
    pub setting: ChannelSetting,
}

/// `log_channel_failed` detail.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogChannelFailedDetail {
    /// The entry that wasn't posted.
    pub entry_id: ModLogId,
    /// Why.
    pub reason: LogChannelFailure,
    /// Discord's error code, when it sent one.
    pub discord_code: Option<u32>,
}

/// `ticket_closed` detail.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketClosedDetail {
    /// Whether the transcript has every message.
    pub transcript_complete: bool,
}

/// `honeypot_case_failed` detail.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HoneypotCaseFailedDetail {
    /// A member refusal, `rule_unavailable`, `policy_changed`, or
    /// `discord_unavailable`.
    pub refusal: HoneypotRefusal,
}

/// `legacy` detail: an imported Go entry, passed through [`super::redact`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyDetail {
    /// The Go action name.
    pub action: String,
    /// The Go resource type.
    pub resource_type: String,
    /// The Go resource id.
    pub resource_id: Option<String>,
    /// The Go failure reason, redacted and cut to 240 characters.
    pub failure_reason: Option<String>,
}
