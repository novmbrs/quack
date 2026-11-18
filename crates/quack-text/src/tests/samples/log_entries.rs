//! One moderation log entry per kind and result variant (§12.10).

use quack_core::action::{ActionFailure, ActionKind, ActionSkipReason};
use quack_core::case::{CaseSource, VoidCause};
use quack_core::dm::{DmFailure, DmKind};
use quack_core::ids::ModLogId;
use quack_core::modlog::{
    ActionDetail, ActionDoneDetail, ActionFailedDetail, AppealClosedDetail,
    CaseContextEditedDetail, CaseCreateRefusedDetail, CaseCreatedDetail, CaseNumberDetail,
    CaseVoidedDetail, ChannelSettingClearedDetail, DmFailedDetail, DmFailedState, DmSentDetail,
    Empty, EvidenceAddedDetail, FieldsDetail, HoneypotCaseFailedDetail, LegacyDetail,
    LogChannelFailedDetail, ModLogEntry, ModuleDetail, ModuleDisabledDetail,
    ModuleSettingsChangedDetail, QuackJoinedDetail, RuleNameDetail, RuleVersionDetail,
    TicketClosedDetail, UndoSkippedDetail,
};
use quack_core::modules::{
    ChannelSetting, HoneypotRefusal, LogChannelFailure, ModuleDisabledReason, ModuleName,
};
use quack_core::permissions::{DiscordPermission, MemberRefusalCode};
use quack_core::rule::FieldKey;
use uuid::Uuid;

fn created(
    source: CaseSource,
    action_kind: Option<ActionKind>,
    replaces: Option<u64>,
) -> ModLogEntry {
    ModLogEntry::CaseCreated(CaseCreatedDetail {
        source,
        case_number: 12,
        level_index: 2,
        level_from_case_count: 3,
        case_count: 4,
        action_kind,
        timeout_seconds: (action_kind == Some(ActionKind::Timeout)).then_some(3_600),
        ban_delete_message_seconds: (action_kind == Some(ActionKind::Ban)).then_some(86_400),
        sends_dm: true,
        replaces_case_number: replaces,
    })
}

