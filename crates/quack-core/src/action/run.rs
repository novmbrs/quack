//! Running an attempt (§5.4): the start decision, the read-first plan of
//! §5.1 and §5.6, classifying results (§5.5), the settle, and the retry
//! policy (§5.7).

use time::{Duration, OffsetDateTime};

use super::controls::StillApplies;
use super::{
    ActionEvent, ActionFailure, ActionKind, ActionSkipReason, ActionState, AttemptOutcome, Failure,
    transition,
};
use crate::InvalidTransition;
use crate::dm::ACTION_WAIT;
use crate::permissions::{BanFacts, MemberFacts};

/// Attempts per round: one first try plus seven retries. A staff retry
/// starts a new round.
pub const MAX_ATTEMPTS: u32 = 8;

/// The wait after attempt n (n = 1…7) of a round before attempt n + 1.
/// They add up to 4 h 21 min 20 s.
pub const RETRY_BACKOFF: [Duration; 7] = [
    Duration::seconds(5),
    Duration::seconds(15),
    Duration::minutes(1),
    Duration::minutes(5),
    Duration::minutes(15),
    Duration::hours(1),
    Duration::hours(3),
];

/// How long a start waits while another action of the same member is
/// running (§5.4 step 1.6). A defer does not use an attempt.
pub const MEMBER_BUSY_DEFER: Duration = Duration::seconds(2);

const UNKNOWN_GUILD: u32 = 10_004;
const UNKNOWN_MEMBER: u32 = 10_007;
const UNKNOWN_BAN: u32 = 10_026;

/// A timeout's fixed end: the case's creation time plus the level's
/// duration, truncated to whole seconds (§5.2). Computed once when the case
/// is created and sent unchanged on every attempt. `None` when the end
/// can't be represented (a creation time at the end of the calendar).
pub fn timeout_ends_at(
    case_created_at: OffsetDateTime,
    duration_seconds: u32,
) -> Option<OffsetDateTime> {
    let end = case_created_at.checked_add(Duration::seconds(i64::from(duration_seconds)))?;
    Some(end.replace_nanosecond(0).unwrap_or(end))
}

/// `at + by`, or the last representable instant when that overflows:
/// a wait that long never ends, and no stored time can make it panic.
pub fn saturating_add(at: OffsetDateTime, by: Duration) -> OffsetDateTime {
    at.checked_add(by).unwrap_or_else(|| {
        if by.is_negative() {
            time::PrimitiveDateTime::MIN.assume_offset(at.offset())
        } else {
            time::PrimitiveDateTime::MAX.assume_offset(at.offset())
        }
    })
}

/// Whether it is too late to apply a timeout ending at `ends_at`.
pub fn timeout_window_passed(now: OffsetDateTime, ends_at: OffsetDateTime) -> bool {
    now >= ends_at
}

/// The wait before the next attempt after attempt `attempt` (1-based
/// within the round) ended uncertainly; `None` once the round's budget is
/// spent. A `retry_after` from Discord stretches the wait, never shortens
/// it.
pub fn retry_delay(attempt: u32, retry_after: Option<Duration>) -> Option<Duration> {
    if attempt >= MAX_ATTEMPTS {
        return None;
    }
    let index = usize::try_from(attempt.max(1) - 1).ok()?;
    let backoff = *RETRY_BACKOFF.get(index)?;
    Some(retry_after.map_or(backoff, |d| d.max(backoff)))
}

/// What the start transaction knows after locking the case and the
/// action (§5.4 step 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartFacts {
    /// The action's kind.
    pub kind: ActionKind,
    /// Its stored state.
    pub state: ActionState,
    /// When a `pending` action's next attempt is due.
    pub next_attempt_at: Option<OffsetDateTime>,
    /// Whether another action of this member is `running`.
    pub member_busy: bool,
    /// Whether the case is valid.
    pub case_valid: bool,
    /// When the case was created.
    pub case_created_at: OffsetDateTime,
    /// Whether the case DM is still `pending` or `sending`.
    pub dm_waiting: bool,
    /// For an undo: the undo guard's answer (§5.6). Ignored for originals.
    pub still_applies: Option<StillApplies>,
    /// The Quack-side pre-check's answer
    /// ([`crate::permissions::action_precheck`]).
    pub precheck: Result<(), Failure>,
    /// For a timeout: its fixed end.
    pub timeout_ends_at: Option<OffsetDateTime>,
    /// The current time.
    pub now: OffsetDateTime,
}

