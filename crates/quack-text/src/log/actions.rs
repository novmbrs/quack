//! Lead sentences for case, action, and DM entries (§12.10).

use quack_core::action::{ActionFailure, ActionKind, ActionSkipReason, Failure};
use quack_core::case::CaseSource;
use quack_core::dm::{DmKind, DmState};
use quack_core::modlog::{
    ActionDoneDetail, ActionFailedDetail, CaseCreatedDetail, DmFailedDetail, DmFailedState,
    DmSentDetail, UndoSkippedDetail,
};
use quack_core::rule::LevelAction;

use super::LogFacts;
use crate::action;
use crate::dm::{self, DmAbout};
use crate::model::{Text, TimeStyle};
use crate::word;

fn level_action(
    kind: Option<ActionKind>,
    timeout: Option<u32>,
    delete: Option<u32>,
) -> Option<LevelAction> {
    match kind? {
        ActionKind::Timeout => Some(LevelAction::Timeout {
            duration_seconds: timeout.unwrap_or(0),
        }),
        ActionKind::Kick => Some(LevelAction::Kick),
        ActionKind::Ban => Some(LevelAction::Ban {
            delete_message_seconds: delete.unwrap_or(0),
        }),
        ActionKind::RemoveTimeout | ActionKind::Unban => None,
    }
}

pub(super) fn case_created(d: &CaseCreatedDetail, f: &LogFacts) -> Text {
    let member = |t: Text| f.member(t);
    let case = |t: Text, n: u64| f.case(t, n);
    let outcome = match level_action(
        d.action_kind,
        d.timeout_seconds,
        d.ban_delete_message_seconds,
    ) {
        None => "This case is a warning.".to_owned(),
        action => format!("Action for this case: {}.", word::level_outcome(action)),
    };
    let t = if d.source == CaseSource::Honeypot {
        let t = f
            .rule(member(case(Text::fixed("Quack added "), d.case_number).s(" for ")).s(" under "));
        member(t.s(" after ")).s(" posted in the honeypot channel.")
    } else {
        f.rule(
            member(case(Text::new().actor(&f.actor).s(" added "), d.case_number).s(" for "))
                .s(" under "),
        )
        .s(format!(
            " (level {}, case count {}).",
            d.level_index, d.case_count
        ))
    };
    let t = t.s(" ").s(outcome);
    match d.replaces_case_number {
        Some(n) => t.s(format!(" It replaces case #{n}.")),
        None => t,
    }
}

pub(super) fn action_done(d: &ActionDoneDetail, f: &LogFacts) -> Text {
    let member = |t: Text| f.member(t);
    let case = |t: Text, n: u64| f.case(t, n);
    let n = d.case_number;
    match (d.kind, d.no_change) {
        (ActionKind::Timeout, false) => match f.timeout_ends_at {
            Some(at) => case(
                member(Text::fixed("Quack set "))
                    .s("'s timeout to end ")
                    .time(at, TimeStyle::FullRelative)
                    .s(" for "),
                n,
            ),
            None => case(member(Text::fixed("Quack timed out ")).s(" for "), n),
        }
        .s("."),
        (ActionKind::Timeout, true) => {
            let t = member(Text::new()).s(" already had a timeout");
            let t = match f.timeout_ends_at {
                Some(at) => t.s(" ending at or after ").time(at, TimeStyle::Full),
                None => t,
            };
            case(t.s(", so Quack left it in place for "), n).s(".")
        }
        (ActionKind::Kick, false) => case(
            member(Text::fixed("Quack removed ")).s(" from the server for "),
            n,
        )
        .s("."),
        (ActionKind::Kick, true) => case(member(Text::new()).s(" left the server after "), n)
            .s(" was added, so Quack didn't remove them."),
        (ActionKind::Ban, false) => case(member(Text::fixed("Quack banned ")).s(" for "), n).s("."),
        (ActionKind::Ban, true) => case(
            member(Text::new()).s(" was already banned when Quack checked for "),
            n,
        )
        .s("."),
        (ActionKind::RemoveTimeout, false) => case(
            member(Text::fixed("Quack removed ")).s("'s timeout for "),
            n,
        )
        .s("."),
        (ActionKind::RemoveTimeout, true) => case(
            member(Text::new()).s(" had no active timeout when Quack checked for "),
            n,
        )
        .s("."),
        (ActionKind::Unban, false) => {
            case(member(Text::fixed("Quack unbanned ")).s(" for "), n).s(".")
        }
        (ActionKind::Unban, true) => case(
            member(Text::new()).s(" wasn't banned when Quack checked for "),
            n,
        )
        .s("."),
    }
}