/// One entry per kind and result variant, with the catalog id it shows.
pub fn entries() -> Vec<(&'static str, String, ModLogEntry)> {
    let mut v: Vec<(&'static str, String, ModLogEntry)> = Vec::new();
    let mut push = |id: &'static str, name: &str, e| v.push((id, name.to_owned(), e));
    push(
        "log.case_created",
        "warning",
        created(CaseSource::Discord, None, None),
    );
    push(
        "log.case_created",
        "timeout",
        created(CaseSource::Dashboard, Some(ActionKind::Timeout), None),
    );
    push(
        "log.case_created.replaces",
        "ban, replacing",
        created(CaseSource::Discord, Some(ActionKind::Ban), Some(9)),
    );
    push(
        "log.case_created.honeypot",
        "kick",
        created(CaseSource::Honeypot, Some(ActionKind::Kick), None),
    );
    for (id, code) in [
        (
            "log.case_create_refused.quack_role_too_low",
            MemberRefusalCode::QuackRoleTooLow,
        ),
        (
            "log.case_create_refused.quack_missing_permission",
            MemberRefusalCode::QuackMissingPermission,
        ),
        (
            "log.case_create_refused.quack_not_in_server",
            MemberRefusalCode::QuackNotInServer,
        ),
        ("log.case_create_refused", MemberRefusalCode::MemberIsBot),
    ] {
        push(
            id,
            code.as_str(),
            ModLogEntry::CaseCreateRefused(CaseCreateRefusedDetail {
                refusal: code,
                permission: Some(DiscordPermission::KickMembers),
            }),
        );
    }
    for (id, cause) in [
        ("log.case_voided", VoidCause::Staff),
        ("log.case_voided.appeal", VoidCause::AppealAccepted),
    ] {
        push(
            id,
            "",
            ModLogEntry::CaseVoided(CaseVoidedDetail {
                case_number: 12,
                cause,
                cancelled_action_ids: vec![],
                undo_action_ids: vec![],
            }),
        );
    }
    push(
        "log.case_context_edited",
        "",
        ModLogEntry::CaseContextEdited(CaseContextEditedDetail {
            case_number: 12,
            changed_keys: vec![FieldKey("what".to_owned())],
        }),
    );
    push(
        "log.evidence_added",
        "one",
        ModLogEntry::EvidenceAdded(EvidenceAddedDetail {
            case_number: 12,
            count: 1,
        }),
    );
    push(
        "log.evidence_added",
        "other",
        ModLogEntry::EvidenceAdded(EvidenceAddedDetail {
            case_number: 12,
            count: 4,
        }),
    );
    for kind in ActionKind::ALL {
        for no_change in [false, true] {
            let id = match (kind, no_change) {
                (ActionKind::Timeout, false) => "log.action_done.timeout",
                (ActionKind::Kick, false) => "log.action_done.kick",
                (ActionKind::Kick, true) => "log.action_done.kick_no_change",
                (ActionKind::Ban, false) => "log.action_done.ban",
                (ActionKind::RemoveTimeout, false) => "log.action_done.remove_timeout",
                (ActionKind::RemoveTimeout, true) => "log.action_done.remove_timeout_no_change",
                (ActionKind::Unban, false) => "log.action_done.unban",
                (ActionKind::Unban, true) => "log.action_done.unban_no_change",
                (ActionKind::Timeout | ActionKind::Ban, true) => "log.action_done",
            };
            push(
                id,
                &format!("{kind:?} no_change={no_change}"),
                ModLogEntry::ActionDone(ActionDoneDetail {
                    case_number: 12,
                    kind: *kind,
                    attempts: 1,
                    no_change,
                }),
            );
        }
        for reason in ActionFailure::ALL {
            let id = if *reason == ActionFailure::Unconfirmed {
                "log.action_failed.unconfirmed"
            } else {
                "log.action_failed"
            };
            push(
                id,
                &format!("{kind:?} {reason:?}"),
                ModLogEntry::ActionFailed(ActionFailedDetail {
                    case_number: 12,
                    kind: *kind,
                    reason: *reason,
                    permission: Some(kind.permission()),
                    http_status: Some(403),
                    discord_code: Some(50_013),
                    attempts: 8,
                }),
            );
        }
        let d = ActionDetail {
            case_number: 12,
            kind: *kind,
        };
        push(
            "log.action_retry_requested",
            &format!("{kind:?}"),
            ModLogEntry::ActionRetryRequested(d),
        );
        push(
            "log.action_dismissed",
            &format!("{kind:?}"),
            ModLogEntry::ActionDismissed(d),
        );
    }
    push(
        "log.action_undo_requested.remove_timeout",
        "",
        ModLogEntry::ActionUndoRequested(ActionDetail {
            case_number: 12,
            kind: ActionKind::RemoveTimeout,
        }),
    );
    push(
        "log.action_undo_requested.unban",
        "",
        ModLogEntry::ActionUndoRequested(ActionDetail {
            case_number: 12,
            kind: ActionKind::Unban,
        }),
    );
    for (id, kind, reason, other) in [
        (
            "log.action_undo_skipped.timeout",
            ActionKind::RemoveTimeout,
            ActionSkipReason::OtherCaseApplies,
            Some(7),
        ),
        (
            "log.action_undo_skipped.ban",
            ActionKind::Unban,
            ActionSkipReason::OtherCaseApplies,
            Some(7),
        ),
        (
            "log.action_undo_skipped.ban",
            ActionKind::Unban,
            ActionSkipReason::OtherCaseApplies,
            None,
        ),
        (
            "log.action_undo_skipped.changed_timeout",
            ActionKind::RemoveTimeout,
            ActionSkipReason::ChangedOutsideCase,
            None,
        ),
        (
            "log.action_undo_skipped.changed_ban",
            ActionKind::Unban,
            ActionSkipReason::ChangedOutsideCase,
            None,
        ),
    ] {
        push(
            id,
            &format!("{reason:?} {other:?}"),
            ModLogEntry::ActionUndoSkipped(UndoSkippedDetail {
                case_number: 12,
                kind,
                skip_reason: reason,
                still_applies_case_number: other,
            }),
        );
    }
    for (id, kind) in [
        ("log.dm_sent.case", DmKind::Case),
        ("log.dm_sent.appeal", DmKind::AppealUpdate),
        ("log.dm_sent.ticket", DmKind::TicketClosed),
    ] {
        push(id, "", ModLogEntry::DmSent(DmSentDetail { dm_kind: kind }));
    }
    for (kind, failed, unknown) in [
        (
            DmKind::Case,
            "log.dm_failed.case",
            "log.dm_failed.case_unknown",
        ),
        (
            DmKind::AppealUpdate,
            "log.dm_failed.appeal",
            "log.dm_failed.appeal_unknown",
        ),
        (
            DmKind::TicketClosed,
            "log.dm_failed.ticket",
            "log.dm_failed.ticket_unknown",
        ),
    ] {
        for f in DmFailure::ALL {
            push(
                failed,
                &format!("{f:?}"),
                ModLogEntry::DmFailed(DmFailedDetail {
                    dm_kind: kind,
                    state: DmFailedState::Failed,
                    failure: Some(*f),
                    discord_code: Some(50_007),
                }),
            );
        }
        push(
            unknown,
            "",
            ModLogEntry::DmFailed(DmFailedDetail {
                dm_kind: kind,
                state: DmFailedState::Unknown,
                failure: None,
                discord_code: None,
            }),
        );
    }
    let n = CaseNumberDetail { case_number: 12 };
    push("log.appeal_submitted", "", ModLogEntry::AppealSubmitted(n));
    push(
        "log.appeal_information_requested",
        "",
        ModLogEntry::AppealInformationRequested(n),
    );
    push(
        "log.appeal_information_provided",
        "",
        ModLogEntry::AppealInformationProvided(n),
    );
    push("log.appeal_accepted", "", ModLogEntry::AppealAccepted(n));
    push("log.appeal_rejected", "", ModLogEntry::AppealRejected(n));
    push(
        "log.appeal_closed",
        "",
        ModLogEntry::AppealClosed(AppealClosedDetail {
            case_number: 12,
            because_case_voided: false,
        }),
    );
    push(
        "log.appeal_closed.case_voided",
        "",
        ModLogEntry::AppealClosed(AppealClosedDetail {
            case_number: 12,
            because_case_voided: true,
        }),
    );
    push("log.appeal_reopened", "", ModLogEntry::AppealReopened(n));
    push(
        "log.rule_created",
        "",
        ModLogEntry::RuleCreated(RuleVersionDetail {
            name: "Spam".to_owned(),
            version: 1,
        }),
    );
    push(
        "log.rule_edited",
        "",
        ModLogEntry::RuleEdited(RuleVersionDetail {
            name: "Spam".to_owned(),
            version: 4,
        }),
    );
    push(
        "log.rule_archived",
        "",
        ModLogEntry::RuleArchived(RuleNameDetail {
            name: "Spam".to_owned(),
        }),
    );
    push(
        "log.rule_restored",
        "",
        ModLogEntry::RuleRestored(RuleNameDetail {
            name: "Spam".to_owned(),
        }),
    );
    push(
        "log.rule_imported",
        "",
        ModLogEntry::RuleImported(RuleNameDetail {
            name: "Spam".to_owned(),
        }),
    );
    let every_setting = [
        "log_channel_id",
        "appeal_channel_id",
        "evidence_channel_id",
        "appeal_invite_url",
        "dm_intro",
        "dm_footer",
        "moderator_role_ids",
        "rules_manager_role_ids",
        "starter_notice",
        "ticket_entry_channel",
        "ticket_queue_channel",
        "transcript_retention",
        "honeypot_channel",
        "honeypot_rule",
        "honeypot_warning",
        "activity_routes",
        "activity_content",
        "something_new",
    ];
    push(
        "log.settings_changed",
        "one",
        ModLogEntry::SettingsChanged(FieldsDetail {
            fields: vec!["log_channel_id".to_owned()],
        }),
    );
    push(
        "log.settings_changed",
        "every setting name",
        ModLogEntry::SettingsChanged(FieldsDetail {
            fields: every_setting.iter().map(|s| (*s).to_owned()).collect(),
        }),
    );
    for m in ModuleName::ALL {
        push(
            "log.module_enabled",
            &format!("{m:?}"),
            ModLogEntry::ModuleEnabled(ModuleDetail { module: *m }),
        );
        push(
            "log.module_disabled",
            &format!("{m:?}"),
            ModLogEntry::ModuleDisabled(ModuleDisabledDetail {
                module: *m,
                reason: None,
            }),
        );
        push(
            "log.module_disabled.channel_deleted",
            &format!("{m:?}"),
            ModLogEntry::ModuleDisabled(ModuleDisabledDetail {
                module: *m,
                reason: Some(ModuleDisabledReason::ChannelDeleted),
            }),
        );
        push(
            "log.module_disabled.quack_left",
            &format!("{m:?}"),
            ModLogEntry::ModuleDisabled(ModuleDisabledDetail {
                module: *m,
                reason: Some(ModuleDisabledReason::QuackLeft),
            }),
        );
        push(
            "log.module_settings_changed",
            &format!("{m:?}"),
            ModLogEntry::ModuleSettingsChanged(ModuleSettingsChangedDetail {
                module: *m,
                fields: vec!["channel".to_owned(), "rule".to_owned()],
            }),
        );
    }
    push(
        "log.quack_joined.first",
        "",
        ModLogEntry::QuackJoined(QuackJoinedDetail { first_join: true }),
    );
    push(
        "log.quack_joined.again",
        "",
        ModLogEntry::QuackJoined(QuackJoinedDetail { first_join: false }),
    );
    push("log.quack_left", "", ModLogEntry::QuackLeft(Empty {}));
    for s in ChannelSetting::ALL {
        push(
            "log.channel_setting_cleared",
            &format!("{s:?}"),
            ModLogEntry::ChannelSettingCleared(ChannelSettingClearedDetail { setting: *s }),
        );
    }
    let entry_id = ModLogId::from_uuid(Uuid::from_u128(9));
    for (id, reason) in [
        (
            "log.log_channel_failed.not_staff_only",
            LogChannelFailure::NotStaffOnly,
        ),
        (
            "log.log_channel_failed.discord_refused",
            LogChannelFailure::DiscordRefused,
        ),
        (
            "log.log_channel_failed.retries_exhausted",
            LogChannelFailure::RetriesExhausted,
        ),
    ] {
        push(
            id,
            "",
            ModLogEntry::LogChannelFailed(LogChannelFailedDetail {
                entry_id,
                reason,
                discord_code: None,
            }),
        );
    }
    push("log.ticket_opened", "", ModLogEntry::TicketOpened(Empty {}));
    push(
        "log.ticket_closed",
        "",
        ModLogEntry::TicketClosed(TicketClosedDetail {
            transcript_complete: true,
        }),
    );
    push(
        "log.ticket_closed.partial",
        "",
        ModLogEntry::TicketClosed(TicketClosedDetail {
            transcript_complete: false,
        }),
    );
    for code in MemberRefusalCode::ALL {
        push(
            "log.honeypot_case_failed",
            code.as_str(),
            ModLogEntry::HoneypotCaseFailed(HoneypotCaseFailedDetail {
                refusal: HoneypotRefusal::Member(*code),
            }),
        );
    }
    for r in [
        HoneypotRefusal::RuleUnavailable,
        HoneypotRefusal::PolicyChanged,
        HoneypotRefusal::DiscordUnavailable,
    ] {
        push(
            "log.honeypot_case_failed",
            r.as_str(),
            ModLogEntry::HoneypotCaseFailed(HoneypotCaseFailedDetail { refusal: r }),
        );
    }
    push(
        "log.legacy",
        "",
        ModLogEntry::Legacy(LegacyDetail {
            action: "case_execution.retry".to_owned(),
            resource_type: "case".to_owned(),
            resource_id: None,
            failure_reason: None,
        }),
    );
    v
}