/// What the start transaction does (§5.4 step 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartDecision {
    /// Terminal or `failed`: finish the job without changes.
    Finish,
    /// An earlier run stopped mid-attempt: settle the open attempt as
    /// [`AttemptResult::Interrupted`], then decide again.
    SettleInterrupted,
    /// Not due yet: run the job again at this time.
    RetryAt(OffsetDateTime),
    /// Wait without using an attempt: another action of the member is
    /// running, or a kick or ban waits for its DM (§6.3).
    DeferUntil(OffsetDateTime),
    /// An original on a voided case: `cancelled`.
    Cancel,
    /// An undo another valid case blocks: `skipped` (`other_case_applies`).
    Skip(StillApplies),
    /// Refused before calling Discord: `failed`.
    Fail(Failure),
    /// Open the attempt (`running`) and commit; then read and act.
    Open,
}

/// Decides the start in the design's order: settled, open attempt, not
/// due, member busy, waiting for the DM, voided original, undo guard,
/// timeout end, pre-check.
pub fn start(facts: &StartFacts) -> StartDecision {
    match facts.state {
        ActionState::Pending => {}
        ActionState::Running => return StartDecision::SettleInterrupted,
        _ => return StartDecision::Finish,
    }
    if let Some(due) = facts.next_attempt_at
        && due > facts.now
    {
        return StartDecision::RetryAt(due);
    }
    if facts.member_busy {
        return StartDecision::DeferUntil(saturating_add(facts.now, MEMBER_BUSY_DEFER));
    }
    if matches!(facts.kind, ActionKind::Kick | ActionKind::Ban)
        && facts.dm_waiting
        && facts.now - facts.case_created_at < ACTION_WAIT
    {
        return StartDecision::DeferUntil(saturating_add(facts.case_created_at, ACTION_WAIT));
    }
    if facts.kind.is_original() {
        if !facts.case_valid {
            return StartDecision::Cancel;
        }
    } else if let Some(still) = facts.still_applies {
        return StartDecision::Skip(still);
    }
    if facts.kind == ActionKind::Timeout
        && facts
            .timeout_ends_at
            .is_none_or(|end| timeout_window_passed(facts.now, end))
    {
        return StartDecision::Fail(Failure::new(ActionFailure::TimeoutWindowPassed));
    }
    match facts.precheck {
        Err(failure) => StartDecision::Fail(failure),
        Ok(()) => StartDecision::Open,
    }
}

/// A definite refusal from Discord: its HTTP status and JSON error code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiscordRefusal {
    /// HTTP status (a 4xx other than 429).
    pub status: u16,
    /// Discord's JSON error code, when the body had one.
    pub code: Option<u32>,
}

/// The result of one Discord call, as the `Discord` port reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallOutcome {
    /// Discord confirmed it.
    Done,
    /// Discord definitely refused; nothing changed.
    Refused(DiscordRefusal),
    /// Rate limited before anything was sent.
    RetryAfter(Duration),
    /// Never sent (connection refused, shutting down); nothing changed.
    NotSent,
    /// May or may not have taken effect.
    Unknown,
}

/// What the action is about, from its case and action rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActionTarget {
    /// The action's kind.
    pub kind: ActionKind,
    /// The case's number: its marker in a ban reason.
    pub case_number: u64,
    /// For a timeout and its removal: the case's fixed end.
    pub timeout_ends_at: Option<OffsetDateTime>,
    /// For a kick: the member's `joined_at` when the case was created;
    /// `None` for an imported case.
    pub member_joined_at: Option<OffsetDateTime>,
    /// When the case was created.
    pub case_created_at: OffsetDateTime,
    /// For an undo: its original never reached `done` and an attempt of it
    /// may have applied, so Discord may still apply it after this read
    /// (the original's change is unresolved).
    pub original_unresolved: bool,
}

