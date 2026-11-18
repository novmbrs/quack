//! Running an attempt (§5.4), attempt outcomes (§5.5), and the retry
//! policy (§5.7).

use time::{Duration, OffsetDateTime};

use super::controls::{StillApplies, undo_needed};
use super::{
    ActionEvent, ActionFailure, ActionKind, ActionPurpose, ActionState, AttemptOutcome, Failure,
    transition,
};
use crate::InvalidTransition;

/// Attempts per job: one first try plus seven retries.
pub const MAX_ATTEMPTS: u32 = 8;

/// The wait after attempt n (n = 1…7) before attempt n + 1.
pub const RETRY_BACKOFF: [Duration; 7] = [
    Duration::seconds(5),
    Duration::seconds(15),
    Duration::minutes(1),
    Duration::minutes(5),
    Duration::minutes(15),
    Duration::hours(1),
    Duration::hours(3),
];

/// How close to a timeout's fixed end Quack stops trying: within this
/// margin the attempt fails with `timeout_window_passed`.
pub const TIMEOUT_END_MARGIN: Duration = Duration::seconds(5);

const UNKNOWN_GUILD: u32 = 10_004;
const UNKNOWN_MEMBER: u32 = 10_007;
const UNKNOWN_BAN: u32 = 10_026;

/// A timeout's fixed end: the case's creation time plus the level's
/// duration. Computed once when the case is created and sent unchanged on
/// every attempt, so repeating the call converges on the same state.
pub fn timeout_ends_at(case_created_at: OffsetDateTime, duration_seconds: u32) -> OffsetDateTime {
    case_created_at + Duration::seconds(i64::from(duration_seconds))
}

/// Whether it is too late to apply a timeout ending at `ends_at`.
pub fn timeout_window_passed(now: OffsetDateTime, ends_at: OffsetDateTime) -> bool {
    now >= ends_at - TIMEOUT_END_MARGIN
}

/// When to try again after attempt `attempt` (1-based within the job's
/// budget) ended uncertainly; `None` once the budget is spent. A
/// `retry_after` from Discord stretches the wait, never shortens it.
pub fn retry_delay(attempt: u32, retry_after: Option<Duration>) -> Option<Duration> {
    if attempt >= MAX_ATTEMPTS {
        return None;
    }
    let index = usize::try_from(attempt.max(1) - 1).ok()?;
    let backoff = *RETRY_BACKOFF.get(index)?;
    Some(retry_after.map_or(backoff, |d| d.max(backoff)))
}

/// A definite refusal from Discord: its HTTP status and JSON error code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiscordRefusal {
    /// HTTP status (a 4xx other than 429).
    pub status: u16,
    /// Discord's JSON error code, when the body had one.
    pub code: Option<u32>,
}

/// The result of one Discord call for an action, as the port reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttemptResult {
    /// Discord confirmed it.
    Done,
    /// Discord definitely refused.
    Refused(DiscordRefusal),
    /// Rate limited before anything happened; wait at least this long.
    RateLimited(Duration),
    /// The call may or may not have taken effect.
    Unknown,
    /// A previous worker crashed mid-attempt (found on the next claim).
    Interrupted,
}

impl AttemptResult {
    /// Classifies the result for an action of `kind` (§5.5): a refusal that
    /// means the desired state already holds is `no_change`.
    pub fn outcome(self, kind: ActionKind) -> (AttemptOutcome, Option<Failure>) {
        match self {
            Self::Done => (AttemptOutcome::Done, None),
            Self::Refused(r) => match (kind, r.code) {
                (ActionKind::Kick | ActionKind::RemoveTimeout, Some(UNKNOWN_MEMBER))
                | (ActionKind::Unban, Some(UNKNOWN_BAN)) => (AttemptOutcome::NoChange, None),
                (_, Some(UNKNOWN_GUILD)) => (
                    AttemptOutcome::Refused,
                    Some(Failure::new(ActionFailure::QuackNotInServer)),
                ),
                (ActionKind::Timeout, Some(UNKNOWN_MEMBER)) => (
                    AttemptOutcome::Refused,
                    Some(Failure::new(ActionFailure::MemberNotInServer)),
                ),
                _ => (
                    AttemptOutcome::Refused,
                    Some(Failure {
                        http_status: Some(r.status),
                        discord_code: r.code,
                        ..Failure::new(ActionFailure::DiscordRefused)
                    }),
                ),
            },
            Self::RateLimited(_) => (AttemptOutcome::RateLimited, None),
            Self::Unknown => (AttemptOutcome::Unknown, None),
            Self::Interrupted => (AttemptOutcome::Interrupted, None),
        }
    }
}

/// What the action job knows when it settles an attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttemptContext {
    /// The action's kind.
    pub kind: ActionKind,
    /// Whether its case is still valid (read inside the settle transaction).
    pub case_valid: bool,
    /// This attempt's number within the job's budget (1-based).
    pub attempt: u32,
    /// The original timeout's fixed end, for deciding whether a voided
    /// case's timeout still needs undoing.
    pub timeout_ends_at: Option<OffsetDateTime>,
    /// The current time.
    pub now: OffsetDateTime,
}

