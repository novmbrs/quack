//! The job queue (§16): every piece of background work is a keyed row in
//! `jobs`, inserted in the same transaction as the change that needs it.
//!
//! - [`enqueue`] upserts by key (coalescing) and notifies `quack_jobs`.
//! - [`claim`] takes due rows with `FOR UPDATE SKIP LOCKED`, a fresh claim
//!   token, and a lease from PostgreSQL's clock.
//! - [`fence`] and [`settle`] end every handler transaction, so a handler
//!   that lost its lease rolls back everything it wrote.
//! - [`Runner`] dispatches claimed jobs to a handler callback with per-kind
//!   and global limits and a hard deadline.

mod kind;
mod queue;
mod runner;

use std::fmt;
use std::time::{Duration, Instant};

use quack_core::ids::{
    ActionId, DmId, EvidenceId, GuildId, JobId, MemberId, MessageId, MessageRowId, ModLogId,
    TicketId,
};
use serde::de::DeserializeOwned;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::DbError;

pub use kind::{DEADLINE_MARGIN, JobKind, JobPolicy};
pub use queue::{
    JobBacklog, backlog, claim, enqueue, enqueue_many, ensure_housekeeping, fence, next_due_in,
    notify, settle, sweep_and_next_due, sweep_leases, wake,
};
pub use runner::{FINALIZE_TRIES, Handled, Handler, HandlerFuture, Phase, Runner, RunnerConfig};

/// A job's key: `<kind>:<owner ids>`. One key always means the same
/// payload, so coalescing two enqueues never loses work.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct JobKey(String);

impl JobKey {
    fn of(kind: JobKind, owner: &[&dyn fmt::Display]) -> Self {
        let mut key = kind.as_str().to_owned();
        for part in owner {
            key.push(':');
            key.push_str(&part.to_string());
        }
        Self(key)
    }

    /// The key text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for JobKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// When a job should run next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunAt {
    /// Now.
    Now,
    /// After this long (from PostgreSQL's clock).
    In(Duration),
    /// At this time (normally a time read from the database).
    At(OffsetDateTime),
}

impl RunAt {
    /// The `(delay seconds, absolute time)` pair the SQL takes; exactly one
    /// is used, `COALESCE($at, now() + $delay)`.
    pub(crate) fn sql(self) -> (f64, Option<OffsetDateTime>) {
        match self {
            Self::Now => (0.0, None),
            Self::In(delay) => (delay.as_secs_f64(), None),
            Self::At(at) => (0.0, Some(at)),
        }
    }
}

/// A job to enqueue. Built only through the per-kind constructors, which
/// fix the key and the payload together.
#[derive(Clone, Debug, PartialEq)]
pub struct NewJob {
    kind: JobKind,
    key: JobKey,
    guild: Option<GuildId>,
    payload: serde_json::Value,
    run_at: RunAt,
}

impl NewJob {
    fn new(
        kind: JobKind,
        guild: Option<GuildId>,
        owner: &[&dyn fmt::Display],
        payload: serde_json::Value,
    ) -> Self {
        Self {
            kind,
            key: JobKey::of(kind, owner),
            guild,
            payload,
            run_at: RunAt::Now,
        }
    }

    /// Runs this job no earlier than `run_at` (default: now).
    #[must_use]
    pub fn at(mut self, run_at: RunAt) -> Self {
        self.run_at = run_at;
        self
    }

    /// `run_action:<action>`.
    pub fn run_action(guild: GuildId, action: ActionId) -> Self {
        Self::new(
            JobKind::RunAction,
            Some(guild),
            &[&action],
            serde_json::json!({ "action_id": action }),
        )
    }

    /// `send_dm:<dm>`.
    pub fn send_dm(guild: GuildId, dm: DmId) -> Self {
        Self::new(
            JobKind::SendDm,
            Some(guild),
            &[&dm],
            serde_json::json!({ "dm_id": dm }),
        )
    }

    /// `sync_message:<row>`.
    pub fn sync_message(guild: GuildId, row: MessageRowId) -> Self {
        Self::new(
            JobKind::SyncMessage,
            Some(guild),
            &[&row],
            serde_json::json!({ "message_row_id": row }),
        )
    }

    /// `post_mod_log:<entry>`, pinned to the log channel at enqueue time.
    pub fn post_mod_log(
        guild: GuildId,
        entry: ModLogId,
        channel: quack_core::ids::ChannelId,
    ) -> Self {
        Self::new(
            JobKind::PostModLog,
            Some(guild),
            &[&entry],
            serde_json::json!({ "entry_id": entry, "channel_id": channel }),
        )
    }

    /// `capture_evidence:<evidence>`.
    pub fn capture_evidence(guild: GuildId, evidence: EvidenceId) -> Self {
        Self::new(
            JobKind::CaptureEvidence,
            Some(guild),
            &[&evidence],
            serde_json::json!({ "evidence_id": evidence }),
        )
    }

    /// `honeypot_case:<guild>:<member>`.
    pub fn honeypot_case(guild: GuildId, member: MemberId) -> Self {
        Self::new(
            JobKind::HoneypotCase,
            Some(guild),
            &[&guild, &member],
            serde_json::json!({ "member_id": member }),
        )
    }

    /// `honeypot_cleanup:<guild>:<message>`.
    pub fn honeypot_cleanup(
        guild: GuildId,
        channel: quack_core::ids::ChannelId,
        message: MessageId,
    ) -> Self {
        Self::new(
            JobKind::HoneypotCleanup,
            Some(guild),
            &[&guild, &message],
            serde_json::json!({ "channel_id": channel, "message_id": message }),
        )
    }

