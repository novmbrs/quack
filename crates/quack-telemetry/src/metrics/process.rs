//! Reads process statistics from `/proc/self` on Linux.
//!
//! Each reader returns `None` when the file is missing or unreadable (other
//! operating systems, restricted containers), so the instrument simply
//! reports nothing.

use std::fs;

/// Linux reports `utime`/`stime` in `USER_HZ` ticks, which is 100 on every
/// architecture Quack ships for (x86-64 and aarch64). Reading it from
/// `sysconf` would need `unsafe`, which the workspace forbids.
const USER_HZ: f64 = 100.0;

/// CPU time in seconds, split into user and system.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CpuTime {
    pub(crate) user: f64,
    pub(crate) system: f64,
}

/// Memory and thread figures from `/proc/self/status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Status {
    pub(crate) resident_bytes: Option<u64>,
    pub(crate) virtual_bytes: Option<u64>,
    pub(crate) threads: Option<u64>,
}

/// Reads the process CPU time from `/proc/self/stat`.
pub(crate) fn cpu_time() -> Option<CpuTime> {
    parse_stat(&fs::read_to_string("/proc/self/stat").ok()?)
}

/// Reads memory and threads from `/proc/self/status`.
pub(crate) fn status() -> Option<Status> {
    Some(parse_status(&fs::read_to_string("/proc/self/status").ok()?))
}

/// Counts open file descriptors in `/proc/self/fd`.
pub(crate) fn open_fds() -> Option<u64> {
    let count = fs::read_dir("/proc/self/fd").ok()?.count();
    // The directory handle used to count is itself an open descriptor.
    u64::try_from(count.saturating_sub(1)).ok()
}

/// Parses `utime` and `stime` (fields 14 and 15) of `/proc/<pid>/stat`.
/// Fields are counted after the parenthesized command name, which may
/// contain spaces.
#[allow(
    clippy::cast_precision_loss,
    reason = "tick counts stay far below 2^52 for any realistic uptime"
)]
fn parse_stat(stat: &str) -> Option<CpuTime> {
    let rest = &stat[stat.rfind(')')? + 1..];
    let mut fields = rest.split_whitespace();
    // After ')' comes field 3 (state); utime is field 14, so skip 11 fields.
    let user: u64 = fields.nth(11)?.parse().ok()?;
    let system: u64 = fields.next()?.parse().ok()?;
    Some(CpuTime {
        user: user as f64 / USER_HZ,
        system: system as f64 / USER_HZ,
    })
}

/// Parses `VmRSS`, `VmSize` (in kB), and `Threads` from `/proc/<pid>/status`.
fn parse_status(status: &str) -> Status {
    let mut parsed = Status::default();
    for line in status.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let number = value
            .split_whitespace()
            .next()
            .and_then(|n| n.parse::<u64>().ok());
        match key {
            "VmRSS" => parsed.resident_bytes = number.map(|kb| kb.saturating_mul(1024)),
            "VmSize" => parsed.virtual_bytes = number.map(|kb| kb.saturating_mul(1024)),
            "Threads" => parsed.threads = number,
            _ => {}
        }
    }
    parsed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stat_with_spaces_in_the_name() {
        let stat = "1234 (quack (worker) 1) S 1 1234 1234 0 -1 4194560 100 0 0 0 250 75 0 0 20 0 9 0 100 1000 200";
        let cpu = parse_stat(stat).unwrap();
        assert!((cpu.user - 2.5).abs() < f64::EPSILON);
        assert!((cpu.system - 0.75).abs() < f64::EPSILON);
    }

    #[test]
    fn parses_status() {
        let status = "Name:\tquack\nVmSize:\t  2048 kB\nVmRSS:\t   512 kB\nThreads:\t9\n";
        assert_eq!(
            parse_status(status),
            Status {
                resident_bytes: Some(512 * 1024),
                virtual_bytes: Some(2048 * 1024),
                threads: Some(9),
            }
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn reads_this_process() {
        assert!(cpu_time().is_some());
        assert!(status().and_then(|s| s.resident_bytes).unwrap_or(0) > 0);
        assert!(open_fds().unwrap_or(0) > 0);
    }
}
