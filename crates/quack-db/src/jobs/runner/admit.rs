//! Global admission: one FIFO arbiter hands out the global permits.
//!
//! A freed permit goes straight to the first waiting kind (a reserved
//! turn), so a kind that just finished a job can't take it back while
//! another kind waits. Permits a pass reserved but didn't use go back
//! the same way, and never wake the kind that returned them: an idle
//! dispatcher must not wake itself.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::sync::{Notify, OwnedSemaphorePermit};

use crate::jobs::JobKind;

/// The global permit pool with its waiting list.
pub(super) struct Arbiter {
    state: Mutex<State>,
}

struct State {
    /// Permits nobody holds or has been granted.
    free: usize,
    /// Kinds that wanted a permit and found none, oldest first.
    waiting: VecDeque<JobKind>,
    /// Permits handed to a waiting kind that it has not picked up yet.
    granted: HashMap<JobKind, usize>,
}

impl Arbiter {
    pub(super) fn new(permits: usize) -> Self {
        Self {
            state: Mutex::new(State {
                free: permits,
                waiting: VecDeque::new(),
                granted: HashMap::new(),
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // The state is a few counters and stays consistent between
        // statements, so a poisoned lock is still safe to use.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Takes up to `want` permits for `kind`: its granted turns first, then
    /// free permits only when no other kind is waiting. A kind that gets
    /// fewer than it wanted joins the back of the waiting list.
    pub(super) fn take(&self, kind: JobKind, want: usize) -> usize {
        let mut state = self.lock();
        let mut got = 0;
        if let Some(granted) = state.granted.get_mut(&kind) {
            got = (*granted).min(want);
            *granted -= got;
            if *granted == 0 {
                state.granted.remove(&kind);
            }
        }
        let others_waiting = state.waiting.iter().any(|k| *k != kind);
        if got < want && !others_waiting {
            let more = state.free.min(want - got);
            state.free -= more;
            got += more;
        }
        if got < want && !state.waiting.contains(&kind) {
            state.waiting.push_back(kind);
        } else if got == want {
            state.waiting.retain(|k| *k != kind);
        }
        got
    }

    /// Leaves the waiting list and returns any turns granted to `kind`
    /// (it has nothing due now), handing them on like [`Arbiter::give_back`].
    pub(super) fn forfeit(&self, kind: JobKind) -> Vec<JobKind> {
        let granted = {
            let mut state = self.lock();
            state.waiting.retain(|k| *k != kind);
            state.granted.remove(&kind).unwrap_or(0)
        };
        self.give_back(granted)
    }

    /// Gives `count` permits back: each goes to the first waiting kind
    /// (returned so the caller can wake it), or to the free pool.
    pub(super) fn give_back(&self, count: usize) -> Vec<JobKind> {
        let mut state = self.lock();
        let mut woken = Vec::new();
        for _ in 0..count {
            match state.waiting.pop_front() {
                Some(next) => {
                    *state.granted.entry(next).or_default() += 1;
                    woken.push(next);
                }
                None => state.free += 1,
            }
        }
        woken
    }
}

/// What a running job holds: its kind permit and one global permit.
/// Dropping it frees both, wakes the job's own kind (a permit freed up),
/// and wakes whichever kind the global permit was handed to.
pub(super) struct Permit {
    pub(super) kind: JobKind,
    pub(super) own: Option<OwnedSemaphorePermit>,
    pub(super) arbiter: Arc<Arbiter>,
    pub(super) wakes: Arc<HashMap<JobKind, Arc<Notify>>>,
}

impl Drop for Permit {
    fn drop(&mut self) {
        drop(self.own.take());
        let woken = self.arbiter.give_back(1);
        for kind in std::iter::once(self.kind).chain(woken) {
            if let Some(wake) = self.wakes.get(&kind) {
                wake.notify_one();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_freed_permit_goes_to_the_waiting_kind_first() {
        let arbiter = Arbiter::new(1);
        assert_eq!(arbiter.take(JobKind::Housekeeping, 1), 1);
        assert_eq!(arbiter.take(JobKind::SendDm, 1), 0, "SendDm waits");
        assert_eq!(
            arbiter.give_back(1),
            vec![JobKind::SendDm],
            "the freed permit is handed to SendDm"
        );
        assert_eq!(
            arbiter.take(JobKind::Housekeeping, 1),
            0,
            "Housekeeping can't jump the queue"
        );
        assert_eq!(arbiter.take(JobKind::SendDm, 1), 1);
    }

    #[test]
    fn a_kind_with_nothing_due_hands_its_turn_on() {
        let arbiter = Arbiter::new(1);
        assert_eq!(arbiter.take(JobKind::RunAction, 1), 1);
        assert_eq!(arbiter.take(JobKind::SendDm, 1), 0);
        assert_eq!(arbiter.take(JobKind::SyncMessage, 1), 0);
        assert_eq!(arbiter.give_back(1), vec![JobKind::SendDm]);
        assert_eq!(arbiter.forfeit(JobKind::SendDm), vec![JobKind::SyncMessage]);
        assert_eq!(arbiter.take(JobKind::SyncMessage, 1), 1);
    }

    #[test]
    fn unused_permits_return_without_waking_anyone_when_nobody_waits() {
        let arbiter = Arbiter::new(4);
        assert_eq!(arbiter.take(JobKind::SendDm, 3), 3);
        assert_eq!(arbiter.give_back(3), Vec::<JobKind>::new());
        assert_eq!(arbiter.take(JobKind::RunAction, 8), 4);
    }

    #[test]
    fn a_kind_stays_in_line_until_it_gets_what_it_wanted() {
        let arbiter = Arbiter::new(1);
        assert_eq!(arbiter.take(JobKind::RunAction, 1), 1);
        assert_eq!(arbiter.take(JobKind::SendDm, 2), 0);
        assert_eq!(arbiter.take(JobKind::SyncMessage, 1), 0);
        assert_eq!(arbiter.give_back(1), vec![JobKind::SendDm]);
        assert_eq!(arbiter.take(JobKind::SendDm, 2), 1);
        // SendDm is back in line, behind SyncMessage.
        assert_eq!(arbiter.give_back(1), vec![JobKind::SyncMessage]);
    }
}
