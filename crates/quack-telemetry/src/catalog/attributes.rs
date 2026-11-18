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
    attr(
        InteractionKind::KEY,
        InteractionKind::VALUES,
        "Kind of Discord interaction.",
    ),
    attr(
        InteractionOutcome::KEY,
        InteractionOutcome::VALUES,
        "How handling an interaction ended.",
    ),
    attr(CommandName::KEY, &[], "Application command name."),
    attr(
        Outcome::KEY,
        Outcome::VALUES,
        "Terminal outcome of the work.",
    ),
    attr(
        Reason::KEY,
        Reason::VALUES,
        "Why it failed or was refused; absent on success.",
    ),
    attr(ActionKind::KEY, ACTION_KIND_VALUES, "Action kind."),
    attr(
        AttemptOutcome::KEY,
        AttemptOutcome::VALUES,
        "How one action attempt ended.",
    ),
    attr(
        ActionState::KEY,
        ActionState::VALUES,
        "The state an action finished in.",
    ),
    attr(
        CaseSource::KEY,
        CaseSource::VALUES,
        "Where the case was created.",
    ),
    attr(
        VoidCause::KEY,
        VoidCause::VALUES,
        "Why the case was voided.",
    ),
    attr(DmKind::KEY, DmKind::VALUES, "What the DM is about."),
    attr(
        DmOutcome::KEY,
        DmOutcome::VALUES,
        "Terminal state of the DM.",
    ),
    attr(
        AppealTransition::KEY,
        AppealTransition::VALUES,
        "Appeal change.",
    ),
    attr(ModLogKind::KEY, &[], "Moderation-log entry kind."),
    attr(
        ModLogResult::KEY,
        ModLogResult::VALUES,
        "The result a moderation-log entry records.",
    ),
    attr(
        MessageKind::KEY,
        MessageKind::VALUES,
        "Tracked Discord message kind.",
    ),
    attr(JobKind::KEY, JobKind::VALUES, "Background job kind."),
    attr(
        JobOutcome::KEY,
        JobOutcome::VALUES,
        "How one job run ended.",
    ),
    attr(
        HttpMethod::KEY,
        HttpMethod::VALUES,
        "HTTP method (semconv).",
    ),
    attr(HttpRoute::KEY, &[], "API route template (semconv)."),
    attr(
        UrlTemplate::KEY,
        &[],
        "Discord REST route template (semconv).",
    ),
    attr(
        keys::HTTP_RESPONSE_STATUS_CODE,
        &[],
        "HTTP status code (semconv); absent when no response arrived.",
    ),
    attr(
        RestOutcome::KEY,
        RestOutcome::VALUES,
        "Outcome class of a Discord REST call.",
    ),
    attr(
        RateLimitScope::KEY,
        RateLimitScope::VALUES,
        "Which Discord rate limit applied.",
    ),
    attr(GatewayEvent::KEY, &[], "Gateway event type."),
    attr(
        GatewayConnectionEvent::KEY,
        GatewayConnectionEvent::VALUES,
        "Gateway connection change.",
    ),
    attr(
        SignInOutcome::KEY,
        SignInOutcome::VALUES,
        "How a sign-in ended.",
    ),
    attr(RateLimitClass::KEY, &[], "API rate-limit class."),
    attr(
        keys::DB_SYSTEM_NAME,
        &["postgresql"],
        "Database system (semconv).",
    ),
    attr(
        DbOperation::KEY,
        DbOperation::VALUES,
        "Database operation (semconv).",
    ),
    attr(DbCollection::KEY, &[], "Database table (semconv)."),
    attr(
        DbErrorClass::KEY,
        DbErrorClass::VALUES,
        "Database error class (semconv `error.type`); absent on success.",
    ),
    attr(PoolName::KEY, &[], "Database pool name (semconv)."),
    attr(
        ConnectionState::KEY,
        ConnectionState::VALUES,
        "Pool connection state (semconv).",
    ),
    attr(
        TransactionOutcome::KEY,
        TransactionOutcome::VALUES,
        "How a transaction ended.",
    ),
    attr(CacheName::KEY, &[], "Bounded in-memory cache."),
    attr(
        CacheResult::KEY,
        CacheResult::VALUES,
        "Cache lookup result.",
    ),
    attr(QueueName::KEY, &[], "Bounded in-memory queue."),
    attr(ActivityEvent::KEY, &[], "Server activity log event type."),
    attr(
        ActivityStage::KEY,
        ActivityStage::VALUES,
        "How far the event got.",
    ),
    attr(
        HoneypotOutcome::KEY,
        HoneypotOutcome::VALUES,
        "What Quack did about a honeypot post.",
    ),
    attr(CpuMode::KEY, CpuMode::VALUES, "CPU mode (semconv)."),
    attr(
        keys::SERVICE_VERSION,
        &[],
        "The running Quack version (semconv).",
    ),
];

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
