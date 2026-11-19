//! Job kinds and their per-kind policy (§16.5).

use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use quack_core::UnknownValue;

/// A kind of job (`job_kind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum JobKind {
    /// Start or settle an action attempt.
    RunAction,
    /// Send one DM.
    SendDm,
    /// Bring a tracked Discord message up to date.
    SyncMessage,
    /// Post a moderation log entry to the log channel.
    PostModLog,
    /// Save an evidence snapshot and copy its files.
    CaptureEvidence,
    /// Turn a member's honeypot hits into a case.
    HoneypotCase,
    /// Delete one honeypot post.
    HoneypotCleanup,
    /// Create a ticket's thread and welcome.
    OpenTicket,
    /// Freeze, transcribe, close, and delete a ticket's thread.
    CloseTicket,
    /// Delete expired rows (periodic).
    Housekeeping,
}

impl JobKind {
    /// Every kind, in declaration order.
    pub const ALL: &'static [Self] = &[
        Self::RunAction,
        Self::SendDm,
        Self::SyncMessage,
        Self::PostModLog,
        Self::CaptureEvidence,
        Self::HoneypotCase,
        Self::HoneypotCleanup,
        Self::OpenTicket,
        Self::CloseTicket,
        Self::Housekeeping,
    ];

    /// The stable name, equal to the Postgres label and the key prefix.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RunAction => "run_action",
            Self::SendDm => "send_dm",
            Self::SyncMessage => "sync_message",
            Self::PostModLog => "post_mod_log",
            Self::CaptureEvidence => "capture_evidence",
            Self::HoneypotCase => "honeypot_case",
            Self::HoneypotCleanup => "honeypot_cleanup",
            Self::OpenTicket => "open_ticket",
            Self::CloseTicket => "close_ticket",
            Self::Housekeeping => "housekeeping",
        }
    }

    /// The kind's limits and retry schedule (§16.5).
    pub const fn policy(self) -> JobPolicy {
        const S: u64 = 1;
        const M: u64 = 60;
        const H: u64 = 3600;
        let (concurrency, lease, max_attempts, backoff): (usize, u64, u32, &'static [u64]) =
            match self {
                Self::RunAction => (8, 60, 8, &[5, 15, M, 5 * M, 15 * M, H, 3 * H]),
                Self::SendDm => (4, 120, 3, &[5, 30]),
                Self::SyncMessage => (4, 60, 6, &[2, 10, M, 5 * M, 30 * M]),
                Self::PostModLog => (2, 60, 6, &[10, 30, 2 * M, 10 * M, H]),
                Self::CaptureEvidence => (4, 120, 4, &[10, M, 5 * M]),
                Self::HoneypotCase => (2, 60, 5, &[5, 30, 2 * M, 10 * M]),
                Self::HoneypotCleanup => (2, 30, 10, &[S, 2, 4, 8, 16, 32, 64, 64, 64]),
                Self::OpenTicket => (2, 60, 4, &[2, 10, 30]),
                Self::CloseTicket => (2, 300, 6, &[5, 30, 2 * M, 10 * M, H]),
                Self::Housekeeping => (1, 300, 1, &[10 * M]),
            };
        JobPolicy {
            concurrency,
            lease: Duration::from_secs(lease),
            max_attempts,
            backoff_secs: backoff,
        }
    }
}

impl fmt::Display for JobKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for JobKind {
    type Err = UnknownValue;

    fn from_str(s: &str) -> Result<Self, UnknownValue> {
        Self::ALL
            .iter()
            .copied()
            .find(|kind| kind.as_str() == s)
            .ok_or_else(|| UnknownValue {
                type_name: "JobKind",
                value: s.to_owned(),
            })
    }
}

/// How one kind of job runs (§16.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JobPolicy {
    /// Jobs of this kind running at once in this process.
    pub concurrency: usize,
    /// How long a claim lasts.
    pub lease: Duration,
    /// Runs before the kind's finalize takes over.
    pub max_attempts: u32,
    backoff_secs: &'static [u64],
}

/// The margin between a handler's hard deadline and its lease.
pub const DEADLINE_MARGIN: Duration = Duration::from_secs(15);

impl JobPolicy {
    /// The hard deadline of one run: the lease minus [`DEADLINE_MARGIN`].
    pub fn deadline(&self) -> Duration {
        self.lease.saturating_sub(DEADLINE_MARGIN)
    }

    /// The wait before the next run after run `attempt` (1-based) failed;
    /// the last entry repeats.
    pub fn backoff(&self, attempt: u32) -> Duration {
        let index = usize::try_from(attempt.max(1) - 1).unwrap_or(usize::MAX);
        let secs = self
            .backoff_secs
            .get(index)
            .or_else(|| self.backoff_secs.last())
            .copied()
            .unwrap_or(1);
        Duration::from_secs(secs)
    }
}
