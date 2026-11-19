//! The gateway connection's life (§11.9): when it dropped, how many
//! attempts to bring it back failed, and how long to wait before the next.
//!
//! Twilight reconnects by itself and waits between failed connection
//! attempts (1 s, 2 s, 4 s, … up to about 4 minutes). It doesn't wait when a
//! connection opens and then closes before a session starts, so [`Link`]
//! adds a wait for that case and the shard loop never spins.

use std::time::{Duration, Instant};

/// The longest wait Quack adds between attempts.
const MAX_WAIT: Duration = Duration::from_mins(1);

/// From this many failed attempts in a row, each one is logged as a
/// warning instead of routine information.
pub(crate) const WARN_AFTER: u32 = 5;

/// What a connection change means for the shard loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Change {
    /// The first session started.
    Connected,
    /// A session started again after a drop: `resumed` when nothing was
    /// missed.
    Back {
        /// `RESUMED` (true) or a new `READY` (false).
        resumed: bool,
        /// How long the gateway was down.
        down: Duration,
        /// Attempts that failed meanwhile.
        failures: u32,
    },
    /// A live connection dropped; Twilight is reconnecting.
    Dropped,
    /// An attempt to bring the connection back failed.
    Failed {
        /// Failed attempts in a row.
        failures: u32,
        /// How long the shard loop waits before the next attempt (zero
        /// when Twilight already waits).
        wait: Duration,
    },
    /// Nothing worth reporting (for example the close that follows
    /// Discord's reconnect request).
    Nothing,
}

#[derive(Debug, Clone, Copy)]
enum Phase {
    /// A session is live.
    Up,
    /// No session since `since`.
    Down {
        since: Instant,
        /// Whether a session ever started (else this is startup).
        before: bool,
        failures: u32,
        /// Whether a connection opened (`HELLO`) since the last failure.
        opened: bool,
    },
}

/// The connection state machine; pure, so it is tested without a gateway.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Link {
    phase: Phase,
}

impl Link {
    /// Before the first connection, starting at `now`.
    pub(crate) fn new(now: Instant) -> Self {
        Self {
            phase: Phase::Down {
                since: now,
                before: false,
                failures: 0,
                opened: false,
            },
        }
    }

    /// `READY` or `RESUMED`: a session is live.
    pub(crate) fn session(&mut self, resumed: bool, now: Instant) -> Change {
        let change = match self.phase {
            Phase::Up => Change::Nothing,
            Phase::Down { before: false, .. } => Change::Connected,
            Phase::Down {
                since, failures, ..
            } => Change::Back {
                resumed,
                down: now.saturating_duration_since(since),
                failures,
            },
        };
        self.phase = Phase::Up;
        change
    }

    /// `HELLO`: a new connection opened. While a session was live, the old
    /// connection ended without a close frame: that is the drop.
    pub(crate) fn opened(&mut self, now: Instant) -> Change {
        match &mut self.phase {
            Phase::Up => {
                self.phase = Phase::Down {
                    since: now,
                    before: true,
                    failures: 0,
                    opened: true,
                };
                Change::Dropped
            }
            Phase::Down { opened, .. } => {
                *opened = true;
                Change::Nothing
            }
        }
    }

    /// The connection closed (a close frame, Discord's reconnect request,
    /// an invalid session, or an unreadable compressed frame).
    pub(crate) fn closed(&mut self, now: Instant) -> Change {
        match &mut self.phase {
            Phase::Up => {
                self.phase = Phase::Down {
                    since: now,
                    before: true,
                    failures: 0,
                    opened: false,
                };
                Change::Dropped
            }
            // A connection opened and closed again before a session
            // started: Twilight would retry at once, so Quack waits.
            Phase::Down {
                failures,
                opened: opened @ true,
                ..
            } => {
                *opened = false;
                *failures = failures.saturating_add(1);
                Change::Failed {
                    failures: *failures,
                    wait: wait_after(*failures),
                }
            }
            // The close that follows a drop already reported.
            Phase::Down { opened: false, .. } => Change::Nothing,
        }
    }

