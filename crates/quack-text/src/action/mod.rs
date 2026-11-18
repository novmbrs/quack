//! Action sentences (§12.6): the one place that turns an action's kind,
//! state, and failure into words. The staff card, the receipts, the log
//! channel, and the API all use [`sentence`]; the member's dashboard uses
//! [`member_sentence`]. A failure reason says what is wrong and where to
//! fix it, never "retry": the card and the log post offer Retry only when
//! it is allowed, and say so next to the button.

mod controls;
mod member;

use quack_core::action::{ActionFailure, ActionKind, ActionSkipReason, ActionState, Failure};
use time::OffsetDateTime;

pub use crate::access::permission;
pub use controls::{
    Undo, dismiss_done, retry_done, undo_button, undo_confirm, undo_from_original, undo_queued,
};
pub(crate) use controls::{failure_noun, retry_noun};
pub use member::{member_none, member_sentence};

use crate::access::{self, Reader};
use crate::fmt;
use crate::model::{Actor, Person, Text, TimeStyle};
use crate::word::Outcome;

/// What an action is and where it stands: the facts every action sentence
/// is built from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionView {
    /// What it does.
    pub kind: ActionKind,
    /// Where it stands.
    pub state: ActionState,
    /// Why it failed, when `failed`.
    pub failure: Option<Failure>,
    /// Why an undo left things in place, when `skipped`.
    pub skip: Option<ActionSkipReason>,
    /// For `other_case_applies`: the number of the case that still applies.
    pub still_applies_case: Option<u64>,
    /// `done` because the desired state already held.
    pub no_change: bool,
    /// Attempts in the current round.
    pub attempts: u32,
    /// When the next retry is due, while `pending` after an attempt.
    pub next_attempt_at: Option<OffsetDateTime>,
    /// A timeout's fixed end (the original's, for a remove timeout).
    pub timeout_ends_at: Option<OffsetDateTime>,
    /// How much of the member's recent messages a ban deletes, in seconds.
    pub ban_delete_message_seconds: Option<u32>,
    /// The number of the action's own case.
    pub case_number: u64,
    /// Who dismissed the failure, and when.
    pub dismissed: Option<(Actor, OffsetDateTime)>,
    /// Some attempt may have applied (§5.2,
    /// `quack_core::action::may_have_applied` over all its attempts), so a
    /// later refusal or a void can't say the action didn't happen.
    pub may_have_applied: bool,
}

impl ActionView {
    /// A fresh action: `pending`, no attempts, nothing set.
    pub const fn new(kind: ActionKind, state: ActionState, case_number: u64) -> Self {
        Self {
            kind,
            state,
            failure: None,
            skip: None,
            still_applies_case: None,
            no_change: false,
            attempts: 0,
            next_attempt_at: None,
            timeout_ends_at: None,
            ban_delete_message_seconds: None,
            case_number,
            dismissed: None,
            may_have_applied: false,
        }
    }

    /// Whether Quack can't tell if Discord applied it: the last round ended
    /// unconfirmed, or an earlier attempt may have applied before a later
    /// refusal or a void.
    fn uncertain(&self) -> bool {
        match self.state {
            ActionState::Failed => {
                self.may_have_applied
                    || self
                        .failure
                        .is_some_and(|f| f.reason == ActionFailure::Unconfirmed)
            }
            ActionState::Cancelled => self.may_have_applied,
            ActionState::Pending
            | ActionState::Running
            | ActionState::Done
            | ActionState::Skipped => false,
        }
    }
}

/// How much of the sentence to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detail {
    /// Staff views: the failure reason, the check to make, the retry time,
    /// and who dismissed it.
    Full,
    /// The public receipt (§11.7): the bare outcome, nothing staff-only.
    Receipt,
}

/// The outcome badge for an action (`word.outcome.*`).
pub fn outcome(a: &ActionView) -> Outcome {
    if a.uncertain() {
        return Outcome::CouldntConfirm;
    }
    match a.state {
        ActionState::Pending | ActionState::Running => Outcome::InProgress,
        ActionState::Done => Outcome::Done,
        ActionState::Failed => Outcome::Failed,
        ActionState::Cancelled => Outcome::Cancelled,
        ActionState::Skipped => Outcome::LeftInPlace,
    }
}

