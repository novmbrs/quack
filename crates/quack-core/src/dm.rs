//! DMs (§6): the one direct message per case, appeal decision, or closed
//! ticket, sent at most once.

use time::{Duration, OffsetDateTime};

use crate::InvalidTransition;
use crate::limits::NONCE_WINDOW;

str_enum! {
    /// What a DM is about (`dm_kind`).
    pub enum DmKind {
        /// A case whose level sends a DM.
        Case = "case",
        /// A staff decision on an appeal, or Quack closing it.
        AppealUpdate = "appeal_update",
        /// A ticket finished closing.
        TicketClosed = "ticket_closed",
    }
}

str_enum! {
    /// A DM's stored state (`dm_state`).
    pub enum DmState {
        /// Not yet sent. Outcome: in progress.
        Pending = "pending",
        /// The DM channel is open and `first_attempt_at` is recorded before
        /// the first create. Outcome: in progress.
        Sending = "sending",
        /// Discord returned the message. Outcome: done.
        Sent = "sent",
        /// Discord definitely refused before any create with an unknown
        /// result, or the DM channel could not be opened. Outcome: failed.
        Failed = "failed",
        /// A create's result was never learned and the nonce window ended.
        /// Terminal: never sent again. Outcome: couldn't confirm.
        Unknown = "unknown",
        /// The case was voided before the DM was sent.
        NotNeeded = "not_needed",
    }
}

impl DmState {
    /// Whether the DM has settled for good.
    pub const fn is_terminal(self) -> bool {
        !matches!(self, Self::Pending | Self::Sending)
    }
}

str_enum! {
    /// Why a DM failed (`dm_failure`, §6.5).
    pub enum DmFailure {
        /// Discord error 50007: their DMs may be closed, or they blocked Quack.
        DmsClosed = "dms_closed",
        /// Discord error 50278: they no longer share a server with Quack.
        NotInSharedServer = "not_in_shared_server",
        /// The DM channel could not be opened, or Discord kept rate
        /// limiting, within the budget, and nothing was sent.
        DiscordUnavailable = "discord_unavailable",
        /// Any other 4xx except 429.
        DiscordRefused = "discord_refused",
    }
}

const CANNOT_DM: u32 = 50_007;
const NO_MUTUAL_GUILDS: u32 = 50_278;

impl DmFailure {
    /// Classifies a definite refusal (§6.5) by Discord error code.
    pub const fn from_refusal(code: Option<u32>) -> Self {
        match code {
            Some(CANNOT_DM) => Self::DmsClosed,
            Some(NO_MUTUAL_GUILDS) => Self::NotInSharedServer,
            _ => Self::DiscordRefused,
        }
    }
}

/// After a create with an unknown result, wait this long and repeat it with
/// the same nonce while the window lasts.
pub const UNKNOWN_REPEAT_DELAY: Duration = Duration::seconds(2);
/// The longest a kick or ban waits for its case DM (§6.3): the action's
/// job runs at `created_at + 120 s`, moved earlier when the DM settles.
pub const ACTION_WAIT: Duration = Duration::seconds(120);

/// Whether a create may still be repeated with the same nonce: less than
/// [`NONCE_WINDOW`] has passed since `first_attempt_at` (§10.4). Compares
/// elapsed time, so no stored timestamp can overflow it.
pub fn in_window(first_attempt_at: OffsetDateTime, now: OffsetDateTime) -> bool {
    now - first_attempt_at < NONCE_WINDOW
}

/// Whether waiting `wait` from `now` still ends inside the window. A wait
/// too long to add is never inside it.
pub fn wait_fits(first_attempt_at: OffsetDateTime, now: OffsetDateTime, wait: Duration) -> bool {
    (now - first_attempt_at)
        .checked_add(wait)
        .is_some_and(|elapsed| elapsed < NONCE_WINDOW)
}

/// How one create call of a DM ended, as far as the DM's history cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreateResult {
    /// Discord returned the message.
    Sent,
    /// Discord definitely refused it.
    Refused,
    /// Rate limited before anything was sent.
    RetryAfter,
    /// Never sent (a connection refused, shutting down).
    NotSent,
    /// May or may not have reached Discord.
    Unknown,
}