    /// Twilight couldn't open a connection; it waits before the next try.
    /// While a session was live (the old connection ended without a close
    /// frame) this reports the drop, with the failure already counted.
    pub(crate) fn connect_failed(&mut self, now: Instant) -> Change {
        match &mut self.phase {
            Phase::Up => {
                self.phase = Phase::Down {
                    since: now,
                    before: true,
                    failures: 1,
                    opened: false,
                };
                Change::Dropped
            }
            Phase::Down {
                failures, opened, ..
            } => {
                *opened = false;
                *failures = failures.saturating_add(1);
                Change::Failed {
                    failures: *failures,
                    wait: Duration::ZERO,
                }
            }
        }
    }
}

/// Quack's wait after `failures` connections in a row closed before a
/// session started: 1 s, 2 s, 4 s, … at most a minute.
fn wait_after(failures: u32) -> Duration {
    let exponent = failures.saturating_sub(1).min(6);
    Duration::from_secs(1 << exponent).min(MAX_WAIT)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(start: Instant, secs: u64) -> Instant {
        start + Duration::from_secs(secs)
    }

    #[test]
    fn the_first_session_is_connected_and_a_repeat_is_nothing() {
        let t = Instant::now();
        let mut link = Link::new(t);
        link.opened(t);
        assert_eq!(link.session(false, t), Change::Connected);
        assert_eq!(link.session(true, t), Change::Nothing);
    }

    #[test]
    fn a_routine_reconnect_drops_once_and_resumes() {
        let t = Instant::now();
        let mut link = Link::new(t);
        link.session(false, t);
        // Discord's reconnect request, then the close that follows it.
        assert_eq!(link.closed(at(t, 10)), Change::Dropped);
        assert_eq!(link.closed(at(t, 10)), Change::Nothing);
        link.opened(t);
        assert_eq!(
            link.session(true, at(t, 12)),
            Change::Back {
                resumed: true,
                down: Duration::from_secs(2),
                failures: 0
            }
        );
    }

    #[test]
    fn failed_attempts_are_counted_until_the_session_is_back() {
        let t = Instant::now();
        let mut link = Link::new(t);
        link.session(false, t);
        assert_eq!(link.closed(at(t, 5)), Change::Dropped);
        assert_eq!(link.connect_failed(at(t, 5)), twilight_failed(1));
        assert_eq!(link.connect_failed(at(t, 6)), twilight_failed(2));
        link.opened(t);
        assert_eq!(
            link.session(false, at(t, 65)),
            Change::Back {
                resumed: false,
                down: Duration::from_mins(1),
                failures: 2
            }
        );
        // A later drop starts counting again.
        link.closed(at(t, 100));
        assert_eq!(link.connect_failed(at(t, 101)), twilight_failed(1));
    }

    #[test]
    fn a_connection_that_closes_before_a_session_backs_off() {
        let t = Instant::now();
        let mut link = Link::new(t);
        link.session(false, t);
        link.closed(t);
        let mut waits = Vec::new();
        for _ in 0..9 {
            link.opened(t);
            match link.closed(t) {
                Change::Failed { wait, .. } => waits.push(wait.as_secs()),
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(waits, [1, 2, 4, 8, 16, 32, 60, 60, 60]);
    }

    #[test]
    fn a_connection_that_ends_without_a_close_is_a_drop() {
        let t = Instant::now();
        let mut link = Link::new(t);
        link.session(false, t);
        // The next HELLO shows the old connection is gone.
        assert_eq!(link.opened(at(t, 4)), Change::Dropped);
        assert_eq!(
            link.session(true, at(t, 5)),
            Change::Back {
                resumed: true,
                down: Duration::from_secs(1),
                failures: 0
            }
        );
        // Or a failed attempt does, with that failure counted.
        assert_eq!(link.connect_failed(at(t, 9)), Change::Dropped);
        assert_eq!(link.connect_failed(at(t, 10)), twilight_failed(2));
    }

    #[test]
    fn startup_failures_end_in_connected() {
        let t = Instant::now();
        let mut link = Link::new(t);
        assert_eq!(link.connect_failed(t), twilight_failed(1));
        link.opened(t);
        assert_eq!(link.session(false, at(t, 3)), Change::Connected);
    }

    /// A failure Twilight already waits after.
    fn twilight_failed(failures: u32) -> Change {
        Change::Failed {
            failures,
            wait: Duration::ZERO,
        }
    }
}