/// The settle transaction's decision for one finished attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settlement {
    /// The action's new state.
    pub state: ActionState,
    /// The attempt row's outcome.
    pub outcome: AttemptOutcome,
    /// Set when the new state is `failed`.
    pub failure: Option<Failure>,
    /// Done because the desired state already held.
    pub no_change: bool,
    /// Set when the new state is `pending`: retry after this long.
    pub retry_in: Option<Duration>,
    /// The original is done but its case was voided meanwhile: queue the
    /// undo in the same transaction (§4.7 step 5).
    pub queue_undo: Option<ActionKind>,
}

/// Settles a running action's attempt (§5.4 step 10).
pub fn settle(
    ctx: &AttemptContext,
    result: AttemptResult,
) -> Result<Settlement, InvalidTransition> {
    let (outcome, refusal) = result.outcome(ctx.kind);
    let retry_after = match result {
        AttemptResult::RateLimited(d) => Some(d),
        _ => None,
    };
    let retry_in = retry_delay(ctx.attempt, retry_after);
    let event = ActionEvent::AttemptFinished {
        outcome,
        case_valid: ctx.case_valid,
        attempts_left: retry_in.is_some(),
    };
    let state = transition(ctx.kind, ActionState::Running, event)?;
    let failure = match state {
        ActionState::Failed => {
            Some(refusal.unwrap_or(Failure::new(ActionFailure::RetriesExhausted)))
        }
        _ => None,
    };
    let voided_original = !ctx.case_valid && ctx.kind.purpose() == ActionPurpose::Original;
    Ok(Settlement {
        state,
        outcome,
        failure,
        no_change: state == ActionState::Done && outcome == AttemptOutcome::NoChange,
        retry_in: if state == ActionState::Pending {
            retry_in
        } else {
            None
        },
        queue_undo: if state == ActionState::Done
            && voided_original
            && undo_needed(ctx.kind, ctx.timeout_ends_at, ctx.now)
        {
            ctx.kind.undo_kind()
        } else {
            None
        },
    })
}

/// What the action job knows after claiming, before calling Discord
/// (§5.4 steps 2–7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClaimFacts {
    /// The action's kind.
    pub kind: ActionKind,
    /// Its stored state.
    pub state: ActionState,
    /// Whether its case is valid.
    pub case_valid: bool,
    /// For an undo: the undo guard's answer (§5.6). Ignored for originals.
    pub still_applies: Option<StillApplies>,
    /// The Quack-side pre-check's answer (§5.4 step 6).
    pub precheck: Result<(), Failure>,
    /// For a timeout: its fixed end.
    pub timeout_ends_at: Option<OffsetDateTime>,
    /// The current time.
    pub now: OffsetDateTime,
}

/// The job's next step after claiming an action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaimDecision {
    /// Already settled (re-delivery): finish the job without changes.
    AlreadySettled,
    /// A previous attempt crashed: settle it with
    /// [`AttemptResult::Interrupted`] first.
    Interrupted,
    /// Original on a voided case: `cancelled`.
    Cancel,
    /// Undo that another valid case blocks: `skipped`.
    Skip(StillApplies),
    /// Refused before calling Discord: `failed`.
    Fail(Failure),
    /// Record the attempt (`running`) and call Discord. A timeout is sent
    /// with its stored end, never a recomputed one.
    Attempt {
        /// The `communication_disabled_until` value for a timeout.
        timeout_until: Option<OffsetDateTime>,
    },
}

/// Decides what a claimed action job does next, in the design's order:
/// settled state, crashed attempt, voided original, undo guard, pre-check,
/// timeout window.
pub fn before_attempt(facts: &ClaimFacts) -> ClaimDecision {
    match facts.state {
        ActionState::Pending => {}
        ActionState::Running => return ClaimDecision::Interrupted,
        _ => return ClaimDecision::AlreadySettled,
    }
    match facts.kind.purpose() {
        ActionPurpose::Original if !facts.case_valid => return ClaimDecision::Cancel,
        ActionPurpose::Undo => {
            if let Some(still) = facts.still_applies {
                return ClaimDecision::Skip(still);
            }
        }
        ActionPurpose::Original => {}
    }
    if let Err(failure) = facts.precheck {
        return ClaimDecision::Fail(failure);
    }
    if facts.kind == ActionKind::Timeout {
        match facts.timeout_ends_at {
            Some(end) if !timeout_window_passed(facts.now, end) => {}
            _ => return ClaimDecision::Fail(Failure::new(ActionFailure::TimeoutWindowPassed)),
        }
    }
    ClaimDecision::Attempt {
        timeout_until: facts.timeout_ends_at,
    }
}