/// What a `send_dm` run knows about this DM's earlier creates (§6.4).
///
/// The rule it carries: once any create may have reached Discord, the DM
/// can only end `sent` or `unknown`, never `failed` or back to `pending`,
/// so it is never sent twice. A run that found the DM already `sending`
/// resumes one that may have crashed after a create Discord accepted, so
/// it starts as "maybe sent"; only a run that committed `pending` →
/// `sending` itself starts clean. A `not_sent` or rate-limited create
/// reached nothing and changes nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SendHistory {
    maybe_sent: bool,
}

impl SendHistory {
    /// The history at the start of a run, from the state the run loaded.
    pub const fn at_start(loaded: DmState) -> Self {
        Self {
            maybe_sent: !matches!(loaded, DmState::Pending),
        }
    }

    /// The history after one more create.
    #[must_use]
    pub const fn after(self, create: CreateResult) -> Self {
        Self {
            maybe_sent: self.maybe_sent || matches!(create, CreateResult::Unknown),
        }
    }

    /// Whether some create of this DM may have reached Discord.
    pub const fn maybe_sent(self) -> bool {
        self.maybe_sent
    }

    /// The event for a create Discord refused.
    pub const fn refused(self) -> DmEvent {
        DmEvent::CreateRefused {
            after_unknown: self.maybe_sent,
        }
    }

    /// The event for a rate limit whose wait does not fit in the window.
    pub const fn retry_after_beyond_window(self) -> DmEvent {
        DmEvent::RetryAfterBeyondWindow {
            after_unknown: self.maybe_sent,
        }
    }
}

/// An event in the DM transition table (§6.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DmEvent {
    /// The case was voided before sending started (case DMs only).
    CaseVoided,
    /// Start: the DM channel is open and, for a case DM, the case is valid.
    Started,
    /// Discord refused to open the DM channel.
    OpenRefused,
    /// Discord returned the message.
    Created,
    /// Discord refused a create.
    CreateRefused {
        /// Whether an earlier create may have reached Discord
        /// ([`SendHistory::maybe_sent`]; a resumed `sending` DM counts).
        after_unknown: bool,
    },
    /// A rate limit's wait does not fit in the window.
    RetryAfterBeyondWindow {
        /// Whether an earlier create may have reached Discord
        /// ([`SendHistory::maybe_sent`]; a resumed `sending` DM counts).
        after_unknown: bool,
    },
    /// The window ended after a create with an unknown result, or a run
    /// found the DM `sending` after the window.
    WindowEnded,
    /// The job's budget is spent (§16.1 finalization).
    BudgetSpent,
}

impl DmEvent {
    /// The event's name, for errors and logs.
    pub const fn name(self) -> &'static str {
        match self {
            Self::CaseVoided => "case_voided",
            Self::Started => "started",
            Self::OpenRefused => "open_refused",
            Self::Created => "created",
            Self::CreateRefused { .. } => "create_refused",
            Self::RetryAfterBeyondWindow { .. } => "retry_after_beyond_window",
            Self::WindowEnded => "window_ended",
            Self::BudgetSpent => "budget_spent",
        }
    }
}

/// Applies one event to a DM of `kind` in `state` (§6.2). Any pair the
/// table does not list is an [`InvalidTransition`]. A create that may have
/// reached Discord is never followed by `failed` or `pending`, so a DM is
/// sent at most once.
pub fn transition(
    kind: DmKind,
    state: DmState,
    event: DmEvent,
) -> Result<DmState, InvalidTransition> {
    use DmEvent as E;
    use DmState as S;
    match (state, event) {
        (S::Pending, E::CaseVoided) if kind == DmKind::Case => Ok(S::NotNeeded),
        (S::Pending, E::Started) => Ok(S::Sending),
        (S::Pending, E::OpenRefused | E::BudgetSpent)
        | (
            S::Sending,
            E::CreateRefused {
                after_unknown: false,
            },
        ) => Ok(S::Failed),
        (S::Sending, E::Created) => Ok(S::Sent),
        (
            S::Sending,
            E::RetryAfterBeyondWindow {
                after_unknown: false,
            },
        ) => Ok(S::Pending),
        (
            S::Sending,
            E::CreateRefused {
                after_unknown: true,
            }
            | E::RetryAfterBeyondWindow {
                after_unknown: true,
            }
            | E::WindowEnded
            | E::BudgetSpent,
        ) => Ok(S::Unknown),
        _ => Err(InvalidTransition {
            machine: "dm",
            from: state.as_str(),
            event: event.name(),
        }),
    }
}