/// How one attempt ended, before it is settled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttemptResult {
    /// The change call returned `Done`, or the ban read found this case's
    /// own earlier ban.
    Done,
    /// The desired state already held.
    NoChange,
    /// An undo found a timeout or ban that is not this case's (§5.6).
    ChangedOutsideCase,
    /// Discord refused a read or the change call, or Quack's read showed it
    /// can't be done.
    Refused {
        /// The failure to record.
        failure: Failure,
        /// Discord's answer, for the attempt row, when there was one.
        discord: Option<DiscordRefusal>,
    },
    /// Nothing reached Discord: a read failed, or a rate limit came before
    /// the change call.
    NotSent {
        /// Discord's requested wait, when it rate limited.
        retry_after: Option<Duration>,
    },
    /// The change call's result is unknown.
    Unknown,
    /// The next run found the attempt still open.
    Interrupted,
    /// An undo found nothing to undo, but its original's change is
    /// unresolved and may still land: the undo changed nothing and reads
    /// again later instead of finishing `no_change`.
    OriginalUnresolved,
}

impl AttemptResult {
    /// The attempt row's outcome.
    pub const fn outcome(self) -> AttemptOutcome {
        match self {
            Self::Done => AttemptOutcome::Done,
            Self::NoChange | Self::ChangedOutsideCase => AttemptOutcome::NoChange,
            Self::Refused { .. } => AttemptOutcome::Refused,
            Self::NotSent { .. } | Self::OriginalUnresolved => AttemptOutcome::NotSent,
            Self::Unknown => AttemptOutcome::Unknown,
            Self::Interrupted => AttemptOutcome::Interrupted,
        }
    }

    /// Discord's refusal, for the attempt row's `http_status` and
    /// `discord_code`.
    pub const fn discord(self) -> Option<DiscordRefusal> {
        match self {
            Self::Refused { discord, .. } => discord,
            _ => None,
        }
    }

    fn refused(reason: ActionFailure, discord: Option<DiscordRefusal>) -> Self {
        Self::Refused {
            failure: Failure::new(reason),
            discord,
        }
    }
}

/// The next step of an attempt after its read (§5.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    /// Make the change call.
    Call,
    /// The ban carries the marker of case #N: check whether that case is a
    /// voided case of this member in this server with a ban action, then
    /// call [`plan_after_marker`] (unban only).
    CheckMarker(u64),
    /// No change call: settle with this result.
    Finish(AttemptResult),
}

fn wrong_read(kind: ActionKind, event: &'static str) -> InvalidTransition {
    InvalidTransition {
        machine: "action",
        from: kind.as_str(),
        event,
    }
}

/// Plans a timeout, kick, or remove timeout from the member read
/// (`None` is 404 Unknown Member).
///
/// - Timeout: absent → `member_not_in_server`; Administrator →
///   `member_is_administrator`; a timeout already ending at or after this
///   case's end → `no_change` (Quack never shortens a timeout).
/// - Kick: absent, or joined after the case's membership (they left and
///   rejoined) → `no_change`.
/// - Remove timeout: Discord's end is first cut to whole seconds (§5.2),
///   then: no timeout, or one that has ended → `no_change` (or
///   [`AttemptResult::OriginalUnresolved`] while the original's change may
///   still land and its end is in the future); one whose end is a second
///   or more from this case's → `changed_outside_case`; within a second
///   (Discord's precision differs from the whole seconds Quack sent) →
///   remove it.
pub fn plan_from_member(
    target: &ActionTarget,
    member: Option<&MemberFacts>,
    now: OffsetDateTime,
) -> Result<Plan, InvalidTransition> {
    let plan = match target.kind {
        ActionKind::Timeout => match member {
            None => Plan::Finish(AttemptResult::refused(
                ActionFailure::MemberNotInServer,
                None,
            )),
            Some(m) if m.is_administrator() => Plan::Finish(AttemptResult::refused(
                ActionFailure::MemberIsAdministrator,
                None,
            )),
            Some(m) => match (m.timeout_until, target.timeout_ends_at) {
                (Some(current), Some(end)) if whole_seconds(current) >= whole_seconds(end) => {
                    Plan::Finish(AttemptResult::NoChange)
                }
                _ => Plan::Call,
            },
        },
        ActionKind::Kick => {
            let membership_start = target.member_joined_at.unwrap_or(target.case_created_at);
            match member {
                Some(m) if m.joined_at.is_none_or(|joined| joined <= membership_start) => {
                    Plan::Call
                }
                _ => Plan::Finish(AttemptResult::NoChange),
            }
        }
        ActionKind::RemoveTimeout => match member.and_then(|m| m.timeout_until) {
            Some(current) if seconds_only(current) > now => match target.timeout_ends_at {
                Some(end) if (current - end).abs() < Duration::SECOND => Plan::Call,
                _ => Plan::Finish(AttemptResult::ChangedOutsideCase),
            },
            _ if target.original_unresolved
                && target.timeout_ends_at.is_some_and(|end| end > now) =>
            {
                Plan::Finish(AttemptResult::OriginalUnresolved)
            }
            _ => Plan::Finish(AttemptResult::NoChange),
        },
        ActionKind::Ban | ActionKind::Unban => return Err(wrong_read(target.kind, "member_read")),
    };
    Ok(plan)
}