    /// `open_ticket:<ticket>`.
    pub fn open_ticket(guild: GuildId, ticket: TicketId) -> Self {
        Self::new(
            JobKind::OpenTicket,
            Some(guild),
            &[&ticket],
            serde_json::json!({ "ticket_id": ticket }),
        )
    }

    /// `close_ticket:<ticket>`.
    pub fn close_ticket(guild: GuildId, ticket: TicketId) -> Self {
        Self::new(
            JobKind::CloseTicket,
            Some(guild),
            &[&ticket],
            serde_json::json!({ "ticket_id": ticket }),
        )
    }

    /// `post_ops:<post>`, run in the ops server (§21.3).
    pub fn post_ops(guild: GuildId, post: Uuid) -> Self {
        Self::new(
            JobKind::PostOps,
            Some(guild),
            &[&post],
            serde_json::json!({ "post_id": post }),
        )
    }

    /// `housekeeping` (no server).
    pub fn housekeeping() -> Self {
        Self::new(JobKind::Housekeeping, None, &[], serde_json::json!({}))
    }

    /// The job's kind.
    pub fn kind(&self) -> JobKind {
        self.kind
    }

    /// The job's key.
    pub fn key(&self) -> &JobKey {
        &self.key
    }
}

/// A claimed job: the handler's input and the fence for its writes.
#[derive(Clone, Debug, PartialEq)]
pub struct ClaimedJob {
    /// The row id.
    pub id: JobId,
    /// The kind.
    pub kind: JobKind,
    /// The key (`<kind>:<owner ids>`).
    pub key: String,
    /// The server; `None` only for housekeeping.
    pub guild: Option<GuildId>,
    /// Immutable ids naming what the job works on.
    pub payload: serde_json::Value,
    /// This run's number, counting the claim (1-based). Above
    /// `max_attempts` the runner finalizes instead of running.
    pub attempt: u32,
    /// The kind's budget when the job was enqueued.
    pub max_attempts: u32,
    /// The claim's fence token.
    pub claim_token: Uuid,
    /// When the lease ends (PostgreSQL time).
    pub locked_until: OffsetDateTime,
    /// The enqueuer's W3C `traceparent`, for the span link.
    pub trace_context: Option<String>,
    /// When the job became due.
    pub run_at: OffsetDateTime,
    /// When the job was first enqueued.
    pub created_at: OffsetDateTime,
    /// The run's hard deadline on this process's monotonic clock: the
    /// lease left at claim, counted from when the claim was sent, minus
    /// [`DEADLINE_MARGIN`]. The runner drops the run when it passes.
    pub deadline: Instant,
}

impl ClaimedJob {
    /// Parses the payload.
    pub fn payload<T: DeserializeOwned>(&self) -> Result<T, DbError> {
        serde_json::from_value(self.payload.clone())
            .map_err(|error| DbError::Decode(format!("job payload: {error}")))
    }

    /// Whether the budget is spent, so the kind's finalize must run instead
    /// of the handler.
    pub fn budget_spent(&self) -> bool {
        self.attempt > self.max_attempts
    }

    /// Whether this is the last run the budget allows.
    pub fn is_last_attempt(&self) -> bool {
        self.attempt >= self.max_attempts
    }

    /// How long the run has until its [`ClaimedJob::deadline`] (zero once
    /// it passed). Check it before each Discord call (§16.1).
    pub fn time_left(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }

    /// How long the job waited between becoming due and this claim.
    pub fn queue_wait(&self) -> Duration {
        let claimed_at = self.locked_until - self.kind.policy().lease;
        (claimed_at - self.run_at)
            .try_into()
            .unwrap_or(Duration::ZERO)
    }
}

/// How a run ended, applied by [`settle`] (§16.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobResult {
    /// Finished: delete the row, or run once more at `rerun_at` when the job
    /// was enqueued again while it ran.
    Done,
    /// Failed: run again at `at`, recording the error. The run counted.
    Retry {
        /// When to run again.
        at: RunAt,
        /// A short machine code (at most 500 characters kept).
        error: String,
    },
    /// Not now (a busy member, a DM still sending): run again at `at`; the
    /// run does not count against the budget.
    Defer {
        /// When to run again.
        at: RunAt,
    },
    /// Progress was saved and the work continues in a new run now; the run
    /// does not count.
    Continue,
    /// The job's records are missing or malformed: keep it as dead.
    Dead {
        /// A short machine code.
        error: String,
    },
    /// Periodic jobs: run again at `at` with a fresh budget.
    Reschedule {
        /// When to run again.
        at: RunAt,
    },
}

/// Cuts an error code to the 500 characters `jobs.last_error` keeps.
pub(crate) fn short_error(error: &str) -> String {
    error.chars().take(500).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for kind in JobKind::ALL {
            assert_eq!(kind.as_str().parse::<JobKind>().unwrap(), *kind);
        }
    }

    #[test]
    fn backoff_follows_the_table_and_repeats_its_last_entry() {
        let policy = JobKind::RunAction.policy();
        assert_eq!(policy.backoff(1), Duration::from_secs(5));
        assert_eq!(policy.backoff(7), Duration::from_hours(3));
        assert_eq!(policy.backoff(20), Duration::from_hours(3));
        assert_eq!(
            JobKind::HoneypotCleanup.policy().deadline(),
            Duration::from_secs(15)
        );
    }

    #[test]
    fn keys_name_the_kind_and_owner() {
        let guild = GuildId::new(7).unwrap();
        let member = MemberId::new(9).unwrap();
        assert_eq!(
            NewJob::honeypot_case(guild, member).key().as_str(),
            "honeypot_case:7:9"
        );
        assert_eq!(NewJob::housekeeping().key().as_str(), "housekeeping");
    }
}
