//! Lead sentences for case, action, and DM entries (§12.10).

use quack_core::action::{ActionFailure, ActionKind, ActionSkipReason, Failure};
use quack_core::case::CaseSource;
use quack_core::dm::DmKind;
use quack_core::modlog::{
    ActionDoneDetail, ActionFailedDetail, CaseCreatedDetail, DmFailedDetail, DmFailedState,
    DmSentDetail, UndoSkippedDetail,
};
use quack_core::rule::LevelAction;

use super::LogFacts;
use crate::action;
use crate::dm::because;
use crate::fmt;
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

fn ends(f: &LogFacts, t: Text) -> Text {
    match f.timeout_ends_at {
        Some(at) => t.s(" until ").time(at, TimeStyle::FullRelative),
        None => t,
    }
}

fn dm_case(f: &LogFacts, t: Text) -> Text {
    match f.case_number {
        Some(n) => f.case(t, n),
        None => t.s("their case"),
    }
}

pub(super) fn case_created(d: &CaseCreatedDetail, f: &LogFacts) -> Text {
    let actor = || Text::new().actor(&f.actor);
    let member = |t: Text| f.member(t);
    let case = |t: Text, n: u64| f.case(t, n);
    {
        let outcome = word::level_outcome(level_action(
            d.action_kind,
            d.timeout_seconds,
            d.ban_delete_message_seconds,
        ));
        let t = if d.source == CaseSource::Honeypot {
            f.rule(
                case(
                    member(case(Text::fixed("Quack added "), d.case_number).s(" for ")),
                    d.case_number,
                )
                .s(" under "),
            )
            .s(format!(
                " because they posted in the honeypot channel: {outcome}."
            ))
        } else {
            f.rule(member(case(actor().s(" added "), d.case_number).s(" for ")).s(" under "))
                .s(format!(
                    ": {}, case count {}, {outcome}.",
                    fmt::level(d.level_index, d.level_from_case_count),
                    d.case_count
                ))
        };
        match d.replaces_case_number {
            Some(n) => t.s(format!(" It replaces case #{n}.")),
            None => t,
        }
    }
}

pub(super) fn action_done(d: &ActionDoneDetail, f: &LogFacts) -> Text {
    let member = |t: Text| f.member(t);
    let case = |t: Text, n: u64| f.case(t, n);
    {
        let n = d.case_number;
        match (d.kind, d.no_change) {
            (ActionKind::Timeout, false) => case(
                ends(f, member(Text::fixed("Quack timed out "))).s(" for "),
                n,
            )
            .s("."),
            (ActionKind::Timeout, true) => case(
                ends(f, member(Text::new()).s(" was already timed out"))
                    .s(" or later when Quack tried to time them out for "),
                n,
            )
            .s("."),
            (ActionKind::Kick, false) => case(
                member(Text::fixed("Quack removed ")).s(" from the server for "),
                n,
            )
            .s("."),
            (ActionKind::Kick, true) => case(
                member(Text::new()).s(" had already left when Quack tried to remove them for "),
                n,
            )
            .s("."),
            (ActionKind::Ban, false) => {
                case(member(Text::fixed("Quack banned ")).s(" for "), n).s(".")
            }
            (ActionKind::Ban, true) => case(
                member(Text::new()).s(" was already banned when Quack tried to ban them for "),
                n,
            )
            .s("."),
            (ActionKind::RemoveTimeout, false) => case(
                member(Text::fixed("Quack removed ")).s("'s timeout from "),
                n,
            )
            .s("."),
            (ActionKind::RemoveTimeout, true) => {
                case(member(Text::new()).s("'s timeout from "), n).s(" had already ended.")
            }
            (ActionKind::Unban, false) => case(
                member(Text::fixed("Quack unbanned ")).s(". The ban came from "),
                n,
            )
            .s("."),
            (ActionKind::Unban, true) => case(
                member(Text::new())
                    .s(" wasn't banned anymore when Quack tried to undo the ban from "),
                n,
            )
            .s("."),
        }
    }
}

