use serde_json::json;
use uuid::Uuid;

use super::*;
use crate::action::{ActionFailure, ActionKind, ActionSkipReason};
use crate::case::{CaseSource, VoidCause};
use crate::dm::{DmFailure, DmKind};
use crate::ids::{ActionId, AppealId, CaseId, MemberId, ModLogId, RuleId, TicketId};
use crate::modules::{
    ChannelSetting, HoneypotRefusal, LogChannelFailure, ModuleDisabledReason, ModuleName,
};
use crate::permissions::{DiscordPermission, MemberRefusal, MemberRefusalCode};
use crate::rule::FieldKey;

#[allow(clippy::too_many_lines, reason = "one literal entry per kind")]
fn one_of_each() -> Vec<ModLogEntry> {
    let action = ActionDetail {
        case_number: 3,
        kind: ActionKind::Ban,
    };
    let n = CaseNumberDetail { case_number: 3 };
    let id = ActionId::from_uuid(Uuid::from_u128(7));
    vec![
        ModLogEntry::CaseCreated(CaseCreatedDetail {
            source: CaseSource::Discord,
            case_number: 3,
            level_index: 2,
            level_from_case_count: 3,
            case_count: 3,
            action_kind: Some(ActionKind::Timeout),
            timeout_seconds: Some(60),
            ban_delete_message_seconds: None,
            sends_dm: true,
            replaces_case_number: None,
        }),
        ModLogEntry::CaseCreateRefused(
            CaseCreateRefusedDetail::from_refusal(MemberRefusal::QuackMissingPermission(
                DiscordPermission::BanMembers,
            ))
            .unwrap(),
        ),
        ModLogEntry::CaseVoided(CaseVoidedDetail {
            case_number: 3,
            cause: VoidCause::Staff,
            cancelled_action_ids: vec![id],
            undo_action_ids: vec![],
        }),
        ModLogEntry::CaseContextEdited(CaseContextEditedDetail {
            case_number: 3,
            changed_keys: vec![FieldKey("why".into())],
        }),
        ModLogEntry::EvidenceAdded(EvidenceAddedDetail {
            case_number: 3,
            count: 2,
        }),
        ModLogEntry::ActionDone(ActionDoneDetail {
            case_number: 3,
            kind: ActionKind::Kick,
            attempts: 1,
            no_change: true,
        }),
        ModLogEntry::ActionFailed(ActionFailedDetail {
            case_number: 3,
            kind: ActionKind::Ban,
            reason: ActionFailure::DiscordRefused,
            permission: None,
            http_status: Some(403),
            discord_code: Some(50_013),
            attempts: 2,
            may_have_applied: false,
        }),
        ModLogEntry::ActionRetryRequested(action),
        ModLogEntry::ActionDismissed(action),
        ModLogEntry::ActionUndoRequested(ActionDetail {
            case_number: 3,
            kind: ActionKind::Unban,
        }),
        ModLogEntry::ActionUndoSkipped(UndoSkippedDetail {
            case_number: 3,
            kind: ActionKind::Unban,
            skip_reason: ActionSkipReason::OtherCaseApplies,
            still_applies_case_number: Some(9),
        }),
        ModLogEntry::DmSent(DmSentDetail {
            dm_kind: DmKind::Case,
        }),
        ModLogEntry::DmFailed(DmFailedDetail {
            dm_kind: DmKind::AppealUpdate,
            state: DmFailedState::Failed,
            failure: Some(DmFailure::DmsClosed),
            discord_code: Some(50_007),
        }),
        ModLogEntry::AppealSubmitted(n),
        ModLogEntry::AppealInformationRequested(n),
        ModLogEntry::AppealInformationProvided(n),
        ModLogEntry::AppealAccepted(n),
        ModLogEntry::AppealRejected(n),
        ModLogEntry::AppealClosed(AppealClosedDetail {
            case_number: 3,
            because_case_voided: true,
        }),
        ModLogEntry::AppealReopened(n),
        ModLogEntry::RuleCreated(RuleVersionDetail {
            name: "Spam".into(),
            version: 1,
        }),
        ModLogEntry::RuleEdited(RuleVersionDetail {
            name: "Spam".into(),
            version: 2,
        }),
        ModLogEntry::RuleArchived(RuleNameDetail {
            name: "Spam".into(),
        }),
        ModLogEntry::RuleRestored(RuleNameDetail {
            name: "Spam".into(),
        }),
        ModLogEntry::RuleImported(RuleNameDetail {
            name: "Spam".into(),
        }),
        ModLogEntry::SettingsChanged(FieldsDetail {
            fields: vec!["dm_intro".into()],
        }),
        ModLogEntry::ModuleEnabled(ModuleDetail {
            module: ModuleName::Tickets,
        }),
        ModLogEntry::ModuleDisabled(ModuleDisabledDetail {
            module: ModuleName::Honeypot,
            reason: Some(ModuleDisabledReason::ChannelDeleted),
        }),
        ModLogEntry::ModuleSettingsChanged(ModuleSettingsChangedDetail {
            module: ModuleName::ActivityLog,
            fields: vec!["routes".into()],
        }),
        ModLogEntry::QuackJoined(QuackJoinedDetail { first_join: true }),
        ModLogEntry::QuackLeft(Empty {}),
        ModLogEntry::ChannelSettingCleared(ChannelSettingClearedDetail {
            setting: ChannelSetting::LogChannel,
        }),
        ModLogEntry::LogChannelFailed(LogChannelFailedDetail {
            entry_id: ModLogId::from_uuid(Uuid::from_u128(1)),
            reason: LogChannelFailure::NotStaffOnly,
            discord_code: None,
        }),
        ModLogEntry::TicketOpened(Empty {}),
        ModLogEntry::TicketClosed(TicketClosedDetail {
            transcript_complete: false,
        }),
        ModLogEntry::HoneypotCaseFailed(HoneypotCaseFailedDetail {
            refusal: HoneypotRefusal::RuleUnavailable,
        }),
        ModLogEntry::Legacy(LegacyDetail {
            action: "x".into(),
            resource_type: "case".into(),
            resource_id: None,
            failure_reason: None,
        }),
    ]
}

