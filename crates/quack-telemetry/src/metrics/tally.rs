//! An in-process tally of job runs over the last 24 hours, for the ops
//! metrics (§21.8). OpenTelemetry instruments can't be read back, so this
//! keeps its own small counts: one bucket per 10 minutes, one row per
//! kind, and reads a whole day plus the bucket in progress. It
//! starts empty with each process.

use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::labels::{JobKind, JobOutcome};

/// One bucket's length.
const BUCKET: u64 = 600;
/// A day of buckets.
const DAY: u64 = 86_400 / BUCKET;
/// Slots: a whole day before the bucket in progress, and that bucket.
const SLOTS: usize = 145;

/// Runs of one job kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KindRuns {
    /// The kind.
    pub kind: JobKind,
    /// Runs that finished, however they ended.
    pub runs: u64,
    /// Runs that failed and left the job to retry (a retry, a lost lease,
    /// or a run past its deadline).
    pub retries: u64,
    /// Runs that left the job dead.
    pub dead: u64,
    /// Time spent running, summed.
    pub busy: Duration,
    /// The longest run.
    pub longest: Duration,
}

impl KindRuns {
    const fn empty(kind: JobKind) -> Self {
        Self {
            kind,
            runs: 0,
            retries: 0,
            dead: 0,
            busy: Duration::ZERO,
            longest: Duration::ZERO,
        }
    }

    fn add(&mut self, other: &Self) {
        self.runs += other.runs;
        self.retries += other.retries;
        self.dead += other.dead;
        self.busy += other.busy;
        self.longest = self.longest.max(other.longest);
    }

    /// The mean run.
    pub fn mean(&self) -> Duration {
        u32::try_from(self.runs)
            .ok()
            .filter(|n| *n > 0)
            .map_or(Duration::ZERO, |n| self.busy / n)
    }
}

/// What the tally read: the kinds that ran, and how long it covers: since
/// start while the process is younger than a day, else the last 24 hours
/// and the part of the 10 minutes before them still in their bucket.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TallyRead {
    /// How far back the counts go.
    pub covers: Duration,
    /// Kinds with at least one run, in [`JobKind::ALL`] order.
    pub kinds: Vec<KindRuns>,
}

#[derive(Clone)]
struct Bucket {
    index: u64,
    kinds: Vec<KindRuns>,
}

impl Bucket {
    fn new(index: u64) -> Self {
        Self {
            index,
            kinds: JobKind::ALL.iter().copied().map(KindRuns::empty).collect(),
        }
    }
}

/// Job runs per 10 minutes over the last day of this process.
pub struct JobTally {
    started: Instant,
    buckets: Mutex<Vec<Bucket>>,
}

impl Default for JobTally {
    fn default() -> Self {
        Self::new()
    }
}

impl JobTally {
    /// An empty tally starting now.
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            buckets: Mutex::new(Vec::new()),
        }
    }

    /// Counts one finished run.
    pub fn record(&self, kind: JobKind, outcome: JobOutcome, duration: Duration) {
        self.record_at(self.started.elapsed(), kind, outcome, duration);
    }

    fn record_at(&self, elapsed: Duration, kind: JobKind, outcome: JobOutcome, took: Duration) {
        let index = elapsed.as_secs() / BUCKET;
        let Some(row) = JobKind::ALL.iter().position(|k| *k == kind) else {
            return;
        };
        let mut buckets = self.buckets.lock().unwrap_or_else(PoisonError::into_inner);
        let slot = usize::try_from(index % SLOTS as u64).unwrap_or(0);
        if buckets.len() <= slot {
            buckets.resize_with(slot + 1, || Bucket::new(u64::MAX));
        }
        if buckets[slot].index != index {
            buckets[slot] = Bucket::new(index);
        }
        let row = &mut buckets[slot].kinds[row];
        row.runs += 1;
        match outcome {
            JobOutcome::Retry | JobOutcome::LostLease | JobOutcome::TimedOut => row.retries += 1,
            JobOutcome::Dead => row.dead += 1,
            JobOutcome::Done
            | JobOutcome::Deferred
            | JobOutcome::Continued
            | JobOutcome::Rescheduled
            | JobOutcome::Finalized => {}
        }
        row.busy += took;
        row.longest = row.longest.max(took);
    }

    /// The runs of the last 24 hours (or since start, when sooner).
    pub fn read(&self) -> TallyRead {
        self.read_at(self.started.elapsed())
    }

    fn read_at(&self, elapsed: Duration) -> TallyRead {
        let index = elapsed.as_secs() / BUCKET;
        let oldest = index.saturating_sub(DAY);
        let mut kinds: Vec<KindRuns> = JobKind::ALL.iter().copied().map(KindRuns::empty).collect();
        let buckets = self.buckets.lock().unwrap_or_else(PoisonError::into_inner);
        for bucket in buckets
            .iter()
            .filter(|b| b.index >= oldest && b.index <= index)
        {
            for (total, row) in kinds.iter_mut().zip(&bucket.kinds) {
                total.add(row);
            }
        }
        let covers = elapsed.saturating_sub(Duration::from_secs(oldest * BUCKET));
        TallyRead {
            covers,
            kinds: kinds.into_iter().filter(|k| k.runs > 0).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn at(hours: u64, secs: u64) -> Duration {
        Duration::from_secs(hours * 3_600 + secs)
    }

    #[test]
    fn counts_runs_by_kind_and_outcome() {
        let tally = JobTally::new();
        let ms = Duration::from_millis;
        tally.record_at(at(0, 5), JobKind::SendDm, JobOutcome::Done, ms(100));
        tally.record_at(at(0, 6), JobKind::SendDm, JobOutcome::Retry, ms(300));
        tally.record_at(at(1, 0), JobKind::SendDm, JobOutcome::Dead, ms(200));
        tally.record_at(at(1, 1), JobKind::RunAction, JobOutcome::TimedOut, ms(50));
        let read = tally.read_at(at(1, 30));
        assert_eq!(read.covers, at(1, 30), "since start");
        assert_eq!(read.kinds.len(), 2);
        let dm = read
            .kinds
            .iter()
            .find(|k| k.kind == JobKind::SendDm)
            .unwrap();
        assert_eq!((dm.runs, dm.retries, dm.dead), (3, 1, 1));
        assert_eq!(dm.mean(), ms(200));
        assert_eq!(dm.longest, ms(300));
        // Kinds come in catalog order.
        assert_eq!(read.kinds[0].kind, JobKind::RunAction);
    }

    #[test]
    fn keeps_a_whole_day() {
        let tally = JobTally::new();
        // Near the end of the first bucket.
        tally.record_at(
            at(0, 590),
            JobKind::SendDm,
            JobOutcome::Done,
            Duration::ZERO,
        );
        tally.record_at(at(10, 0), JobKind::SendDm, JobOutcome::Done, Duration::ZERO);
        // A day after it, the run is still counted and the window is at
        // least 24 hours.
        let read = tally.read_at(at(24, 590));
        assert_eq!(read.kinds[0].runs, 2);
        assert!(read.covers >= at(24, 0), "{:?}", read.covers);
        // Once its bucket is more than a day old, it's gone.
        let read = tally.read_at(at(24, 600));
        assert_eq!(read.kinds[0].runs, 1);
        assert!(read.covers >= at(24, 0) && read.covers <= at(24, 600));
        // Its slot starts over when reused.
        tally.record_at(
            at(24, 1_300),
            JobKind::SendDm,
            JobOutcome::Done,
            Duration::ZERO,
        );
        assert_eq!(tally.read_at(at(24, 1_300)).kinds[0].runs, 2);
    }
}