pub(super) fn action_failed(d: &ActionFailedDetail, f: &LogFacts) -> Text {
    let member = |t: Text| f.member(t);
    let case = |t: Text, n: u64| f.case(t, n);
    {
        let n = d.case_number;
        if d.reason == ActionFailure::Unconfirmed {
            let t = action::whether_applied(d.kind, f.member.as_ref()).s(" for ");
            return member(case(t, n).s(". Check ")).s(" in Discord before you retry.");
        }
        let failure = Failure {
            reason: d.reason,
            permission: d.permission,
            http_status: d.http_status,
            discord_code: d.discord_code,
        };
        let reason = action::failure_reason(d.kind, &failure, f.member.as_ref(), d.attempts)
            .unwrap_or_default();
        case(
            action::couldnt_phrase(d.kind, f.member.as_ref()).s(" for "),
            n,
        )
        .s(".")
        .sentence(reason)
    }
}

pub(super) fn undo_skipped(d: &UndoSkippedDetail, f: &LogFacts) -> Text {
    let member = |t: Text| f.member(t);
    let case = |t: Text, n: u64| f.case(t, n);
    {
        let n = d.case_number;
        let ban = d.kind == ActionKind::Unban;
        let what = if ban { "ban" } else { "timeout" };
        let left = member(Text::fixed("Quack left ")).s(format!("'s {what} in place "));
        match (d.skip_reason, d.still_applies_case_number) {
            (ActionSkipReason::OtherCaseApplies, Some(other)) => case(
                left.s(format!("because case #{other} still applies, even though ")),
                n,
            )
            .s(" was voided."),
            (ActionSkipReason::OtherCaseApplies, None) => {
                case(left.s("because another case still applies, even though "), n).s(" was voided.")
            }
            (ActionSkipReason::ChangedOutsideCase, _) if ban => case(left.s("after "), n)
                .s(" was voided, because the ban didn't come from a voided Quack case. Check it in Discord."),
            (ActionSkipReason::ChangedOutsideCase, _) => case(left.s("after "), n)
                .s(" was voided, because someone changed the timeout outside the case. Check it in Discord."),
        }
    }
}

pub(super) fn dm_sent(d: DmSentDetail, f: &LogFacts) -> Text {
    let member = |t: Text| f.member(t);
    match d.dm_kind {
        DmKind::Case => dm_case(f, member(Text::fixed("Quack sent ")).s(" a DM about ")).s("."),
        DmKind::AppealUpdate => dm_case(
            f,
            member(Text::fixed("Quack sent ")).s(" a DM about their appeal for "),
        )
        .s("."),
        DmKind::TicketClosed => {
            member(Text::fixed("Quack sent ")).s(" the transcript of their ticket.")
        }
    }
}

pub(super) fn dm_failed(d: &DmFailedDetail, f: &LogFacts) -> Text {
    let member = |t: Text| f.member(t);
    {
        let why = because(
            d.failure
                .unwrap_or(quack_core::dm::DmFailure::DiscordRefused),
        );
        match (d.dm_kind, d.state) {
            (DmKind::Case, DmFailedState::Failed) => dm_case(f, member(Text::fixed("Quack couldn't send ")).s(" a DM about "))
                .s(format!(" {why}. They can still see the case by signing in to the dashboard.")),
            (DmKind::Case, DmFailedState::Unknown) => dm_case(
                f,
                member(Text::fixed("Quack couldn't confirm whether Discord accepted the DM to ")).s(" about "),
            )
            .s(", and won't send it again so it can't arrive twice. They can still see the case by signing in to the dashboard."),
            (DmKind::AppealUpdate, DmFailedState::Failed) => dm_case(
                f,
                member(Text::fixed("Quack couldn't send ")).s(" a DM about their appeal for "),
            )
            .s(format!(" {why}. They can still see the decision on the dashboard.")),
            (DmKind::AppealUpdate, DmFailedState::Unknown) => dm_case(
                f,
                member(Text::fixed("Quack couldn't confirm whether Discord accepted the DM to "))
                    .s(" about their appeal for "),
            )
            .s(". They can still see the decision on the dashboard."),
            (DmKind::TicketClosed, DmFailedState::Failed) => member(Text::fixed("Quack couldn't send "))
                .s(format!(" the transcript of their ticket {why}. Staff can read it on the dashboard's ticket page.")),
            (DmKind::TicketClosed, DmFailedState::Unknown) => member(Text::fixed(
                "Quack couldn't confirm whether Discord accepted the transcript DM to ",
            ))
            .s(". Staff can read it on the dashboard's ticket page."),
        }
    }
}
