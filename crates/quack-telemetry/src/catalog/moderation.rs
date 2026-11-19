//! Catalog entries for the moderation domain and the optional modules.

use super::buckets;
use crate::labels::{
    ActionKind, ActionState, ActivityEvent, ActivityStage, AppealTransition, AttemptOutcome,
    Attribute, CaseSource, CommandName, DmKind, DmOutcome, HoneypotOutcome, InteractionKind,
    InteractionOutcome, MessageKind, ModLogKind, ModLogResult, Outcome, Reason, VoidCause,
};

// Interactions.
spec!(INTERACTION_ACK_DURATION: Histogram "quack.interaction.ack.duration", unit "s",
    [InteractionKind::KEY, CommandName::KEY],
    "Time from receiving an interaction to acknowledging it (Discord allows 3 s).",
    buckets buckets::ACK);
spec!(INTERACTION_DURATION: Histogram "quack.interaction.duration", unit "s",
    [InteractionKind::KEY, CommandName::KEY, InteractionOutcome::KEY],
    "Total time handling an interaction, through the final reply; its count is interactions handled.",
    buckets buckets::WORK);
spec!(INTERACTION_ERRORS: Counter "quack.interaction.errors", unit "{error}",
    [InteractionKind::KEY, CommandName::KEY, Reason::KEY],
    "Interactions that ended refused or failed, by reason.");

// Cases.
spec!(CASES_CREATED: Counter "quack.cases.created", unit "{case}",
    [CaseSource::KEY, ActionKind::KEY],
    "Cases created, by where they came from and the level's action (`none` for a warning).");
spec!(CASES_REFUSED: Counter "quack.cases.refused", unit "{refusal}",
    [CaseSource::KEY, Reason::KEY],
    "Case creations refused (member checks, capabilities, 2FA, Quack's access), by source and reason.");
spec!(CASES_VOIDED: Counter "quack.cases.voided", unit "{case}",
    [VoidCause::KEY],
    "Cases voided, by cause.");
spec!(CASE_CREATE_DURATION: Histogram "quack.case.create.duration", unit "s",
    [CaseSource::KEY],
    "Time to validate, count, choose the level, and commit a new case.",
    buckets buckets::WORK);
spec!(CASE_LEVEL: Histogram "quack.case.level", unit "{level}",
    [ActionKind::KEY],
    "The escalation level number (1-based) chosen for each new case.",
    buckets buckets::LEVEL);

// Actions.
spec!(ACTION_ATTEMPTS: Counter "quack.action.attempts", unit "{attempt}",
    [ActionKind::KEY, AttemptOutcome::KEY, Reason::KEY],
    "Attempts to carry out an action, by attempt outcome and failure reason.");
spec!(ACTIONS_FINISHED: Counter "quack.actions.finished", unit "{action}",
    [ActionKind::KEY, ActionState::KEY],
    "Actions that reached a terminal state (or failed), by kind and state.");
spec!(ACTION_COMPLETION_DURATION: Histogram "quack.action.completion.duration", unit "s",
    [ActionKind::KEY],
    "Time from case creation until its action is done, retries included.",
    buckets buckets::LAG);
spec!(ACTION_RETRIES: Counter "quack.action.retries", unit "{retry}",
    [ActionKind::KEY, Reason::KEY],
    "Action attempts scheduled again after a retryable failure.");
spec!(ACTIONS_FAILED: Gauge "quack.actions.failed", unit "{action}",
    [ActionKind::KEY],
    "Actions currently in the failed state, waiting for staff to retry or dismiss.");
spec!(ACTION_UNDO_SKIPPED: Counter "quack.action.undo.skipped", unit "{action}",
    [ActionKind::KEY],
    "Undo actions left in place because another valid case still applies.");

// DMs.
spec!(DM_OUTCOMES: Counter "quack.dm.outcomes", unit "{dm}",
    [DmKind::KEY, DmOutcome::KEY, Reason::KEY],
    "Final DM outcomes, by kind and failure reason.");
spec!(DM_SEND_DURATION: Histogram "quack.dm.send.duration", unit "s",
    [DmKind::KEY],
    "Time to open the DM channel and send the message.",
    buckets buckets::REQUEST);

// Appeals.
spec!(APPEAL_TRANSITIONS: Counter "quack.appeal.transitions", unit "{transition}",
    [AppealTransition::KEY],
    "Appeal changes, including submissions and staff decisions.");
spec!(APPEAL_DECISION_DURATION: Histogram "quack.appeal.decision.duration", unit "s",
    [AppealTransition::KEY],
    "Time from submission to the staff decision (accept, reject, or close).",
    buckets buckets::HUMAN);
spec!(APPEALS_OPEN: Gauge "quack.appeals.open", unit "{appeal}",
    [],
    "Appeals waiting for staff.");

// Evidence.
spec!(EVIDENCE_CAPTURES: Counter "quack.evidence.captures", unit "{capture}",
    [Outcome::KEY],
    "Evidence capture jobs, by outcome.");
spec!(EVIDENCE_FILES: Counter "quack.evidence.files", unit "{file}",
    [],
    "Files copied to the evidence channel.");
spec!(EVIDENCE_BYTES: Counter "quack.evidence.bytes", unit "By",
    [],
    "Bytes copied to the evidence channel.");

// Moderation log.
spec!(MODLOG_ENTRIES: Counter "quack.modlog.entries", unit "{entry}",
    [ModLogKind::KEY, ModLogResult::KEY],
    "Moderation-log entries written, by entry kind and the result they record.");
spec!(MODLOG_POSTS: Counter "quack.modlog.posts", unit "{post}",
    [Outcome::KEY],
    "Moderation-log entries posted to the log channel, by outcome.");

// Tracked messages.
spec!(MESSAGE_SYNCS: Counter "quack.message.syncs", unit "{sync}",
    [MessageKind::KEY, Outcome::KEY],
    "Runs that bring a tracked Discord message up to date, by outcome.");
spec!(MESSAGE_SYNC_LAG: Histogram "quack.message.sync.lag", unit "s",
    [MessageKind::KEY],
    "Time from a change to the tracked message showing it.",
    buckets buckets::LAG);

// Modules.
spec!(TICKETS_OPENED: Counter "quack.tickets.opened", unit "{ticket}",
    [],
    "Tickets opened.");
spec!(TICKETS_CLOSED: Counter "quack.tickets.closed", unit "{ticket}",
    [],
    "Tickets closed.");
spec!(TICKETS_OPEN: Gauge "quack.tickets.open", unit "{ticket}",
    [],
    "Tickets currently open.");
spec!(TICKET_TRANSCRIPT_SIZE: Histogram "quack.ticket.transcript.size", unit "By",
    [],
    "Size of each saved ticket transcript.",
    buckets buckets::BYTES);
spec!(ACTIVITY_EVENTS: Counter "quack.activity_log.events", unit "{event}",
    [ActivityEvent::KEY, ActivityStage::KEY],
    "Server activity log events seen, posted, dropped, and failed, by event type.");
spec!(HONEYPOT_HITS: Counter "quack.honeypot.hits", unit "{hit}",
    [HoneypotOutcome::KEY],
    "Posts in a honeypot channel, by what Quack did.");
spec!(HONEYPOT_CLEANUPS: Counter "quack.honeypot.cleanups", unit "{cleanup}",
    [Outcome::KEY],
    "Honeypot cleanup runs, by outcome.");