/// `action.none`.
pub fn none() -> Text {
    Text::fixed("This case is a warning.")
}

/// The member, or "the member" when Quack can't name them.
fn who(t: Text, member: Option<&Person>) -> Text {
    match member {
        Some(m) => t.person(m),
        None => t.s("the member"),
    }
}

/// What the action does to the member, after a verb's tense is chosen:
/// `(past, present)` such as ("banned @Sam", "ban @Sam").
fn does(kind: ActionKind, member: Option<&Person>, past: bool) -> Text {
    let t = Text::new();
    match (kind, past) {
        (ActionKind::Timeout, true) => who(t.s("timed out "), member),
        (ActionKind::Timeout, false) => who(t.s("time out "), member),
        (ActionKind::Kick, true) => who(t.s("removed "), member).s(" from the server"),
        (ActionKind::Kick, false) => who(t.s("remove "), member).s(" from the server"),
        (ActionKind::Ban, true) => who(t.s("banned "), member),
        (ActionKind::Ban, false) => who(t.s("ban "), member),
        (ActionKind::RemoveTimeout, true) => who(t.s("removed "), member).s("'s timeout"),
        (ActionKind::RemoveTimeout, false) => who(t.s("remove "), member).s("'s timeout"),
        (ActionKind::Unban, true) => who(t.s("unbanned "), member),
        (ActionKind::Unban, false) => who(t.s("unban "), member),
    }
}

/// `action.retry_scheduled`: the whole line of a pending action whose last
/// attempt didn't go through.
pub fn retry_scheduled(kind: ActionKind, member: &Person, at: OffsetDateTime) -> Text {
    Text::fixed("Quack will try to ")
        .then(does(kind, Some(member), false))
        .s(" again ")
        .time(at, TimeStyle::Relative)
        .s(".")
}

/// `action.unconfirmed`: "Quack couldn't confirm whether Discord banned
/// @Sam.", then (staff views) where to check.
pub fn unconfirmed(kind: ActionKind, member: &Person, detail: Detail) -> Text {
    let first = whether_applied(kind, Some(member)).s(".");
    match detail {
        Detail::Full => first.sentence(check(kind, Some(member))),
        Detail::Receipt => first,
    }
}

/// `action.check.*`: where in Discord staff can see whether an action
/// happened.
pub(crate) fn check(kind: ActionKind, member: Option<&Person>) -> Text {
    match kind {
        ActionKind::Timeout | ActionKind::RemoveTimeout => match member {
            Some(m) => Text::fixed("Check ").person(m).s("'s timeout in Discord."),
            None => Text::fixed("Check the member's timeout in Discord."),
        },
        ActionKind::Kick => {
            Text::fixed("Check Server Settings → Audit Log in Discord for the kick.")
        }
        ActionKind::Ban | ActionKind::Unban => {
            Text::fixed("Check Server Settings → Bans in Discord.")
        }
    }
}

/// "Quack couldn't confirm whether Discord banned @Sam", the start of
/// every uncertain sentence.
pub(crate) fn whether_applied(kind: ActionKind, member: Option<&Person>) -> Text {
    Text::fixed("Quack couldn't confirm whether Discord ").then(does(kind, member, true))
}

/// "Quack couldn't ban @Sam", the start of every failed sentence.
pub(crate) fn couldnt_phrase(kind: ActionKind, member: Option<&Person>) -> Text {
    Text::fixed("Quack couldn't ").then(does(kind, member, false))
}

/// `action.dismissed_suffix`.
pub fn dismissed_suffix(by: &Actor, at: OffsetDateTime) -> Text {
    let who = match by {
        Actor::UnknownModerator => Text::fixed("A moderator"),
        by => Text::new().actor(by),
    };
    who.s(" dismissed this failure ")
        .time(at, TimeStyle::Relative)
        .s(". The case keeps its history.")
}

