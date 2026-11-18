//! Action sentences (§12.6): the one place that turns an action's kind,
//! state, and failure into words. The staff card, the receipts, the log
//! channel, and the API all use [`sentence`]; the member's dashboard uses
//! [`member_sentence`].

mod controls;
mod member;

use quack_core::action::{ActionFailure, ActionKind, ActionSkipReason, ActionState, Failure};
use quack_core::permissions::Permission;
use time::OffsetDateTime;

pub use controls::{
    Undo, dismiss_done, retry_done, undo_button, undo_confirm, undo_from_original, undo_queued,
};
pub use member::{member_none, member_sentence};

use crate::fmt;
use crate::model::{Actor, Person, Text, TimeStyle};
use crate::word::{self, Outcome};

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
        }
    }

    fn unconfirmed(&self) -> bool {
        self.state == ActionState::Failed
            && self
                .failure
                .is_some_and(|f| f.reason == ActionFailure::Unconfirmed)
    }
}

/// How much of the sentence to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detail {
    /// Staff views: the failure reason, the retry time, and who dismissed it.
    Full,
    /// The public receipt (§11.7): the bare outcome, nothing staff-only.
    Receipt,
}

/// The outcome badge for an action (`word.outcome.*`).
pub fn outcome(a: &ActionView) -> Outcome {
    match a.state {
        ActionState::Pending | ActionState::Running => Outcome::InProgress,
        ActionState::Done => Outcome::Done,
        ActionState::Failed if a.unconfirmed() => Outcome::CouldntConfirm,
        ActionState::Failed => Outcome::Failed,
        ActionState::Cancelled => Outcome::Cancelled,
        ActionState::Skipped => Outcome::LeftInPlace,
    }
}

/// `action.none`.
pub fn none() -> Text {
    Text::fixed("No action. This case is a warning.")
}

/// `action.retry_scheduled`.
pub fn retry_scheduled(at: OffsetDateTime) -> Text {
    Text::fixed("The last try didn't go through or wasn't confirmed, so Quack will try again ")
        .time(at, TimeStyle::Relative)
        .s(".")
}

/// `action.unconfirmed`: the result of the last attempts is unknown.
pub fn unconfirmed(kind: ActionKind, member: &Person, detail: Detail) -> Text {
    let first = whether_applied(kind, Some(member)).s(".");
    match detail {
        Detail::Full => first
            .s(" Check ")
            .person(member)
            .s(" in Discord before you retry."),
        Detail::Receipt => first,
    }
}

/// "Quack couldn't confirm whether Discord applied the ban to @Sam", the
/// start of every unconfirmed sentence; undos say what they undo.
pub(crate) fn whether_applied(kind: ActionKind, member: Option<&Person>) -> Text {
    let who = |t: Text| match member {
        Some(m) => t.person(m),
        None => t.s("the member"),
    };
    let t = Text::fixed("Quack couldn't confirm whether Discord ");
    match kind {
        ActionKind::Timeout | ActionKind::Kick | ActionKind::Ban => {
            who(t.s(format!("applied the {} to ", word::action(kind))))
        }
        ActionKind::RemoveTimeout => who(t.s("removed ")).s("'s timeout"),
        ActionKind::Unban => who(t.s("unbanned ")),
    }
}

/// "Quack couldn't ban @Sam", the start of every failed sentence.
pub(crate) fn couldnt_phrase(kind: ActionKind, member: Option<&Person>) -> Text {
    let who = |t: Text| match member {
        Some(m) => t.person(m),
        None => t.s("the member"),
    };
    let t = Text::new();
    match kind {
        ActionKind::Timeout => who(t.s("Quack couldn't time out ")),
        ActionKind::Kick => who(t.s("Quack couldn't remove ")).s(" from the server"),
        ActionKind::Ban => who(t.s("Quack couldn't ban ")),
        ActionKind::RemoveTimeout => who(t.s("Quack couldn't remove ")).s("'s timeout"),
        ActionKind::Unban => who(t.s("Quack couldn't unban ")),
    }
}

