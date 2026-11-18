//! Appeals (§8): eligibility, states, and the transition table.

use crate::case::CaseSource;
use crate::{DomainError, limits};

str_enum! {
    /// An appeal's stored state (`appeal_state`).
    pub enum AppealState {
        /// Waiting for staff.
        Pending = "pending",
        /// Staff asked the member for more information.
        NeedsInformation = "needs_information",
        /// Staff accepted; the case is voided. Terminal.
        Accepted = "accepted",
        /// Staff rejected; the case stands.
        Rejected = "rejected",
        /// Staff closed it, or Quack did because the case was voided
        /// (terminal then).
        Closed = "closed",
    }
}

impl AppealState {
    /// Whether the appeal awaits staff or the member (the "open" gauge).
    pub const fn is_open(self) -> bool {
        matches!(self, Self::Pending | Self::NeedsInformation)
    }
}

str_enum! {
    /// One event on an appeal's timeline (`appeal_event_kind`).
    pub enum AppealEventKind {
        /// The member submitted the appeal.
        Submitted = "submitted",
        /// Staff asked for more information.
        InformationRequested = "information_requested",
        /// The member replied with information.
        InformationProvided = "information_provided",
        /// Staff accepted.
        Accepted = "accepted",
        /// Staff rejected.
        Rejected = "rejected",
        /// Staff or Quack closed it.
        Closed = "closed",
        /// Staff reopened it.
        Reopened = "reopened",
    }
}

/// A staff decision on an appeal; each needs a reason of 1–2,000 characters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppealDecision {
    /// Ask the member for more information.
    RequestInformation,
    /// Accept: void the case.
    Accept,
    /// Reject: the case stands.
    Reject,
    /// Close without a decision.
    Close,
    /// Reopen a rejected or closed appeal (asks for information).
    Reopen,
}

impl AppealDecision {
    /// The timeline event this decision records.
    pub const fn event_kind(self) -> AppealEventKind {
        match self {
            Self::RequestInformation => AppealEventKind::InformationRequested,
            Self::Accept => AppealEventKind::Accepted,
            Self::Reject => AppealEventKind::Rejected,
            Self::Close => AppealEventKind::Closed,
            Self::Reopen => AppealEventKind::Reopened,
        }
    }
}

/// Who asks for an appeal transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppealActor {
    /// The case's member.
    Member,
    /// A moderator.
    Staff,
}

/// An appeal row as a transition reads it, under the case and appeal locks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppealRow {
    /// Its state.
    pub state: AppealState,
    /// Its version (incremented by every transition).
    pub version: u32,
    /// Whether its latest `closed` event was Quack closing it because the
    /// case was voided; such an appeal can't be reopened.
    pub closed_by_void: bool,
}

/// What a transition writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppealTransition {
    /// The new state.
    pub to: AppealState,
    /// The new version.
    pub version: u32,
}

/// Applies a staff or member event to an appeal (§8.2). Every transition
/// needs a valid case (else `case_voided`), the `expected_version` the
/// person saw (else `appeal_changed`), and a row of the table for the
/// state, event, and actor (else `appeal_changed`). `submitted` is not a
/// transition (see [`eligibility`]); Quack's close inside a void is
/// [`close_by_void`].
#[allow(clippy::match_same_arms, reason = "one arm per table row")]
pub fn transition(
    row: &AppealRow,
    case_valid: bool,
    event: AppealEventKind,
    actor: AppealActor,
    expected_version: u32,
) -> Result<AppealTransition, DomainError> {
    use AppealActor as A;
    use AppealEventKind as E;
    use AppealState as S;
    if !case_valid {
        return Err(DomainError::CaseVoided);
    }
    let version = next_version(row.version, expected_version)?;
    let to = match (row.state, event, actor) {
        (S::Pending, E::InformationRequested, A::Staff) => S::NeedsInformation,
        (S::Rejected, E::Reopened, A::Staff) => S::NeedsInformation,
        (S::Closed, E::Reopened, A::Staff) if !row.closed_by_void => S::NeedsInformation,
        (S::NeedsInformation, E::InformationProvided, A::Member) => S::Pending,
        (S::Pending, E::Accepted, A::Staff) => S::Accepted,
        (S::Pending, E::Rejected, A::Staff) => S::Rejected,
        (S::Pending | S::NeedsInformation, E::Closed, A::Staff) => S::Closed,
        _ => return Err(DomainError::AppealChanged),
    };
    Ok(AppealTransition { to, version })
}

