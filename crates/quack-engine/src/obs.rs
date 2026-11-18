//! Telemetry for the engine (§18): use-case spans, domain span events, and
//! the mapping from domain values to the catalog's small label enums. Ids
//! go on spans, never on metrics.

use std::future::Future;

use opentelemetry::KeyValue;
use quack_core::DomainError;
use quack_core::action::{ActionFailure, ActionKind, ActionState, AttemptOutcome};
use quack_core::appeal::AppealEventKind;
use quack_core::case::{CaseSource, VoidCause};
use quack_core::dm::{DmFailure, DmKind, DmState};
use quack_core::ids::GuildId;
use quack_core::permissions::MemberRefusal;
use quack_telemetry::labels;
use quack_telemetry::spans::{self, SpanEvent};
use tracing::{Instrument as _, Span};

/// Runs `work` inside a `use_case {name}` span (§18.4).
pub(crate) async fn use_case<T>(
    name: &'static str,
    guild: Option<GuildId>,
    work: impl Future<Output = T>,
) -> T {
    work.instrument(spans::use_case(name, guild.map(GuildId::get)))
        .await
}

/// Adds a domain event to the current span.
pub(crate) fn event(event: SpanEvent, attributes: Vec<KeyValue>) {
    spans::event(&Span::current(), event, attributes);
}

/// A string attribute for a span event.
pub(crate) fn kv(key: &'static str, value: impl std::fmt::Display) -> KeyValue {
    KeyValue::new(key, value.to_string())
}

pub(crate) const fn action_kind(kind: ActionKind) -> labels::ActionKind {
    match kind {
        ActionKind::Timeout => labels::ActionKind::Timeout,
        ActionKind::Kick => labels::ActionKind::Kick,
        ActionKind::Ban => labels::ActionKind::Ban,
        ActionKind::RemoveTimeout => labels::ActionKind::RemoveTimeout,
        ActionKind::Unban => labels::ActionKind::Unban,
    }
}

/// The terminal states `quack.actions.finished` counts; `None` otherwise.
pub(crate) const fn action_state(state: ActionState) -> Option<labels::ActionState> {
    match state {
        ActionState::Done => Some(labels::ActionState::Done),
        ActionState::Failed => Some(labels::ActionState::Failed),
        ActionState::Cancelled => Some(labels::ActionState::Cancelled),
        ActionState::Skipped => Some(labels::ActionState::Skipped),
        ActionState::Pending | ActionState::Running => None,
    }
}

pub(crate) const fn attempt_outcome(outcome: AttemptOutcome) -> labels::AttemptOutcome {
    match outcome {
        AttemptOutcome::Done => labels::AttemptOutcome::Done,
        AttemptOutcome::NoChange => labels::AttemptOutcome::NoChange,
        AttemptOutcome::Refused => labels::AttemptOutcome::Refused,
        AttemptOutcome::NotSent => labels::AttemptOutcome::NotSent,
        AttemptOutcome::Unknown => labels::AttemptOutcome::Unknown,
        AttemptOutcome::Interrupted => labels::AttemptOutcome::Interrupted,
    }
}

/// `ActionFailure` → `quack.reason` (§18.2).
pub(crate) const fn action_failure(failure: ActionFailure) -> labels::Reason {
    use labels::Reason as R;
    match failure {
        ActionFailure::QuackMissingPermission => R::MissingPermission,
        ActionFailure::QuackRoleTooLow => R::RoleHierarchy,
        ActionFailure::MemberIsOwner => R::MemberIsOwner,
        ActionFailure::MemberNotInServer => R::MemberNotFound,
        ActionFailure::QuackNotInServer | ActionFailure::MemberIsAdministrator => R::NotAllowed,
        ActionFailure::TimeoutWindowPassed => R::Invalid,
        ActionFailure::DiscordRefused => R::Discord,
        ActionFailure::RetriesExhausted => R::RetriesExhausted,
        ActionFailure::Unconfirmed => R::Timeout,
    }
}

/// Why an attempt is retried, for `quack.action.retries`.
pub(crate) const fn retry_reason(outcome: AttemptOutcome, rate_limited: bool) -> labels::Reason {
    match outcome {
        AttemptOutcome::NotSent if rate_limited => labels::Reason::RateLimited,
        AttemptOutcome::NotSent => labels::Reason::Discord,
        AttemptOutcome::Interrupted => labels::Reason::Internal,
        _ => labels::Reason::Timeout,
    }
}

/// `DmFailure` → `quack.reason` (§18.2).
pub(crate) const fn dm_failure(failure: DmFailure) -> labels::Reason {
    match failure {
        DmFailure::DmsClosed => labels::Reason::DmsClosed,
        DmFailure::NotInSharedServer => labels::Reason::NoSharedServer,
        DmFailure::DiscordUnavailable => labels::Reason::RetriesExhausted,
        DmFailure::DiscordRefused => labels::Reason::Discord,
    }
}