/// `action.dismissed_suffix`.
pub fn dismissed_suffix(by: &Actor, at: OffsetDateTime) -> Text {
    Text::fixed("Dismissed by ")
        .actor(by)
        .s(" ")
        .time(at, TimeStyle::Relative)
        .s(".")
}

/// The Discord UI name of a permission (`{permission}`).
pub const fn permission(p: Permission) -> &'static str {
    match p {
        Permission::KickMembers => "Kick Members",
        Permission::BanMembers => "Ban Members",
        Permission::Administrator => "Administrator",
        Permission::ManageChannels => "Manage Channels",
        Permission::ManageGuild => "Manage Server",
        Permission::ViewAuditLog => "View Audit Log",
        Permission::ViewChannel => "View Channel",
        Permission::SendMessages => "Send Messages",
        Permission::ManageMessages => "Manage Messages",
        Permission::EmbedLinks => "Embed Links",
        Permission::AttachFiles => "Attach Files",
        Permission::ReadMessageHistory => "Read Message History",
        Permission::ManageThreads => "Manage Threads",
        Permission::CreatePublicThreads => "Create Public Threads",
        Permission::CreatePrivateThreads => "Create Private Threads",
        Permission::SendMessagesInThreads => "Send Messages in Threads",
        Permission::ModerateMembers => "Moderate Members",
    }
}