/// Quack closing an open appeal inside a direct void of its case (§4.7):
/// the event is `closed` with `closed_by_void = true`, actor NULL, and text
/// [`CLOSED_BECAUSE_VOIDED`]. `None` when the appeal is not open (nothing
/// to close). Not used when the void is the appeal's own acceptance.
pub const fn close_by_void(row: &AppealRow) -> Option<AppealTransition> {
    if row.state.is_open() {
        Some(AppealTransition {
            to: AppealState::Closed,
            version: row.version.saturating_add(1),
        })
    } else {
        None
    }
}

/// Checks a transition's `expected_version` against the row's and
/// returns the version the transition writes (§8.2).
pub fn next_version(current: u32, expected: u32) -> Result<u32, DomainError> {
    if current == expected {
        current.checked_add(1).ok_or(DomainError::AppealChanged)
    } else {
        Err(DomainError::AppealChanged)
    }
}

/// Facts about a case when its member tries to appeal it (§8.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent facts, each checked in order"
)]
pub struct EligibilityFacts {
    /// Whether the person asking is the case's member.
    pub is_member: bool,
    /// Whether the case is valid.
    pub case_valid: bool,
    /// Whether the case's rule version was appealable.
    pub appealable: bool,
    /// Whether the case already has an appeal.
    pub has_appeal: bool,
    /// The case's source.
    pub source: CaseSource,
}

/// Whether the member may appeal. Someone else's case reads as not found,
/// so members can't learn whether a case exists in another member's name.
pub fn eligibility(facts: &EligibilityFacts) -> Result<(), DomainError> {
    if !facts.is_member {
        Err(DomainError::CaseNotFound)
    } else if facts.has_appeal {
        Err(DomainError::AppealExists)
    } else if !facts.case_valid {
        Err(DomainError::CaseVoided)
    } else if !facts.appealable || facts.source == CaseSource::Imported {
        Err(DomainError::AppealNotAllowed)
    } else {
        Ok(())
    }
}

/// Whether a statement or information reply has an allowed length.
pub fn statement_ok(text: &str) -> bool {
    limits::APPEAL_TEXT.allows(text)
}

/// Whether a staff decision reason has an allowed length.
pub fn decision_reason_ok(text: &str) -> bool {
    limits::APPEAL_DECISION_REASON.allows(text)
}

/// The appeal event text Quack records when a void closes the appeal
/// (`appeal.closed_by_void`).
pub const CLOSED_BECAUSE_VOIDED: &str = "Closed because the case was voided.";

