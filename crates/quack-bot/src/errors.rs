//! Engine errors → the person's message (`quack-text`) and the telemetry
//! reason. Every refusal a person can cause is a typed domain error mapped
//! through `quack_text::error::for_domain` with the facts the handler
//! knows; infrastructure failures say that nothing changed, or that Quack
//! couldn't confirm it.

use quack_core::DomainError;
use quack_core::case::ContextProblem;
use quack_core::permissions::MemberRefusal;
use quack_core::rule::{Field, FieldKind};
use quack_db::DbError;
use quack_engine::actions::ActionError;
use quack_engine::appeals::AppealError;
use quack_engine::cases::{ApplyError, CaseError};
use quack_engine::settings::SettingsError;
use quack_telemetry::labels::Reason;
use quack_text::Text;
use quack_text::error::{self, ErrorFacts};

use crate::ack::Failure;

/// The telemetry reason for a domain refusal.
const fn domain_reason(err: DomainError) -> Reason {
    match err {
        DomainError::MissingCapability(_) | DomainError::MfaRequired => Reason::NotAllowed,
        DomainError::MemberRefused(r) => match r {
            MemberRefusal::QuackMissingPermission(_) | MemberRefusal::QuackNotInServer => {
                Reason::MissingPermission
            }
            MemberRefusal::MemberRoleTooHigh | MemberRefusal::QuackRoleTooLow => {
                Reason::RoleHierarchy
            }
            MemberRefusal::MemberIsOwner => Reason::MemberIsOwner,
            MemberRefusal::MemberNotInServer | MemberRefusal::ActorNotInServer => {
                Reason::MemberNotFound
            }
            MemberRefusal::MemberIsSelf
            | MemberRefusal::MemberIsBot
            | MemberRefusal::MemberIsAdministrator => Reason::Invalid,
        },
        DomainError::CaseNotFound
        | DomainError::RuleNotFound
        | DomainError::AppealNotFound
        | DomainError::TooManyLinks => Reason::Invalid,
        _ => Reason::Conflict,
    }
}

/// A domain refusal with the facts the caller knows.
pub(crate) fn domain(err: DomainError, facts: &ErrorFacts<'_>) -> Failure {
    Failure::refused(error::for_domain(&err, facts), domain_reason(err))
}

/// A database failure: nothing changed (`error.internal`).
pub(crate) fn db(err: &DbError) -> Failure {
    tracing::warn!(error = %err, "a database call failed");
    Failure::failed(error::internal(), Reason::Database)
}

/// Discord didn't answer: nothing changed (`error.discord_unavailable`).
pub(crate) fn discord_unavailable() -> Failure {
    Failure::failed(error::discord_unavailable(), Reason::Discord)
}

/// A request the engine refused as malformed (an operation key reused for
/// a different request, a length the modal should have prevented):
/// nothing changed.
fn invalid() -> Failure {
    Failure::refused(error::internal(), Reason::Invalid)
}

/// One line per context problem, in the words of `context.*`.
pub(crate) fn context_problems(problems: &[ContextProblem], fields: &[Field]) -> Vec<Text> {
    problems
        .iter()
        .map(|p| {
            let field = fields.iter().find(|f| f.key.as_str() == p.key);
            let kind: Option<FieldKind> = field.map(|f| f.kind);
            quack_text::case::context_problem(p, field.map(|f| f.label.as_str()), kind)
        })
        .collect()
}

/// Applying a rule failed.
pub(crate) fn apply(err: &ApplyError, facts: &ErrorFacts<'_>) -> Failure {
    match err {
        ApplyError::Refused(d) => domain(*d, facts),
        ApplyError::ContextInvalid(_)
        | ApplyError::LinksInvalid(_)
        | ApplyError::OperationKeyReused => invalid(),
        ApplyError::CaseNumbersExhausted => {
            Failure::refused(error::case_numbers_exhausted(), Reason::Conflict)
        }
        ApplyError::HoneypotPolicyChanged => {
            Failure::refused(error::honeypot_policy_changed(), Reason::Conflict)
        }
        ApplyError::DiscordUnavailable => discord_unavailable(),
        ApplyError::Unconfirmed => Failure::failed(error::unconfirmed(), Reason::Timeout),
        ApplyError::Deadline => Failure::failed(error::internal(), Reason::Timeout),
        ApplyError::Db(e) => db(e),
    }
}

/// A reason (void, appeal decision) that is empty or longer than `max`.
pub(crate) fn reason(text: &str, max: usize) -> Failure {
    let words = if text.trim().is_empty() {
        error::reason_empty()
    } else {
        error::reason_too_long(max)
    };
    Failure::refused(words, Reason::Invalid)
}

/// A member's appeal text that is empty or longer than `max`.
pub(crate) fn appeal_text(text: &str, max: usize) -> Failure {
    let words = if text.trim().is_empty() {
        error::appeal_text_empty()
    } else {
        error::appeal_text_too_long(max)
    };
    Failure::refused(words, Reason::Invalid)
}

/// A void, context edit, or evidence addition failed. A void's reason
/// problem is reported with its text by the caller ([`reason`]).
pub(crate) fn case(err: &CaseError, facts: &ErrorFacts<'_>) -> Failure {
    match err {
        CaseError::Refused(d) => domain(*d, facts),
        CaseError::ContextChanged => Failure::refused(error::context_changed(), Reason::Conflict),
        CaseError::NoLinks => Failure::refused(error::no_links(), Reason::Invalid),
        CaseError::ReasonLength => reason("x", quack_core::limits::VOID_REASON.max),
        CaseError::ContextInvalid(_)
        | CaseError::LinksInvalid(_)
        | CaseError::OperationKeyReused => invalid(),
        CaseError::Db(e) => db(e),
    }
}

/// A retry, dismiss, or undo failed.
pub(crate) fn action(err: &ActionError, facts: &ErrorFacts<'_>) -> Failure {
    match err {
        ActionError::NotFound => Failure::refused(error::action_not_found(), Reason::Invalid),
        ActionError::Refused(d) => domain(*d, facts),
        ActionError::OperationKeyReused => invalid(),
        ActionError::DiscordUnavailable => discord_unavailable(),
        ActionError::Db(e) => db(e),
    }
}

/// An appeal operation failed. Text and reason problems are reported with
/// their text by the callers that have it ([`appeal_text`], [`reason`]).
pub(crate) fn appeal(err: &AppealError, facts: &ErrorFacts<'_>) -> Failure {
    match err {
        AppealError::Refused(d) => domain(*d, facts),
        AppealError::TextLength => appeal_text("x", quack_core::limits::APPEAL_TEXT.max),
        AppealError::ReasonLength => reason("x", quack_core::limits::APPEAL_DECISION_REASON.max),
        AppealError::OperationKeyReused => invalid(),
        AppealError::Db(e) => db(e),
    }
}

/// A settings change failed (field problems are rendered by `/setup`).
pub(crate) fn settings(err: &SettingsError) -> Failure {
    match err {
        SettingsError::Refused(d) => domain(*d, &ErrorFacts::default()),
        SettingsError::Invalid(_)
        | SettingsError::ModuleNotReady(_)
        | SettingsError::OperationKeyReused => invalid(),
        SettingsError::DiscordUnavailable => discord_unavailable(),
        SettingsError::Db(e) => db(e),
    }
}