#[test]
fn every_kind_has_one_variant_and_round_trips() {
    let entries = one_of_each();
    let kinds: Vec<ModLogKind> = entries.iter().map(ModLogEntry::kind).collect();
    assert_eq!(kinds, ModLogKind::ALL);
    for entry in entries {
        let detail = entry.detail().unwrap();
        assert!(detail.is_object(), "{:?}", entry.kind());
        assert_eq!(
            ModLogEntry::from_detail(entry.kind(), detail).unwrap(),
            entry
        );
    }
}

#[test]
fn detail_shapes_match_the_design() {
    let created = &one_of_each()[0];
    assert_eq!(
        created.detail().unwrap(),
        json!({
            "source": "discord", "case_number": 3, "level_index": 2, "level_from_case_count": 3,
            "case_count": 3, "action_kind": "timeout", "timeout_seconds": 60,
            "ban_delete_message_seconds": null, "sends_dm": true, "replaces_case_number": null
        })
    );
    let refused = &one_of_each()[1];
    assert_eq!(
        refused.detail().unwrap(),
        json!({ "refusal": "quack_missing_permission", "permission": "ban_members" })
    );
    assert_eq!(
        ModLogEntry::QuackLeft(Empty {}).detail().unwrap(),
        json!({})
    );
    let honeypot = HoneypotCaseFailedDetail {
        refusal: HoneypotRefusal::Member(MemberRefusalCode::MemberIsBot),
    };
    assert_eq!(
        serde_json::to_value(honeypot).unwrap(),
        json!({ "refusal": "member_is_bot" })
    );
    assert!(ModLogEntry::from_detail(ModLogKind::ActionDone, json!({ "case_number": 1 })).is_err());
    for refusal in [
        HoneypotRefusal::PolicyChanged,
        HoneypotRefusal::DiscordUnavailable,
    ] {
        assert_eq!(
            serde_json::to_value(HoneypotCaseFailedDetail { refusal }).unwrap(),
            json!({ "refusal": refusal.as_str() })
        );
    }
    let changed_outside = UndoSkippedDetail {
        case_number: 4,
        kind: ActionKind::RemoveTimeout,
        skip_reason: ActionSkipReason::ChangedOutsideCase,
        still_applies_case_number: None,
    };
    assert_eq!(
        serde_json::to_value(changed_outside).unwrap(),
        json!({
            "case_number": 4, "kind": "remove_timeout",
            "skip_reason": "changed_outside_case", "still_applies_case_number": null
        })
    );
    let unconfirmed = ActionFailedDetail {
        case_number: 4,
        kind: ActionKind::Ban,
        reason: ActionFailure::Unconfirmed,
        permission: None,
        http_status: None,
        discord_code: None,
        attempts: 8,
        may_have_applied: true,
    };
    assert_eq!(
        serde_json::to_value(unconfirmed).unwrap()["reason"],
        json!("unconfirmed")
    );
    // Entries written before `may_have_applied` existed still read.
    let old: ActionFailedDetail = serde_json::from_value(json!({
        "case_number": 4, "kind": "ban", "reason": "discord_refused", "permission": null,
        "http_status": 403, "discord_code": 50013, "attempts": 2
    }))
    .unwrap();
    assert!(!old.may_have_applied);
    let left = ModuleDisabledDetail {
        module: ModuleName::Honeypot,
        reason: Some(ModuleDisabledReason::QuackLeft),
    };
    assert_eq!(
        serde_json::to_value(left).unwrap(),
        json!({ "module": "honeypot", "reason": "quack_left" })
    );
}

#[test]
fn only_quack_access_refusals_are_logged() {
    assert!(CaseCreateRefusedDetail::from_refusal(MemberRefusal::QuackRoleTooLow).is_some());
    assert!(CaseCreateRefusedDetail::from_refusal(MemberRefusal::QuackNotInServer).is_some());
    assert!(CaseCreateRefusedDetail::from_refusal(MemberRefusal::MemberIsBot).is_none());
    assert!(CaseCreateRefusedDetail::from_refusal(MemberRefusal::MemberRoleTooHigh).is_none());
}

