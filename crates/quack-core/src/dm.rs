//! DMs (§6): the one direct message per case, appeal decision, or closed
//! ticket, sent at most once.

use time::Duration;

use crate::InvalidTransition;

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
        /// Recorded just before the send call. Outcome: in progress.
        Sending = "sending",
        /// Discord returned the message. Outcome: done.
        Sent = "sent",
        /// Discord definitely refused. Outcome: failed.
        Failed = "failed",
        /// The send started but its result was never learned. Terminal: never
        /// sent again. Outcome: couldn't confirm.
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
    /// Why a DM failed (`dm_failure`).
    pub enum DmFailure {
        /// The member's DMs are closed, or they blocked Quack.
        DmsClosed = "dms_closed",
        /// The member no longer shares a server with Quack.
        NotInSharedServer = "not_in_shared_server",
        /// Discord didn't open a DM channel after three tries.
        DmChannelUnavailable = "dm_channel_unavailable",
        /// Discord refused for another reason.
        DiscordRefused = "discord_refused",
    }
}

const CANNOT_DM: u32 = 50_007;
const NO_MUTUAL_GUILDS: u32 = 50_278;

impl DmFailure {
    /// Classifies a definite refusal (§6.5) by Discord error code and
    /// whether the member is still in the server.
    pub const fn from_refusal(code: Option<u32>, member_in_server: bool) -> Self {
        match code {
            Some(CANNOT_DM) if member_in_server => Self::DmsClosed,
            Some(CANNOT_DM | NO_MUTUAL_GUILDS) => Self::NotInSharedServer,
            _ => Self::DiscordRefused,
        }
    }
}

/// Calls to open the DM channel before giving up with
/// `dm_channel_unavailable` (the `send_dm` job's budget).
pub const OPEN_ATTEMPTS: u32 = 3;
/// Waits between DM job attempts, only before `sending`.
pub const OPEN_BACKOFF: [Duration; 2] = [Duration::seconds(5), Duration::seconds(30)];
/// After an unknown send result, wait this long and repeat once with the
/// same nonce.
pub const UNKNOWN_REPEAT_DELAY: Duration = Duration::seconds(2);
/// A rate limit at most this long is waited out inside the job.
pub const MAX_IN_JOB_RATE_LIMIT: Duration = Duration::seconds(10);
/// Most in-job repeats after short rate limits.
pub const MAX_RATE_LIMIT_REPEATS: u32 = 2;

/// An event in the DM transition table (§6.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DmEvent {
    /// The case was voided before sending (case DMs only).
    CaseVoided,
    /// The DM channel is open; about to send.
    Sending,
    /// Opening the DM channel was refused or never succeeded.
    OpenFailed,
    /// Discord returned the message.
    Sent,
    /// Discord refused the message.
    SendRefused,
    /// Rate limited for longer than the job waits; nothing was sent.
    LongRateLimit,
    /// The result is still unknown, or the job found the row already
    /// `sending` (a crash).
    Unconfirmed,
}

impl DmEvent {
    /// The event's name, for errors and logs.
    pub const fn name(self) -> &'static str {
        match self {
            Self::CaseVoided => "case_voided",
            Self::Sending => "sending",
            Self::OpenFailed => "open_failed",
            Self::Sent => "sent",
            Self::SendRefused => "send_refused",
            Self::LongRateLimit => "long_rate_limit",
            Self::Unconfirmed => "unconfirmed",
        }
    }
}

/// Applies one event to a DM of `kind` in `state` (§6.2). Any pair the
/// table does not list is an [`InvalidTransition`].
pub fn transition(
    kind: DmKind,
    state: DmState,
    event: DmEvent,
) -> Result<DmState, InvalidTransition> {
    use DmEvent as E;
    use DmState as S;
    match (state, event) {
        (S::Pending, E::CaseVoided) if kind == DmKind::Case => Ok(S::NotNeeded),
        (S::Pending, E::Sending) => Ok(S::Sending),
        (S::Pending, E::OpenFailed) | (S::Sending, E::SendRefused) => Ok(S::Failed),
        (S::Sending, E::Sent) => Ok(S::Sent),
        (S::Sending, E::LongRateLimit) => Ok(S::Pending),
        (S::Sending, E::Unconfirmed) => Ok(S::Unknown),
        _ => Err(InvalidTransition {
            machine: "dm",
            from: state.as_str(),
            event: event.name(),
        }),
    }
}

/// Whether reaching `to` releases a kick or ban waiting on this DM (§6.3):
/// a case DM settling as sent, failed, or unknown. (`not_needed` comes from
/// a void, which already cancelled the action.)
pub const fn releases_waiting_action(kind: DmKind, to: DmState) -> bool {
    matches!(kind, DmKind::Case) && matches!(to, DmState::Sent | DmState::Failed | DmState::Unknown)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    const EVENTS: [DmEvent; 7] = [
        DmEvent::CaseVoided,
        DmEvent::Sending,
        DmEvent::OpenFailed,
        DmEvent::Sent,
        DmEvent::SendRefused,
        DmEvent::LongRateLimit,
        DmEvent::Unconfirmed,
    ];

    #[allow(clippy::match_same_arms, reason = "one arm per table row")]
    fn expected(kind: DmKind, state: DmState, event: DmEvent) -> Option<DmState> {
        use DmEvent as E;
        use DmState as S;
        Some(match (state, event) {
            (S::Pending, E::CaseVoided) if kind == DmKind::Case => S::NotNeeded,
            (S::Pending, E::Sending) => S::Sending,
            (S::Pending, E::OpenFailed) => S::Failed,
            (S::Sending, E::Sent) => S::Sent,
            (S::Sending, E::SendRefused) => S::Failed,
            (S::Sending, E::LongRateLimit) => S::Pending,
            (S::Sending, E::Unconfirmed) => S::Unknown,
            _ => return None,
        })
    }

    #[test]
    fn every_pair_matches_the_table() {
        for &kind in DmKind::ALL {
            for &state in DmState::ALL {
                for event in EVENTS {
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
        assert_eq!(
            DmFailure::from_refusal(Some(50_007), true),
            DmFailure::DmsClosed
        );
        assert_eq!(
            DmFailure::from_refusal(Some(50_007), false),
            DmFailure::NotInSharedServer
        );
        assert_eq!(
            DmFailure::from_refusal(Some(50_278), true),
            DmFailure::NotInSharedServer
        );
        assert_eq!(
            DmFailure::from_refusal(Some(50_013), true),
            DmFailure::DiscordRefused
        );
        assert_eq!(
            DmFailure::from_refusal(None, false),
            DmFailure::DiscordRefused
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

    proptest! {
        #[test]
        fn terminal_states_never_change(
            kind in prop::sample::select(DmKind::ALL),
            events in prop::collection::vec(prop::sample::select(EVENTS.to_vec()), 0..30),
        ) {
            let mut state = DmState::Pending;
            for event in events {
                if let Ok(next) = transition(kind, state, event) {
                    prop_assert!(!state.is_terminal(), "left {state:?} via {event:?}");
                    // Only a long rate limit (nothing was sent) leads back to
                    // pending, so a send that may have happened is never repeated.
                    if next == DmState::Pending {
                        prop_assert_eq!(event, DmEvent::LongRateLimit);
                    }
                    state = next;
                }
            }
        }
    }
}