/// `action.failure.*`: why an action failed, as the `{reason}` sentence.
/// `None` for `unconfirmed`, which has its own whole sentence
/// ([`unconfirmed`]) so one sentence never says both "couldn't" and
/// "couldn't confirm".
///
/// Without a member (a log entry about someone Quack can't name) the
/// sentence says "the member".
pub fn failure_reason(
    kind: ActionKind,
    failure: &Failure,
    member: Option<&Person>,
    attempts: u32,
) -> Option<Text> {
    let t = Text::new();
    let who = |t: Text| match member {
        Some(m) => t.person(m),
        None => t.s("The member"),
    };
    Some(match failure.reason {
        ActionFailure::QuackNotInServer => t.s("Quack isn't in this server anymore."),
        ActionFailure::QuackMissingPermission => t
            .s("Quack needs the ")
            .s(permission(failure.permission.unwrap_or(kind.permission()).permission()))
            .s(" permission. Ask an administrator to give it to Quack's role, then retry."),
        ActionFailure::QuackRoleTooLow => who(t).s(
            "'s highest role is at or above Quack's. Ask an administrator to move Quack's role above it, then retry.",
        ),
        ActionFailure::MemberIsOwner => who(t).s(
            " owns the server, and Discord doesn't let anyone time out, kick, or ban the owner.",
        ),
        ActionFailure::MemberNotInServer => who(t)
            .s(" isn't in the server, so there's no one to time out."),
        ActionFailure::MemberIsAdministrator => who(t).s(
            " has the Administrator permission, and Discord doesn't allow timeouts for administrators.",
        ),
        ActionFailure::TimeoutWindowPassed => {
            t.s("The timeout would already have ended by now, so Quack didn't apply it.")
        }
        ActionFailure::DiscordRefused => {
            t.s("Discord didn't let Quack do this. Check Quack's role and permissions, then retry.")
        }
        ActionFailure::RetriesExhausted => t
            .s("Discord turned Quack away ")
            .s(fmt::plural(u64::from(attempts), "time", "times"))
            .s(", so nothing changed. Retry when Discord is available again."),
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

fn in_progress(a: &ActionView, m: &Person) -> Text {
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
                .person(m)
                .s(" is timed out until ")
                .time(at, TimeStyle::FullRelative)
                .s("."),
            None => t.s("Quack timed out ").person(m).s("."),
        },
        (ActionKind::Timeout, true) => match a.timeout_ends_at {
            Some(at) => t
                .person(m)
                .s(" was already timed out until ")
                .time(at, TimeStyle::FullRelative)
                .s(" or later, so Quack left that timeout as it was."),
            None => t
                .person(m)
                .s(" was already timed out, so Quack left that timeout as it was."),
        },
        (ActionKind::Kick, false) => t.s("Quack removed ").person(m).s(" from the server."),
        (ActionKind::Kick, true) => t
            .person(m)
            .s(" had already left the server, so Quack didn't remove them."),
        (ActionKind::Ban, false) => match a.ban_delete_message_seconds {
            Some(secs) if secs > 0 => t
                .s("Quack banned ")
                .person(m)
                .s(" and deleted their messages from the last ")
                .s(fmt::last(u64::from(secs)))
                .s("."),
            _ => t.s("Quack banned ").person(m).s("."),
        },
        (ActionKind::Ban, true) => t.person(m).s(" was already banned."),
        (ActionKind::RemoveTimeout, false) => t.s("Quack removed ").person(m).s("'s timeout."),
        (ActionKind::RemoveTimeout, true) => t.person(m).s("'s timeout had already ended."),
        (ActionKind::Unban, false) => t.s("Quack unbanned ").person(m).s("."),
        (ActionKind::Unban, true) => t
            .person(m)
            .s(" wasn't banned anymore, so there was nothing to undo."),
    }
}

/// `action.<kind>.failed` without its reason: "Quack couldn't ban @Sam."
fn couldnt(kind: ActionKind, m: &Person) -> Text {
    couldnt_phrase(kind, Some(m)).s(".")
}

/// `action.<kind>.cancelled`. Undo actions are never cancelled; their
/// wording here only keeps the match total.
fn cancelled(kind: ActionKind, m: &Person) -> Text {
    let t = Text::new();
    let because = " because the case was voided first.";
    match kind {
        ActionKind::Timeout => t.s("Quack didn't time out ").person(m).s(because),
        ActionKind::Kick | ActionKind::Ban => t
            .s("Quack didn't ")
            .s(if kind == ActionKind::Kick {
                "remove "
            } else {
                "ban "
            })
            .person(m)
            .s(because),
        ActionKind::RemoveTimeout => t.s("Quack didn't remove ").person(m).s("'s timeout."),
        ActionKind::Unban => t.s("Quack didn't unban ").person(m).s("."),
    }
}

fn skipped(a: &ActionView, m: &Person) -> Text {
    let t = Text::new();
    let what = match a.kind {
        ActionKind::RemoveTimeout => "'s timeout in place because ",
        ActionKind::Unban => "'s ban in place because ",
        // Originals are never skipped; this only keeps the match total.
        ActionKind::Timeout | ActionKind::Kick | ActionKind::Ban => return cancelled(a.kind, m),
    };
    let t = t.s("Quack left ").person(m).s(what);
    match (a.skip, a.kind) {
        (Some(ActionSkipReason::ChangedOutsideCase), ActionKind::Unban) => {
            t.s("it didn't come from a voided Quack case. Check it in Discord.")
        }
        (Some(ActionSkipReason::ChangedOutsideCase), _) => t
            .s("someone changed it outside case #")
            .s(a.case_number.to_string())
            .s(". Check it in Discord."),
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
        ActionState::Pending => {
            let retry = a
                .next_attempt_at
                .filter(|_| full && a.attempts > 0)
                .map_or_else(Text::new, retry_scheduled);
            in_progress(a, member).sentence(retry)
        }
        ActionState::Running => in_progress(a, member),
        ActionState::Done => done(a, member),
        ActionState::Failed => {
            let base = if a.unconfirmed() {
                unconfirmed(a.kind, member, detail)
            } else {
                let reason = a
                    .failure
                    .filter(|_| full)
                    .and_then(|f| failure_reason(a.kind, &f, Some(member), a.attempts))
                    .unwrap_or_default();
                couldnt(a.kind, member).sentence(reason)
            };
            match (&a.dismissed, full) {
                (Some((by, at)), true) => base.sentence(dismissed_suffix(by, at.to_owned())),
                _ => base,
            }
        }
        ActionState::Cancelled => cancelled(a.kind, member),
        ActionState::Skipped => skipped(a, member),
    }
}