/// Plans a ban or unban from the ban read (`None` is 404 Unknown Ban).
///
/// - Ban: a ban exists → `done` when its reason carries this case's marker
///   (an earlier attempt placed it), else `no_change`; so messages are
///   never deleted twice. No ban → place it.
/// - Unban: no ban → `no_change` (or [`AttemptResult::OriginalUnresolved`]
///   while the original's ban may still land); a ban carrying this undo's own case
///   marker → remove it (its case placed it; the guard already ran, so no
///   other valid case wants it, whether this case was voided or staff
///   asked for the undo by hand); another case's marker → check that case
///   ([`Plan::CheckMarker`]); no marker → `changed_outside_case`.
pub fn plan_from_ban(
    target: &ActionTarget,
    ban: Option<&BanFacts>,
) -> Result<Plan, InvalidTransition> {
    let marker = ban.and_then(BanFacts::case_marker);
    let plan = match (target.kind, ban) {
        (ActionKind::Ban, None) => Plan::Call,
        (ActionKind::Ban, Some(_)) if marker == Some(target.case_number) => {
            Plan::Finish(AttemptResult::Done)
        }
        (ActionKind::Unban, None) if target.original_unresolved => {
            Plan::Finish(AttemptResult::OriginalUnresolved)
        }
        (ActionKind::Ban, Some(_)) | (ActionKind::Unban, None) => {
            Plan::Finish(AttemptResult::NoChange)
        }
        (ActionKind::Unban, Some(_)) => match marker {
            Some(n) if n == target.case_number => Plan::Call,
            Some(n) => Plan::CheckMarker(n),
            None => Plan::Finish(AttemptResult::ChangedOutsideCase),
        },
        _ => return Err(wrong_read(target.kind, "ban_read")),
    };
    Ok(plan)
}

/// Finishes an unban's plan once the marker's case is known: remove the
/// ban when that case is a voided case of this member in this server with
/// a ban action, otherwise leave it (`changed_outside_case`).
pub const fn plan_after_marker(voided_ban_case_of_member: bool) -> Plan {
    if voided_ban_case_of_member {
        Plan::Call
    } else {
        Plan::Finish(AttemptResult::ChangedOutsideCase)
    }
}

/// Classifies a read that did not answer (§5.5): a refusal fails the
/// action (Unknown Guild → `quack_not_in_server`, else `discord_refused`);
/// anything else changed nothing (`not_sent`).
pub fn after_read_failure(outcome: CallOutcome) -> AttemptResult {
    match outcome {
        CallOutcome::Refused(r) if r.code == Some(UNKNOWN_GUILD) => {
            AttemptResult::refused(ActionFailure::QuackNotInServer, Some(r))
        }
        CallOutcome::Refused(r) => AttemptResult::Refused {
            failure: Failure::discord_refused(r.status, r.code),
            discord: Some(r),
        },
        CallOutcome::RetryAfter(d) => AttemptResult::NotSent {
            retry_after: Some(d),
        },
        CallOutcome::Done | CallOutcome::NotSent | CallOutcome::Unknown => {
            AttemptResult::NotSent { retry_after: None }
        }
    }
}

