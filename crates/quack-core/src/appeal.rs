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
        /// Staff closed it, or Quack did because the case was voided.
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

/// Who causes an appeal event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppealActor {
    /// The case's member.
    Member,
    /// A moderator.
    Staff,
    /// Quack, closing the appeal because its case was voided directly.
    Quack,
}

/// Applies an event to an appeal (§8.2). The initial `submitted` event is
/// not a transition; see [`eligibility`]. Anything the table does not list,
/// including the wrong actor, is `appeal_changed`.
pub fn transition(
    state: AppealState,
    event: AppealEventKind,
    actor: AppealActor,
) -> Result<AppealState, DomainError> {
    use AppealActor as A;
    use AppealEventKind as E;
    use AppealState as S;
    match (state, event, actor) {
        (S::Pending, E::InformationRequested, A::Staff)
        | (S::Rejected | S::Closed, E::Reopened, A::Staff) => Ok(S::NeedsInformation),
        (S::NeedsInformation, E::InformationProvided, A::Member) => Ok(S::Pending),
        (S::Pending, E::Accepted, A::Staff) => Ok(S::Accepted),
        (S::Pending, E::Rejected, A::Staff) => Ok(S::Rejected),
        (S::Pending | S::NeedsInformation, E::Closed, A::Staff | A::Quack) => Ok(S::Closed),
        _ => Err(DomainError::AppealChanged),
    }
}

/// Checks a staff transition's `expected_version` against the row's and
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

/// The appeal event text Quack records when a void closes the appeal.
pub const CLOSED_BECAUSE_VOIDED: &str = "Closed because the case was voided.";

#[cfg(test)]
mod tests {
    use super::*;

    const ACTORS: [AppealActor; 3] = [AppealActor::Member, AppealActor::Staff, AppealActor::Quack];

    fn expected(
        state: AppealState,
        event: AppealEventKind,
        actor: AppealActor,
    ) -> Option<AppealState> {
        use AppealActor as A;
        use AppealEventKind as E;
        use AppealState as S;
        let rows: &[(&[S], E, &[A], S)] = &[
            (
                &[S::Pending],
                E::InformationRequested,
                &[A::Staff],
                S::NeedsInformation,
            ),
            (
                &[S::NeedsInformation],
                E::InformationProvided,
                &[A::Member],
                S::Pending,
            ),
            (&[S::Pending], E::Accepted, &[A::Staff], S::Accepted),
            (&[S::Pending], E::Rejected, &[A::Staff], S::Rejected),
            (
                &[S::Pending, S::NeedsInformation],
                E::Closed,
                &[A::Staff, A::Quack],
                S::Closed,
            ),
            (
                &[S::Rejected, S::Closed],
                E::Reopened,
                &[A::Staff],
                S::NeedsInformation,
            ),
        ];
        rows.iter()
            .find(|(from, e, actors, _)| {
                from.contains(&state) && *e == event && actors.contains(&actor)
            })
            .map(|row| row.3)
    }

    #[test]
    fn every_triple_matches_the_table() {
        for &state in AppealState::ALL {
            for &event in AppealEventKind::ALL {
                for actor in ACTORS {
                    let want = expected(state, event, actor).ok_or(DomainError::AppealChanged);
                    assert_eq!(
                        transition(state, event, actor),
                        want,
                        "{state:?} {event:?} {actor:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn accepted_is_terminal_and_submitted_is_never_a_transition() {
        for &event in AppealEventKind::ALL {
            for actor in ACTORS {
                assert!(transition(AppealState::Accepted, event, actor).is_err());
            }
        }
        for &state in AppealState::ALL {
            for actor in ACTORS {
                assert!(transition(state, AppealEventKind::Submitted, actor).is_err());
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
}