/// `action.failure.*`: why an action failed and where to fix it, as the
/// `{reason}` sentences. `None` for `unconfirmed`, which has its own whole
/// sentence ([`unconfirmed`]). `may_have_applied` drops the claim that
/// Discord didn't apply it.
///
/// Without a member (a log entry about someone Quack can't name) the
/// sentence says "the member".
pub fn failure_reason(
    kind: ActionKind,
    failure: &Failure,
    member: Option<&Person>,
    attempts: u32,
    may_have_applied: bool,
) -> Option<Text> {
    let who = |t: Text| match member {
        Some(m) => t.person(m),
        None => t.s("The member"),
    };
    let t = Text::new();
    Some(match failure.reason {
        ActionFailure::QuackNotInServer => access::quack_not_in_server(),
        ActionFailure::QuackMissingPermission => access::quack_role_needs(
            &[failure.permission.unwrap_or(kind.permission()).permission()],
            Reader::Staff,
        ),
        ActionFailure::QuackRoleTooLow => access::quack_role_too_low(member),
        ActionFailure::MemberIsOwner => who(t)
            .s(" owns this server. Discord doesn't allow this action on the server owner."),
        ActionFailure::MemberNotInServer => match kind {
            ActionKind::Timeout => who(t).s(" isn't in the server, so Discord can't time them out."),
            _ => who(t).s(" isn't in the server."),
        },
        ActionFailure::MemberIsAdministrator => who(t).s(
            " has Administrator, and Discord doesn't allow timeouts for members with Administrator.",
        ),
        ActionFailure::TimeoutWindowPassed => {
            t.s("The timeout would already have ended, so Quack didn't apply it.")
        }
        ActionFailure::DiscordRefused => t.s(
            "Discord didn't allow it. Check Quack's role and permissions in Server Settings → Roles.",
        ),
        ActionFailure::RetriesExhausted => {
            let t = if attempts <= 1 {
                t.s("Quack stopped after one attempt.")
            } else {
                t.s(format!("Quack stopped trying after {} attempts.", fmt::count(attempts)))
            };
            if may_have_applied {
                t
            } else {
                t.s(" Discord didn't apply it.")
            }
        }
        ActionFailure::Unconfirmed => return None,
    })
}

/// Appends " until" and the end time, or nothing when the end isn't known.
fn until(a: &ActionView, t: Text) -> Text {
    match a.timeout_ends_at {
        Some(at) => t.s(" until ").time(at, TimeStyle::FullRelative),
        None => t,
    }
}

fn pending(a: &ActionView, m: &Person) -> Text {
    let t = Text::fixed("Quack will ").then(does(a.kind, Some(m), false));
    if a.kind == ActionKind::Timeout {
        until(a, t).s(".")
    } else {
        t.s(".")
    }
}

fn running(a: &ActionView, m: &Person) -> Text {
    let t = Text::new();
    match a.kind {
        ActionKind::Timeout => until(a, t.s("Quack is timing out ").person(m)).s("."),
        ActionKind::Kick => t.s("Quack is removing ").person(m).s(" from the server."),
        ActionKind::Ban => t.s("Quack is banning ").person(m).s("."),
        ActionKind::RemoveTimeout => t.s("Quack is removing ").person(m).s("'s timeout."),
        ActionKind::Unban => t.s("Quack is unbanning ").person(m).s("."),
    }
}

fn done(a: &ActionView, m: &Person) -> Text {
    let t = Text::new();
    match (a.kind, a.no_change) {
        (ActionKind::Timeout, false) => match a.timeout_ends_at {
            Some(at) => t
                .s("Quack set ")
                .person(m)
                .s("'s timeout to end ")
                .time(at, TimeStyle::FullRelative)
                .s("."),
            None => t.s("Quack timed out ").person(m).s("."),
        },
        (ActionKind::Timeout, true) => {
            let t = t.person(m).s(" already had a timeout");
            match a.timeout_ends_at {
                Some(at) => t
                    .s(" ending at or after ")
                    .time(at, TimeStyle::Full)
                    .s(". Quack left it in place."),
                None => t.s(". Quack left it in place."),
            }
        }
        (ActionKind::Kick, false) => t.s("Quack removed ").person(m).s(" from the server."),
        (ActionKind::Kick, true) => t
            .person(m)
            .s(" left the server after this case was added, so Quack didn't remove them."),
        (ActionKind::Ban, false) => match a.ban_delete_message_seconds {
            Some(secs) if secs > 0 => t
                .s("Quack banned ")
                .person(m)
                .s(" and deleted their messages from the last ")
                .s(fmt::last(u64::from(secs)))
                .s("."),
            _ => t.s("Quack banned ").person(m).s("."),
        },
        (ActionKind::Ban, true) => t.person(m).s(" was already banned when Quack checked."),
        (ActionKind::RemoveTimeout, false) => t.s("Quack removed ").person(m).s("'s timeout."),
        (ActionKind::RemoveTimeout, true) => {
            t.person(m).s(" had no active timeout when Quack checked.")
        }
        (ActionKind::Unban, false) => t.s("Quack unbanned ").person(m).s("."),
        (ActionKind::Unban, true) => t.person(m).s(" wasn't banned when Quack checked."),
    }
}

