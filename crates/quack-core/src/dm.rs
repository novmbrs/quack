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

/// Attempts of the `send_dm` job (§6.4); a run that ends with the DM still
/// `pending` uses one.
pub const SEND_ATTEMPTS: u32 = 3;
/// Waits between `send_dm` job attempts.
pub const SEND_BACKOFF: [Duration; 2] = [Duration::seconds(5), Duration::seconds(30)];
/// After a create with an unknown result, wait this long and repeat it with
/// the same nonce while the window lasts.
pub const UNKNOWN_REPEAT_DELAY: Duration = Duration::seconds(2);
/// The longest a kick or ban waits for its case DM (§6.3): the action's
/// job runs at `created_at + 120 s`, moved earlier when the DM settles.
pub const ACTION_WAIT: Duration = Duration::seconds(120);

/// Whether a create may still be repeated with the same nonce: less than
/// [`NONCE_WINDOW`] has passed since `first_attempt_at` (§10.4).
pub fn in_window(first_attempt_at: OffsetDateTime, now: OffsetDateTime) -> bool {
    now < first_attempt_at + NONCE_WINDOW
}

/// Whether waiting `wait` from `now` still ends inside the window.
pub fn wait_fits(first_attempt_at: OffsetDateTime, now: OffsetDateTime, wait: Duration) -> bool {
    in_window(first_attempt_at, now + wait)
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
        /// Whether an earlier create of this DM had an unknown result.
        after_unknown: bool,
    },
    /// A rate limit's wait does not fit in the window.
    RetryAfterBeyondWindow {
        /// Whether an earlier create of this DM had an unknown result.
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

/// The failure a transition to `failed` records: the refusal's reason, or
/// `discord_unavailable` when the budget ran out before anything was sent.
pub const fn failure_for(event: DmEvent, refusal_code: Option<u32>) -> Option<DmFailure> {
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
        assert_eq!(
            failure_for(DmEvent::OpenRefused, Some(50_007)),
            Some(DmFailure::DmsClosed)
        );
        assert_eq!(
            failure_for(DmEvent::BudgetSpent, None),
            Some(DmFailure::DiscordUnavailable)
        );
        assert_eq!(
            failure_for(
                DmEvent::CreateRefused {
                    after_unknown: true
                },
                Some(50_007)
            ),
            None
        );
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

    proptest! {
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