pub(crate) const fn dm_kind(kind: DmKind) -> labels::DmKind {
    match kind {
        DmKind::Case => labels::DmKind::Case,
        DmKind::AppealUpdate => labels::DmKind::AppealUpdate,
        DmKind::TicketClosed => labels::DmKind::TicketClosed,
    }
}

/// A settled DM state → `quack.dm.outcome`; `None` while in progress.
pub(crate) const fn dm_outcome(state: DmState) -> Option<labels::DmOutcome> {
    match state {
        DmState::Sent => Some(labels::DmOutcome::Sent),
        DmState::Failed => Some(labels::DmOutcome::Failed),
        DmState::Unknown => Some(labels::DmOutcome::Unknown),
        DmState::NotNeeded => Some(labels::DmOutcome::NotNeeded),
        DmState::Pending | DmState::Sending => None,
    }
}

/// Counts a DM reaching a terminal state and adds its span event.
pub(crate) fn dm_settled(
    dm: quack_core::ids::DmId,
    kind: DmKind,
    state: DmState,
    failure: Option<DmFailure>,
) {
    if let Some(outcome) = dm_outcome(state) {
        let reason = match state {
            DmState::Unknown => Some(labels::Reason::Timeout),
            _ => failure.map(dm_failure),
        };
        quack_telemetry::metrics()
            .dms
            .outcome(dm_kind(kind), outcome, reason);
    }
    event(
        SpanEvent::DmOutcome,
        vec![
            kv("dm.id", dm.get()),
            kv("kind", kind.as_str()),
            kv("state", state.as_str()),
            kv("failure", failure.map_or("", DmFailure::as_str)),
        ],
    );
}

pub(crate) const fn case_source(source: CaseSource) -> labels::CaseSource {
    match source {
        CaseSource::Discord => labels::CaseSource::Discord,
        CaseSource::Dashboard | CaseSource::Imported => labels::CaseSource::Dashboard,
        CaseSource::Honeypot => labels::CaseSource::Honeypot,
    }
}

pub(crate) const fn void_cause(cause: VoidCause) -> labels::VoidCause {
    match cause {
        VoidCause::Staff => labels::VoidCause::Staff,
        VoidCause::AppealAccepted => labels::VoidCause::AppealAccepted,
    }
}

/// A refusal of case creation → `quack.reason` (§18.2).
pub(crate) const fn refusal_reason(error: DomainError) -> labels::Reason {
    use labels::Reason as R;
    match error {
        DomainError::MemberRefused(refusal) => match refusal {
            MemberRefusal::MemberRoleTooHigh | MemberRefusal::QuackRoleTooLow => R::RoleHierarchy,
            MemberRefusal::MemberNotInServer | MemberRefusal::ActorNotInServer => R::MemberNotFound,
            MemberRefusal::MemberIsOwner => R::MemberIsOwner,
            MemberRefusal::QuackMissingPermission(_) => R::MissingPermission,
            MemberRefusal::QuackNotInServer
            | MemberRefusal::MemberIsSelf
            | MemberRefusal::MemberIsBot
            | MemberRefusal::MemberIsAdministrator => R::NotAllowed,
        },
        DomainError::MissingCapability(_) => R::MissingPermission,
        DomainError::MfaRequired => R::NotAllowed,
        DomainError::RuleChanged | DomainError::AppealChanged => R::Conflict,
        _ => R::Invalid,
    }
}

/// An appeal event → `quack.appeal.transition`.
pub(crate) const fn appeal_transition(kind: AppealEventKind) -> labels::AppealTransition {
    use labels::AppealTransition as T;
    match kind {
        AppealEventKind::Submitted => T::Submit,
        AppealEventKind::InformationRequested => T::RequestInfo,
        AppealEventKind::InformationProvided => T::Reply,
        AppealEventKind::Accepted => T::Accept,
        AppealEventKind::Rejected => T::Reject,
        AppealEventKind::Closed => T::Close,
        AppealEventKind::Reopened => T::Reopen,
    }
}

/// Seconds between two times, never negative, as a `Duration`.
pub(crate) fn since(
    earlier: time::OffsetDateTime,
    later: time::OffsetDateTime,
) -> std::time::Duration {
    std::time::Duration::try_from(later - earlier).unwrap_or_default()
}

pub(crate) const fn message_kind(kind: quack_core::messages::MessageKind) -> labels::MessageKind {
    use quack_core::messages::MessageKind as K;
    match kind {
        K::CaseReceipt => labels::MessageKind::CaseReceipt,
        K::AppealPost => labels::MessageKind::AppealPost,
        K::TicketPanel => labels::MessageKind::TicketPanel,
        K::TicketPost => labels::MessageKind::TicketPost,
        K::HoneypotWarning => labels::MessageKind::HoneypotWarning,
    }
}
