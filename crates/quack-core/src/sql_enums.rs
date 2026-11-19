//! Checks every quack-core enum stored in PostgreSQL against the
//! `CREATE TYPE … AS ENUM` lines of the design (§15.1), so a renamed or
//! missing value fails here before it fails at runtime.

use std::collections::BTreeMap;

const DESIGN: &str = include_str!("../../../docs/design.md");

/// The enums of §15.1, by type name, parsed from the design.
fn design_enums() -> BTreeMap<String, Vec<String>> {
    let section = DESIGN
        .split("### 15.1 Enums")
        .nth(1)
        .and_then(|s| s.split("```sql").nth(1))
        .and_then(|s| s.split("```").next())
        .unwrap_or_default();
    section
        .split(';')
        .filter_map(|statement| {
            let statement = statement.trim();
            let rest = statement.strip_prefix("CREATE TYPE")?;
            let (name, values) = rest.split_once("AS ENUM")?;
            let values = values
                .trim()
                .trim_start_matches('(')
                .trim_end_matches(')')
                .split(',')
                .map(|v| v.trim().trim_matches('\'').to_owned())
                .collect();
            Some((name.trim().to_owned(), values))
        })
        .collect()
}

macro_rules! rust_enums {
    ($($sql:literal => $ty:ty,)+) => {
        fn rust_enums() -> Vec<(&'static str, Vec<&'static str>)> {
            vec![$(($sql, <$ty>::ALL.iter().map(|v| v.as_str()).collect()),)+]
        }
    };
}

rust_enums! {
    "case_source" => crate::case::CaseSource,
    "void_cause" => crate::case::VoidCause,
    "action_kind" => crate::action::ActionKind,
    "action_state" => crate::action::ActionState,
    "action_failure" => crate::action::ActionFailure,
    "action_skip_reason" => crate::action::ActionSkipReason,
    "attempt_outcome" => crate::action::AttemptOutcome,
    "discord_permission" => crate::permissions::DiscordPermission,
    "dm_kind" => crate::dm::DmKind,
    "dm_state" => crate::dm::DmState,
    "dm_failure" => crate::dm::DmFailure,
    "evidence_state" => crate::evidence::EvidenceState,
    "evidence_failure" => crate::evidence::EvidenceFailure,
    "file_copy_state" => crate::evidence::FileCopyState,
    "appeal_state" => crate::appeal::AppealState,
    "appeal_event_kind" => crate::appeal::AppealEventKind,
    "mod_log_kind" => crate::modlog::ModLogKind,
    "mod_log_result" => crate::modlog::ModLogResult,
    "mod_log_source" => crate::modlog::ModLogSource,
    "message_kind" => crate::messages::MessageKind,
    "message_state" => crate::messages::MessageState,
    "ticket_state" => crate::modules::TicketState,
    "ticket_close_reason" => crate::modules::TicketCloseReason,
    "transcript_partial" => crate::modules::TranscriptPartial,
    "module_disabled_reason" => crate::modules::ModuleDisabledReason,
    "activity_event" => crate::modules::ActivityEvent,
    "honeypot_hit_state" => crate::modules::HoneypotHitState,
}

/// Enum types of §15.1 that quack-core does not own.
const ELSEWHERE: [&str; 1] = [
    // The job queue's kinds belong to quack-db (and quack-telemetry's labels).
    "job_kind",
];

#[test]
fn every_stored_enum_matches_the_design_exactly() {
    let design = design_enums();
    assert!(design.len() > 20, "could not parse §15.1: {design:?}");
    let rust = rust_enums();
    for (name, values) in &rust {
        let expected = design
            .get(*name)
            .unwrap_or_else(|| panic!("{name} is not in §15.1"));
        assert_eq!(values, expected, "{name}");
    }
    for name in design.keys() {
        assert!(
            rust.iter().any(|(n, _)| n == name) || ELSEWHERE.contains(&name.as_str()),
            "§15.1 enum {name} has no quack-core type"
        );
    }
}