/// The fixed void reason stored on a case voided by accepting its appeal
/// (`case.void.reason.appeal_accepted`).
pub const APPEAL_ACCEPTED_VOID_REASON: &str = "Appeal accepted.";

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    const ACTORS: [AppealActor; 2] = [AppealActor::Member, AppealActor::Staff];

    fn row(state: AppealState, closed_by_void: bool) -> AppealRow {
        AppealRow {
            state,
            version: 3,
            closed_by_void,
        }
    }

    /// The design's table (§8.2), row by row.
    fn expected(
        state: AppealState,
        closed_by_void: bool,
        event: AppealEventKind,
        actor: AppealActor,
    ) -> Option<AppealState> {
        use AppealActor as A;
        use AppealEventKind as E;
        use AppealState as S;
        let rows: &[(&[S], E, A, S)] = &[
            (
                &[S::Pending],
                E::InformationRequested,
                A::Staff,
                S::NeedsInformation,
            ),
            (
                &[S::NeedsInformation],
                E::InformationProvided,
                A::Member,
                S::Pending,
            ),
            (&[S::Pending], E::Accepted, A::Staff, S::Accepted),
            (&[S::Pending], E::Rejected, A::Staff, S::Rejected),
            (
                &[S::Pending, S::NeedsInformation],
                E::Closed,
                A::Staff,
                S::Closed,
            ),
            (
                &[S::Rejected, S::Closed],
                E::Reopened,
                A::Staff,
                S::NeedsInformation,
            ),
        ];
        if state == S::Closed && closed_by_void && event == E::Reopened {
            return None;
        }
        rows.iter()
            .find(|(from, e, a, _)| from.contains(&state) && *e == event && *a == actor)
            .map(|r| r.3)
    }

    #[test]
    fn every_case_matches_the_table() {
        for &state in AppealState::ALL {
            for by_void in [false, true] {
                for &event in AppealEventKind::ALL {
                    for actor in ACTORS {
                        let want = expected(state, by_void, event, actor)
                            .map(|to| AppealTransition { to, version: 4 })
                            .ok_or(DomainError::AppealChanged);
                        assert_eq!(
                            transition(&row(state, by_void), true, event, actor, 3),
                            want,
                            "{state:?} {by_void} {event:?} {actor:?}"
                        );
                        // Every transition needs a valid case, reopen included.
                        assert_eq!(
                            transition(&row(state, by_void), false, event, actor, 3),
                            Err(DomainError::CaseVoided)
                        );
                        // And the version the person saw.
                        assert_eq!(
                            transition(&row(state, by_void), true, event, actor, 2),
                            Err(DomainError::AppealChanged)
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn an_old_reply_never_answers_a_newer_request() {
        let asked = row(AppealState::NeedsInformation, false);
        let reply = |v| {
            transition(
                &asked,
                true,
                AppealEventKind::InformationProvided,
                AppealActor::Member,
                v,
            )
        };
        assert_eq!(
            reply(3),
            Ok(AppealTransition {
                to: AppealState::Pending,
                version: 4
            })
        );
        assert_eq!(reply(1), Err(DomainError::AppealChanged));
    }

    #[test]
    fn closing_by_void() {
        for state in [AppealState::Pending, AppealState::NeedsInformation] {
            assert_eq!(
                close_by_void(&row(state, false)),
                Some(AppealTransition {
                    to: AppealState::Closed,
                    version: 4
                })
            );
        }
        for state in [
            AppealState::Accepted,
            AppealState::Rejected,
            AppealState::Closed,
        ] {
            assert_eq!(close_by_void(&row(state, false)), None);
        }
        // An appeal closed by a void can't be reopened.
        let closed = row(AppealState::Closed, true);
        assert_eq!(
            transition(
                &closed,
                true,
                AppealEventKind::Reopened,
                AppealActor::Staff,
                3
            ),
            Err(DomainError::AppealChanged)
        );
        assert_eq!(APPEAL_ACCEPTED_VOID_REASON, "Appeal accepted.");
    }

    #[test]
    fn accepted_is_terminal_and_submitted_is_never_a_transition() {
        for &event in AppealEventKind::ALL {
            for actor in ACTORS {
                assert!(
                    transition(&row(AppealState::Accepted, false), true, event, actor, 3).is_err()
                );
            }
        }
        for &state in AppealState::ALL {
            for actor in ACTORS {
                assert!(
                    transition(
                        &row(state, false),
                        true,
                        AppealEventKind::Submitted,
                        actor,
                        3
                    )
                    .is_err()
                );
            }
        }
    }

    #[test]
    fn decisions_map_to_events() {
        let decisions = [
            (
                AppealDecision::RequestInformation,
                AppealEventKind::InformationRequested,
            ),
            (AppealDecision::Accept, AppealEventKind::Accepted),
            (AppealDecision::Reject, AppealEventKind::Rejected),
            (AppealDecision::Close, AppealEventKind::Closed),
            (AppealDecision::Reopen, AppealEventKind::Reopened),
        ];
        for (decision, event) in decisions {
            assert_eq!(decision.event_kind(), event);
        }
    }

    #[test]
    fn versions() {
        assert_eq!(next_version(1, 1), Ok(2));
        assert_eq!(next_version(2, 1), Err(DomainError::AppealChanged));
        assert_eq!(
            next_version(u32::MAX, u32::MAX),
            Err(DomainError::AppealChanged)
        );
    }

    #[test]
    fn eligibility_order() {
        let ok = EligibilityFacts {
            is_member: true,
            case_valid: true,
            appealable: true,
            has_appeal: false,
            source: CaseSource::Discord,
        };
        assert_eq!(eligibility(&ok), Ok(()));
        let cases = [
            (
                EligibilityFacts {
                    is_member: false,
                    has_appeal: true,
                    ..ok
                },
                DomainError::CaseNotFound,
            ),
            (
                EligibilityFacts {
                    has_appeal: true,
                    case_valid: false,
                    ..ok
                },
                DomainError::AppealExists,
            ),
            (
                EligibilityFacts {
                    case_valid: false,
                    appealable: false,
                    ..ok
                },
                DomainError::CaseVoided,
            ),
            (
                EligibilityFacts {
                    appealable: false,
                    ..ok
                },
                DomainError::AppealNotAllowed,
            ),
            (
                EligibilityFacts {
                    source: CaseSource::Imported,
                    ..ok
                },
                DomainError::AppealNotAllowed,
            ),
        ];
        for (facts, err) in cases {
            assert_eq!(eligibility(&facts), Err(err), "{facts:?}");
        }
    }

    #[test]
    fn text_limits() {
        assert!(statement_ok("x"));
        assert!(!statement_ok(" "));
        assert!(statement_ok(&"x".repeat(4000)));
        assert!(!statement_ok(&"x".repeat(4001)));
        assert!(decision_reason_ok(&"x".repeat(2000)));
        assert!(!decision_reason_ok(&"x".repeat(2001)));
        assert!(AppealState::NeedsInformation.is_open());
        assert!(!AppealState::Rejected.is_open());
    }

    fn event_strategy() -> impl Strategy<Value = (AppealEventKind, AppealActor, bool)> {
        (
            prop::sample::select(AppealEventKind::ALL),
            prop::sample::select(ACTORS.to_vec()),
            any::<bool>(),
        )
    }

    proptest! {
        #[test]
        fn random_events_stay_in_the_table(events in prop::collection::vec(event_strategy(), 0..40)) {
            let mut row = AppealRow { state: AppealState::Pending, version: 1, closed_by_void: false };
            for (event, actor, quack_void) in events {
                let before = row;
                if quack_void {
                    if let Some(t) = close_by_void(&row) {
                        prop_assert!(before.state.is_open());
                        row = AppealRow { state: t.to, version: t.version, closed_by_void: true };
                    }
                } else if let Ok(t) = transition(&row, true, event, actor, row.version) {
                    prop_assert_eq!(Some(t.to), expected(before.state, before.closed_by_void, event, actor));
                    prop_assert_eq!(t.version, before.version + 1);
                    let closed_by_void = if t.to == AppealState::Closed { false } else { before.closed_by_void };
                    row = AppealRow { state: t.to, version: t.version, closed_by_void };
                }
                // Accepted and closed-by-void never change again.
                if before.state == AppealState::Accepted
                    || (before.state == AppealState::Closed && before.closed_by_void)
                {
                    prop_assert_eq!(row, before);
                }
            }
        }
    }
}