#[test]
fn results_and_posting() {
    use ModLogKind as K;
    let not_posted = [
        K::EvidenceAdded,
        K::DmSent,
        K::QuackLeft,
        K::LogChannelFailed,
        K::Legacy,
    ];
    for &kind in ModLogKind::ALL {
        assert_eq!(kind.is_posted(), !not_posted.contains(&kind), "{kind:?}");
        let expected = match kind {
            K::CaseCreateRefused => Some(ModLogResult::Refused),
            K::ActionFailed | K::DmFailed | K::LogChannelFailed | K::HoneypotCaseFailed => {
                Some(ModLogResult::Failed)
            }
            K::Legacy => None,
            _ => Some(ModLogResult::Ok),
        };
        assert_eq!(kind.result(), expected, "{kind:?}");
    }
    let entry = ModLogEntry::DmSent(DmSentDetail {
        dm_kind: DmKind::Case,
    });
    assert!(!entry.is_posted());
    assert_eq!(entry.result(), Some(ModLogResult::Ok));
    let disabled = |reason| {
        ModLogEntry::ModuleDisabled(ModuleDisabledDetail {
            module: ModuleName::Tickets,
            reason,
        })
    };
    assert!(disabled(None).is_posted());
    assert!(disabled(Some(ModuleDisabledReason::ChannelDeleted)).is_posted());
    assert!(!disabled(Some(ModuleDisabledReason::QuackLeft)).is_posted());
}

#[test]
fn enum_names_match_the_database() {
    let names = |v: Vec<&str>| v.join(",");
    assert_eq!(
        names(ModLogResult::ALL.iter().map(|v| v.as_str()).collect()),
        "ok,failed,refused"
    );
    assert_eq!(
        names(ModLogSource::ALL.iter().map(|v| v.as_str()).collect()),
        "discord,dashboard,quack,imported"
    );
    assert_eq!(
        names(ModLogKind::ALL.iter().map(|v| v.as_str()).collect()),
        "case_created,case_create_refused,case_voided,case_context_edited,evidence_added,\
         action_done,action_failed,action_retry_requested,action_dismissed,action_undo_requested,\
         action_undo_skipped,dm_sent,dm_failed,appeal_submitted,appeal_information_requested,\
         appeal_information_provided,appeal_accepted,appeal_rejected,appeal_closed,appeal_reopened,\
         rule_created,rule_edited,rule_archived,rule_restored,rule_imported,settings_changed,\
         module_enabled,module_disabled,module_settings_changed,quack_joined,quack_left,\
         channel_setting_cleared,log_channel_failed,ticket_opened,ticket_closed,\
         honeypot_case_failed,legacy"
    );
}

#[test]
fn links_follow_the_kind() {
    let case_id = CaseId::from_uuid(Uuid::from_u128(1));
    let member_id = MemberId::new(5).unwrap();
    let rule_id = RuleId::from_uuid(Uuid::from_u128(2));
    let created = ModLogLinks {
        case_id: Some(case_id),
        member_id: Some(member_id),
        rule_id: Some(rule_id),
        ..ModLogLinks::default()
    };
    assert_eq!(created.check(ModLogKind::CaseCreated), Ok(()));
    assert_eq!(
        ModLogLinks {
            rule_id: None,
            ..created
        }
        .check(ModLogKind::CaseCreated),
        Err(LinkProblem::Missing {
            kind: ModLogKind::CaseCreated,
            link: Link::Rule
        })
    );
    assert_eq!(
        created.check(ModLogKind::SettingsChanged),
        Err(LinkProblem::Unexpected {
            kind: ModLogKind::SettingsChanged,
            link: Link::Case
        })
    );
    let voided = ModLogLinks {
        rule_id: None,
        ..created
    };
    assert_eq!(voided.check(ModLogKind::CaseVoided), Ok(()));
    let by_appeal = ModLogLinks {
        appeal_id: Some(AppealId::from_uuid(Uuid::from_u128(3))),
        ..voided
    };
    assert_eq!(by_appeal.check(ModLogKind::CaseVoided), Ok(()));
    let ticket_dm = ModLogLinks {
        member_id: Some(member_id),
        ticket_id: Some(TicketId::from_uuid(Uuid::from_u128(4))),
        ..ModLogLinks::default()
    };
    assert_eq!(ticket_dm.check(ModLogKind::DmSent), Ok(()));
    assert_eq!(
        ModLogLinks::default().check(ModLogKind::QuackJoined),
        Ok(())
    );
    assert_eq!(ModLogLinks::default().check(ModLogKind::Legacy), Ok(()));
    let action = ModLogLinks {
        action_id: Some(ActionId::from_uuid(Uuid::from_u128(5))),
        ..voided
    };
    assert_eq!(action.check(ModLogKind::ActionDone), Ok(()));
    let honeypot = ModLogLinks {
        member_id: Some(member_id),
        ..ModLogLinks::default()
    };
    assert_eq!(honeypot.check(ModLogKind::HoneypotCaseFailed), Ok(()));
}