/// `action.<kind>.cancelled`: the case was voided before any attempt could
/// apply. Undo actions are never cancelled; their wording only keeps the
/// match total.
fn cancelled(a: &ActionView, m: &Person) -> Text {
    Text::fixed(format!("Case #{} was voided before Quack ", a.case_number))
        .then(does(a.kind, Some(m), true))
        .s(".")
}

/// `action.cancelled_uncertain`: voided after an attempt that may have
/// applied. Staff views add what happens next.
fn cancelled_uncertain(a: &ActionView, m: &Person, full: bool) -> Text {
    let t = Text::fixed(format!("Case #{} was voided. ", a.case_number))
        .then(whether_applied(a.kind, Some(m)))
        .s(". Quack won't try again.");
    if !full {
        return t;
    }
    t.s(match a.kind {
        ActionKind::Timeout => " Quack will check whether the timeout can be removed.",
        ActionKind::Ban => " Quack will check whether the ban can be removed.",
        ActionKind::Kick => " A kick can't be undone.",
        ActionKind::RemoveTimeout | ActionKind::Unban => "",
    })
}

fn skipped(a: &ActionView, m: &Person) -> Text {
    let what = match a.kind {
        ActionKind::RemoveTimeout => "'s timeout in place because ",
        ActionKind::Unban => "'s ban in place because ",
        // Originals are never skipped; this only keeps the match total.
        ActionKind::Timeout | ActionKind::Kick | ActionKind::Ban => return cancelled(a, m),
    };
    let t = Text::fixed("Quack left ").person(m).s(what);
    match (a.skip, a.kind) {
        (Some(ActionSkipReason::ChangedOutsideCase), ActionKind::Unban) => t
            .s(format!(
                "it may not have come from case #{}.",
                a.case_number
            ))
            .sentence(check(a.kind, Some(m))),
        (Some(ActionSkipReason::ChangedOutsideCase), _) => t
            .s(format!("it no longer matches case #{}.", a.case_number))
            .sentence(check(a.kind, Some(m))),
        (Some(ActionSkipReason::OtherCaseApplies) | None, _) => match a.still_applies_case {
            Some(n) => t.s(format!("case #{n} still applies.")),
            None => t.s("another case still applies."),
        },
    }
}

/// The action sentence (§12.6) for staff views and receipts.
pub fn sentence(a: &ActionView, member: &Person, detail: Detail) -> Text {
    let full = detail == Detail::Full;
    match a.state {
        ActionState::Pending => match a.next_attempt_at {
            Some(at) if full && a.attempts > 0 => retry_scheduled(a.kind, member, at),
            _ => pending(a, member),
        },
        ActionState::Running => running(a, member),
        ActionState::Done => done(a, member),
        ActionState::Failed => {
            let reason = a
                .failure
                .filter(|_| full)
                .and_then(|f| {
                    failure_reason(a.kind, &f, Some(member), a.attempts, a.may_have_applied)
                })
                .unwrap_or_default();
            let base = if a.uncertain() {
                unconfirmed(a.kind, member, detail).sentence(reason)
            } else {
                couldnt_phrase(a.kind, Some(member)).s(".").sentence(reason)
            };
            match (&a.dismissed, full) {
                (Some((by, at)), true) => base.sentence(dismissed_suffix(by, at.to_owned())),
                _ => base,
            }
        }
        ActionState::Cancelled if a.uncertain() => cancelled_uncertain(a, member, full),
        ActionState::Cancelled => cancelled(a, member),
        ActionState::Skipped => skipped(a, member),
    }
}
