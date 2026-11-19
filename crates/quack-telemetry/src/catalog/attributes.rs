//! Every attribute key catalog instruments may carry, with allowed values.

use super::AttributeSpec;
use crate::keys;
use crate::labels::{
    ActionKind, ActionState, ActivityEvent, ActivityStage, AppealTransition, AttemptOutcome,
    Attribute, CacheName, CacheResult, CaseSource, CommandName, ConnectionState, CpuMode,
    DbCollection, DbErrorClass, DbOperation, DmKind, DmOutcome, GatewayConnectionEvent,
    GatewayEvent, HoneypotOutcome, HttpMethod, HttpRoute, InteractionKind, InteractionOutcome,
    JobKind, JobOutcome, MessageKind, ModLogKind, ModLogResult, Outcome, PoolName, QueueName,
    RateLimitClass, RateLimitScope, Reason, RestOutcome, SignInOutcome, TransactionOutcome,
    UrlTemplate, VoidCause,
};

/// Values `quack.action.kind` takes: the action kinds, plus `none` on
/// `quack.cases.created` and `quack.case.level` for a warning.
const ACTION_KIND_VALUES: &[&str] = &["timeout", "kick", "ban", "remove_timeout", "unban", "none"];

/// Every attribute key used by [`INSTRUMENTS`](super::INSTRUMENTS).
pub static ATTRIBUTES: &[AttributeSpec] = &[
    of::<InteractionKind>("Kind of Discord interaction."),
    of::<InteractionOutcome>("How handling an interaction ended."),
    of::<CommandName>("Application command name."),
    of::<Outcome>("Terminal outcome of the work."),
    of::<Reason>("Why it failed or was refused; absent on success."),
    attr(ActionKind::KEY, ACTION_KIND_VALUES, "Action kind."),
    of::<AttemptOutcome>("How one action attempt ended."),
    of::<ActionState>("The state an action finished in."),
    of::<CaseSource>("Where the case was created."),
    of::<VoidCause>("Why the case was voided."),
    of::<DmKind>("What the DM is about."),
    of::<DmOutcome>("Terminal state of the DM."),
    of::<AppealTransition>("Appeal change."),
    of::<ModLogKind>("Moderation-log entry kind."),
    of::<ModLogResult>("The result a moderation-log entry records."),
    of::<MessageKind>("Tracked Discord message kind."),
    of::<JobKind>("Background job kind."),
    of::<JobOutcome>("How one job run ended."),
    of::<HttpMethod>("HTTP method (semconv)."),
    of::<HttpRoute>("API route template (semconv)."),
    of::<UrlTemplate>("Discord REST route template (semconv)."),
    attr(
        keys::HTTP_RESPONSE_STATUS_CODE,
        &[],
        "HTTP status code (semconv); absent when no response arrived.",
    ),
    of::<RestOutcome>("Outcome class of a Discord REST call."),
    of::<RateLimitScope>("Which Discord rate limit applied."),
    of::<GatewayEvent>("Gateway event type."),
    of::<GatewayConnectionEvent>("Gateway connection change."),
    of::<SignInOutcome>("How a sign-in ended."),
    of::<RateLimitClass>("API rate-limit class."),
    attr(
        keys::DB_SYSTEM_NAME,
        &["postgresql"],
        "Database system (semconv).",
    ),
    of::<DbOperation>("Database operation (semconv)."),
    of::<DbCollection>("Database table (semconv)."),
    of::<DbErrorClass>("Database error class (semconv `error.type`); absent on success."),
    of::<PoolName>("Database pool name (semconv)."),
    of::<ConnectionState>("Pool connection state (semconv)."),
    of::<TransactionOutcome>("How a transaction ended."),
    of::<CacheName>("Bounded in-memory cache."),
    of::<CacheResult>("Cache lookup result."),
    of::<QueueName>("Bounded in-memory queue."),
    of::<ActivityEvent>("Server activity log event type."),
    of::<ActivityStage>("How far the event got."),
    of::<HoneypotOutcome>("What Quack did about a honeypot post."),
    of::<CpuMode>("CPU mode (semconv)."),
    attr(
        keys::SERVICE_VERSION,
        &[],
        "The running Quack version (semconv).",
    ),
];

/// The spec of a typed attribute: its key and values come from the type.
const fn of<A: Attribute>(description: &'static str) -> AttributeSpec {
    attr(A::KEY, A::VALUES, description)
}

const fn attr(
    key: &'static str,
    values: &'static [&'static str],
    description: &'static str,
) -> AttributeSpec {
    AttributeSpec {
        key,
        description,
        values,
    }
}