/// Classifies the change call's result (§5.5). Unknown Member on a kick and
/// Unknown Ban on an unban mean the desired state already holds. A timeout
/// Discord refuses after its end has passed is `timeout_window_passed`.
pub fn after_call(
    target: &ActionTarget,
    outcome: CallOutcome,
    now: OffsetDateTime,
) -> AttemptResult {
    match outcome {
        CallOutcome::Done => AttemptResult::Done,
        CallOutcome::Refused(r) => match (target.kind, r.code) {
            (ActionKind::Kick, Some(UNKNOWN_MEMBER)) | (ActionKind::Unban, Some(UNKNOWN_BAN)) => {
                AttemptResult::NoChange
            }
            (_, Some(UNKNOWN_GUILD)) => {
                AttemptResult::refused(ActionFailure::QuackNotInServer, Some(r))
            }
            (ActionKind::Timeout, _)
                if target
                    .timeout_ends_at
                    .is_some_and(|end| timeout_window_passed(now, end)) =>
            {
                AttemptResult::refused(ActionFailure::TimeoutWindowPassed, Some(r))
            }
            (ActionKind::Timeout, Some(UNKNOWN_MEMBER)) => {
                AttemptResult::refused(ActionFailure::MemberNotInServer, Some(r))
            }
            _ => AttemptResult::Refused {
                failure: Failure::discord_refused(r.status, r.code),
                discord: Some(r),
            },
        },
        CallOutcome::RetryAfter(d) => AttemptResult::NotSent {
            retry_after: Some(d),
        },
        CallOutcome::NotSent => AttemptResult::NotSent { retry_after: None },
        CallOutcome::Unknown => AttemptResult::Unknown,
    }
}

/// What the settle transaction knows (§5.4 step 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SettleFacts<'a> {
    /// The action's kind.
    pub kind: ActionKind,
    /// Whether its case is still valid (read under the case lock).
    pub case_valid: bool,
    /// This attempt's number within the round (the action's `attempts`).
    pub attempt: u32,
    /// The outcomes of this round's earlier attempts.
    pub earlier: &'a [AttemptOutcome],
    /// The current time.
    pub now: OffsetDateTime,
}

/// The settle transaction's writes for one finished attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settlement {
    /// The action's new state.
    pub state: ActionState,
    /// The attempt row's outcome.
    pub outcome: AttemptOutcome,
    /// Set when the new state is `failed`.
    pub failure: Option<Failure>,
    /// Set when the new state is `skipped`.
    pub skip_reason: Option<ActionSkipReason>,
    /// Done because the desired state already held.
    pub no_change: bool,
    /// Set when the new state is `pending`: when the next attempt is due.
    pub next_attempt_at: Option<OffsetDateTime>,
}

/// Settles a running action's attempt through the settle rows of §5.3.
pub fn settle(
    facts: &SettleFacts<'_>,
    result: AttemptResult,
) -> Result<Settlement, InvalidTransition> {
    let outcome = result.outcome();
    let retry_after = match result {
        AttemptResult::NotSent { retry_after } => retry_after,
        _ => None,
    };
    let retry_in = retry_delay(facts.attempt, retry_after);
    let event = ActionEvent::Settled {
        outcome,
        changed_outside_case: result == AttemptResult::ChangedOutsideCase,
        case_valid: facts.case_valid,
        attempts_left: retry_in.is_some(),
    };
    let state = transition(facts.kind, ActionState::Running, event)?;
    let failure = match (state, result) {
        (ActionState::Failed, AttemptResult::Refused { failure, .. }) => Some(failure),
        (ActionState::Failed, AttemptResult::OriginalUnresolved) => {
            Some(Failure::new(ActionFailure::OriginalUnconfirmed))
        }
        (ActionState::Failed, _) => {
            let round_applied = outcome.may_have_applied()
                || facts
                    .earlier
                    .iter()
                    .copied()
                    .any(AttemptOutcome::may_have_applied);
            Some(Failure::new(if round_applied {
                ActionFailure::Unconfirmed
            } else {
                ActionFailure::RetriesExhausted
            }))
        }
        _ => None,
    };
    Ok(Settlement {
        state,
        outcome,
        failure,
        skip_reason: (state == ActionState::Skipped)
            .then_some(ActionSkipReason::ChangedOutsideCase),
        no_change: state == ActionState::Done && outcome == AttemptOutcome::NoChange,
        next_attempt_at: retry_in
            .filter(|_| state == ActionState::Pending)
            .map(|wait| saturating_add(facts.now, wait)),
    })
}

fn whole_seconds(at: OffsetDateTime) -> i64 {
    at.unix_timestamp()
}

fn seconds_only(at: OffsetDateTime) -> OffsetDateTime {
    at.replace_nanosecond(0).unwrap_or(at)
}