/// The failure a transition records: for `to == failed`, the refusal's
/// reason, or `discord_unavailable` when the budget ran out before anything
/// was sent; `None` for every other state (only `failed` has a failure).
pub const fn failure_for(
    to: DmState,
    event: DmEvent,
    refusal_code: Option<u32>,
) -> Option<DmFailure> {
    if !matches!(to, DmState::Failed) {
        return None;
    }
    match event {
        DmEvent::OpenRefused
        | DmEvent::CreateRefused {
            after_unknown: false,
        } => Some(DmFailure::from_refusal(refusal_code)),
        DmEvent::BudgetSpent => Some(DmFailure::DiscordUnavailable),
        _ => None,
    }
}

/// Whether reaching `to` wakes a kick or ban waiting on this DM (§6.2,
/// §6.3): a case DM settling as sent, failed, or unknown. (`not_needed`
/// comes from a void, which already cancelled the action.)
pub const fn releases_waiting_action(kind: DmKind, to: DmState) -> bool {
    matches!(kind, DmKind::Case) && matches!(to, DmState::Sent | DmState::Failed | DmState::Unknown)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use time::macros::datetime;

    use super::*;

    fn events() -> Vec<DmEvent> {
        let mut events = vec![
            DmEvent::CaseVoided,
            DmEvent::Started,
            DmEvent::OpenRefused,
            DmEvent::Created,
            DmEvent::WindowEnded,
            DmEvent::BudgetSpent,
        ];
        for after_unknown in [false, true] {
            events.push(DmEvent::CreateRefused { after_unknown });
            events.push(DmEvent::RetryAfterBeyondWindow { after_unknown });
        }
        events
    }

    /// The design's table (§6.2), written out row by row.
    #[allow(clippy::match_same_arms, reason = "one arm per table row")]
    fn expected(kind: DmKind, state: DmState, event: DmEvent) -> Option<DmState> {
        use DmEvent as E;
        use DmState as S;
        Some(match (state, event) {
            (S::Pending, E::CaseVoided) if kind == DmKind::Case => S::NotNeeded,
            (S::Pending, E::Started) => S::Sending,
            (S::Pending, E::OpenRefused) => S::Failed,
            (S::Pending, E::BudgetSpent) => S::Failed,
            (S::Sending, E::Created) => S::Sent,
            (
                S::Sending,
                E::CreateRefused {
                    after_unknown: false,
                },
            ) => S::Failed,
            (
                S::Sending,
                E::RetryAfterBeyondWindow {
                    after_unknown: false,
                },
            ) => S::Pending,
            (S::Sending, E::WindowEnded) => S::Unknown,
            (
                S::Sending,
                E::CreateRefused {
                    after_unknown: true,
                },
            ) => S::Unknown,
            (
                S::Sending,
                E::RetryAfterBeyondWindow {
                    after_unknown: true,
                },
            ) => S::Unknown,
            (S::Sending, E::BudgetSpent) => S::Unknown,
            _ => return None,
        })
    }

    #[test]
    fn every_pair_matches_the_table() {
        for &kind in DmKind::ALL {
            for &state in DmState::ALL {
                for event in events() {
                    let got = transition(kind, state, event);
                    match expected(kind, state, event) {
                        Some(to) => assert_eq!(got, Ok(to), "{kind:?} {state:?} {event:?}"),
                        None => assert_eq!(
                            got.map_err(|e| (e.machine, e.from, e.event)),
                            Err(("dm", state.as_str(), event.name())),
                            "{kind:?} {state:?} {event:?}"
                        ),
                    }
                }
            }
        }
    }

    #[test]
    fn appeal_and_ticket_dms_are_not_cancelled_by_a_void() {
        assert!(transition(DmKind::AppealUpdate, DmState::Pending, DmEvent::CaseVoided).is_err());
        assert!(transition(DmKind::TicketClosed, DmState::Pending, DmEvent::CaseVoided).is_err());
    }

    #[test]
    fn failure_reasons() {
        assert_eq!(DmFailure::from_refusal(Some(50_007)), DmFailure::DmsClosed);
        assert_eq!(
            DmFailure::from_refusal(Some(50_278)),
            DmFailure::NotInSharedServer
        );
        assert_eq!(
            DmFailure::from_refusal(Some(50_013)),
            DmFailure::DiscordRefused
        );
        assert_eq!(DmFailure::from_refusal(None), DmFailure::DiscordRefused);
        let failed = DmState::Failed;
        assert_eq!(
            failure_for(failed, DmEvent::OpenRefused, Some(50_007)),
            Some(DmFailure::DmsClosed)
        );
        assert_eq!(
            failure_for(failed, DmEvent::BudgetSpent, None),
            Some(DmFailure::DiscordUnavailable)
        );
        assert_eq!(
            failure_for(
                DmState::Unknown,
                DmEvent::CreateRefused {
                    after_unknown: true
                },
                Some(50_007)
            ),
            None
        );
        // The budget spent while `sending` settles `unknown`: no failure.
        for from in [DmState::Pending, DmState::Sending] {
            let to = transition(DmKind::Case, from, DmEvent::BudgetSpent).unwrap();
            let failure = failure_for(to, DmEvent::BudgetSpent, None);
            assert_eq!(failure.is_some(), to == DmState::Failed, "{from:?}");
        }
        let names: Vec<&str> = DmFailure::ALL.iter().map(|f| f.as_str()).collect();
        assert_eq!(
            names,
            [
                "dms_closed",
                "not_in_shared_server",
                "discord_unavailable",
                "discord_refused"
            ]
        );
    }

    #[test]
    fn window() {
        let start = datetime!(2026-11-17 12:00 UTC);
        assert!(in_window(start, start));
        assert!(in_window(start, start + Duration::seconds(59)));
        assert!(!in_window(start, start + Duration::seconds(60)));
        assert!(wait_fits(
            start,
            start + Duration::seconds(50),
            Duration::seconds(9)
        ));
        assert!(!wait_fits(
            start,
            start + Duration::seconds(50),
            Duration::seconds(10)
        ));
        assert_eq!(ACTION_WAIT, Duration::seconds(120));
        // Stored extremes never panic.
        let max = OffsetDateTime::new_utc(time::Date::MAX, time::Time::MAX);
        let min = OffsetDateTime::new_utc(time::Date::MIN, time::Time::MIDNIGHT);
        assert!(in_window(max, max));
        assert!(!in_window(min, max));
        assert!(!wait_fits(start, start, Duration::MAX));
        assert!(!wait_fits(min, max, Duration::MAX));
        assert!(wait_fits(max, max, Duration::ZERO));
    }

    #[test]
    fn a_resumed_sending_dm_counts_as_maybe_sent() {
        // A worker crashed after a create Discord may have accepted. The
        // next run finds `sending`; a rate limit past the window must not
        // put it back to `pending`, and a refusal must not fail it.
        let resumed = SendHistory::at_start(DmState::Sending);
        assert!(resumed.maybe_sent());
        for event in [resumed.retry_after_beyond_window(), resumed.refused()] {
            assert_eq!(
                transition(DmKind::Case, DmState::Sending, event),
                Ok(DmState::Unknown)
            );
        }
        let fresh = SendHistory::at_start(DmState::Pending);
        assert!(!fresh.after(CreateResult::NotSent).maybe_sent());
        assert!(!fresh.after(CreateResult::RetryAfter).maybe_sent());
        assert!(fresh.after(CreateResult::Unknown).maybe_sent());
        assert_eq!(
            transition(DmKind::Case, DmState::Sending, fresh.refused()),
            Ok(DmState::Failed)
        );
    }

    #[test]
    fn releases() {
        for &state in DmState::ALL {
            let release = matches!(state, DmState::Sent | DmState::Failed | DmState::Unknown);
            assert_eq!(
                releases_waiting_action(DmKind::Case, state),
                release,
                "{state:?}"
            );
            assert!(!releases_waiting_action(DmKind::AppealUpdate, state));
        }
    }

    /// One step of a simulated `send_dm` job, across runs and crashes.
    #[derive(Clone, Copy, Debug)]
    enum Step {
        Create(CreateResult),
        /// Discord accepted the create, then the worker crashed before
        /// settling: the row stays `sending`.
        SentThenCrash,
        /// The worker crashed between creates.
        Crash,
        /// A rate limit whose wait doesn't fit in the window.
        RetryBeyondWindow,
        WindowEnded,
        BudgetSpent,
    }

    fn steps() -> impl Strategy<Value = Step> {
        prop_oneof![
            prop::sample::select(vec![
                CreateResult::Sent,
                CreateResult::Refused,
                CreateResult::NotSent,
                CreateResult::Unknown,
            ])
            .prop_map(Step::Create),
            Just(Step::SentThenCrash),
            Just(Step::Crash),
            Just(Step::RetryBeyondWindow),
            Just(Step::WindowEnded),
            Just(Step::BudgetSpent),
        ]
    }

    proptest! {
        /// Runs a DM through runs that crash at any point. Whatever the
        /// order, once a create may have reached Discord the DM ends only
        /// `sent` or `unknown`, and a settled DM never leaves its state.
        #[test]
        fn a_maybe_sent_dm_survives_crashes(
            kind in prop::sample::select(DmKind::ALL),
            steps in prop::collection::vec(steps(), 0..40),
        ) {
            let mut state = DmState::Pending;
            let mut reached_discord = false;
            let mut history = SendHistory::at_start(state);
            for step in steps {
                if state.is_terminal() {
                    break;
                }
                if state == DmState::Pending {
                    // A fresh run commits the start itself.
                    history = SendHistory::at_start(DmState::Pending);
                    state = transition(kind, state, DmEvent::Started).unwrap();
                }
                let event = match step {
                    Step::Create(CreateResult::Sent) => Some(DmEvent::Created),
                    Step::Create(CreateResult::Refused) => Some(history.refused()),
                    Step::Create(other) => {
                        reached_discord |= other == CreateResult::Unknown;
                        history = history.after(other);
                        None
                    }
                    Step::SentThenCrash | Step::Crash => {
                        reached_discord |= matches!(step, Step::SentThenCrash);
                        // The next run loads the row, still `sending`.
                        history = SendHistory::at_start(state);
                        None
                    }
                    Step::RetryBeyondWindow => Some(history.retry_after_beyond_window()),
                    Step::WindowEnded => Some(DmEvent::WindowEnded),
                    Step::BudgetSpent => Some(DmEvent::BudgetSpent),
                };
                let Some(event) = event else { continue };
                let next = transition(kind, state, event).unwrap();
                if reached_discord {
                    prop_assert!(
                        matches!(next, DmState::Sent | DmState::Unknown),
                        "{state:?} -> {next:?} via {event:?} after a possible send"
                    );
                }
                if next == DmState::Pending {
                    // A fresh run starts again; first_attempt_at resets.
                    history = SendHistory::at_start(DmState::Pending);
                }
                state = next;
            }
        }

        #[test]
        fn a_dm_that_may_have_been_sent_is_never_sent_again(
            kind in prop::sample::select(DmKind::ALL),
            events in prop::collection::vec(prop::sample::select(events()), 0..30),
        ) {
            let mut state = DmState::Pending;
            let mut unknown_create = false;
            for event in events {
                if let Ok(next) = transition(kind, state, event) {
                    prop_assert!(!state.is_terminal(), "left {state:?} via {event:?}");
                    if let DmEvent::CreateRefused { after_unknown } | DmEvent::RetryAfterBeyondWindow { after_unknown } = event {
                        unknown_create |= after_unknown;
                    }
                    // Back to pending only when nothing was sent.
                    if next == DmState::Pending {
                        prop_assert_eq!(event, DmEvent::RetryAfterBeyondWindow { after_unknown: false });
                    }
                    // After an unknown create, the DM ends unknown, never failed or pending.
                    if unknown_create {
                        prop_assert_eq!(next, DmState::Unknown);
                    }
                    state = next;
                }
            }
        }
    }
}