pub(super) fn action_failed(d: &ActionFailedDetail, f: &LogFacts) -> Text {
    let case = |t: Text, n: u64| f.case(t, n);
    let n = d.case_number;
    let failure = Failure {
        reason: d.reason,
        permission: d.permission,
        http_status: d.http_status,
        discord_code: d.discord_code,
    };
    if d.reason == ActionFailure::Unconfirmed {
        let t = case(
            action::whether_applied(d.kind, f.member.as_ref()).s(" for "),
            n,
        )
        .s(".");
        return t.sentence(action::check(d.kind, f.member.as_ref()));
    }
    let reason = action::failure_reason(
        d.kind,
        &failure,
        f.member.as_ref(),
        d.attempts,
        d.may_have_applied,
    )
    .unwrap_or_default();
    // An attempt may have applied: the entry can't say it didn't happen.
    let start = if d.may_have_applied {
        action::whether_applied(d.kind, f.member.as_ref())
    } else {
        action::couldnt_phrase(d.kind, f.member.as_ref())
    };
    case(start.s(" for "), n).s(".").sentence(reason)
}

pub(super) fn undo_skipped(d: &UndoSkippedDetail, f: &LogFacts) -> Text {
    let member = |t: Text| f.member(t);
    let case = |t: Text, n: u64| f.case(t, n);
    let n = d.case_number;
    let ban = d.kind == ActionKind::Unban;
    let what = if ban { "ban" } else { "timeout" };
    let left = member(Text::fixed("Quack left ")).s(format!("'s {what} in place "));
    let t = match (d.skip_reason, d.still_applies_case_number) {
        (ActionSkipReason::OtherCaseApplies, Some(other)) => {
            case(left.s("after "), n).s(format!(" was voided because case #{other} still applies."))
        }
        (ActionSkipReason::OtherCaseApplies, None) => {
            case(left.s("after "), n).s(" was voided because another case still applies.")
        }
        (ActionSkipReason::ChangedOutsideCase, _) if ban => {
            case(left.s("because it may not have come from "), n).s(".")
        }
        (ActionSkipReason::ChangedOutsideCase, _) => {
            case(left.s("because it no longer matches "), n).s(".")
        }
    };
    match d.skip_reason {
        ActionSkipReason::ChangedOutsideCase => {
            t.sentence(action::check(d.kind, f.member.as_ref()))
        }
        ActionSkipReason::OtherCaseApplies => t,
    }
}

fn dm_about(kind: DmKind, f: &LogFacts) -> DmAbout {
    match kind {
        DmKind::Case => DmAbout::Case(f.case_number),
        DmKind::AppealUpdate => DmAbout::Appeal(f.case_number),
        DmKind::TicketClosed => DmAbout::Transcript,
    }
}

pub(super) fn dm_sent(d: DmSentDetail, f: &LogFacts) -> Text {
    dm::outcome_with(
        DmState::Sent,
        None,
        f.member.as_ref(),
        dm_about(d.dm_kind, f),
        &|t, n| f.case(t, n),
    )
}

pub(super) fn dm_failed(d: &DmFailedDetail, f: &LogFacts) -> Text {
    let state = match d.state {
        DmFailedState::Failed => DmState::Failed,
        DmFailedState::Unknown => DmState::Unknown,
    };
    dm::outcome_with(
        state,
        d.failure,
        f.member.as_ref(),
        dm_about(d.dm_kind, f),
        &|t, n| f.case(t, n),
    )
}
