//! Explicit histogram bucket boundaries, one set per family of measurement.
//!
//! Durations are in seconds (OpenTelemetry convention). Each set puts most of
//! its resolution where the interesting decisions are.

/// Interaction acknowledgement: fine below Discord's 3 s deadline.
pub(super) const ACK: &[f64] = &[
    0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.15, 0.2, 0.3, 0.4, 0.5, 0.75, 1.0, 1.5, 2.0, 2.5, 3.0,
    5.0,
];

/// Request/response calls (HTTP server and client), the semconv default.
pub(super) const REQUEST: &[f64] = &[
    0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0,
];

/// Database queries and transactions: sub-millisecond to seconds.
pub(super) const DB: &[f64] = &[
    0.0005, 0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

/// Whole units of work (interactions, jobs, case creation).
pub(super) const WORK: &[f64] = &[
    0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0,
];

/// Waiting and lag: queue wait, sync lag, case-to-action time (up to an hour).
pub(super) const LAG: &[f64] = &[
    0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0, 300.0, 900.0, 3600.0,
];

/// Rate-limit waits and gateway heartbeats.
pub(super) const WAIT: &[f64] = &[
    0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0,
];

/// Human decisions: one minute to thirty days.
pub(super) const HUMAN: &[f64] = &[
    60.0,
    300.0,
    900.0,
    1800.0,
    3600.0,
    10_800.0,
    21_600.0,
    43_200.0,
    86_400.0,
    172_800.0,
    259_200.0,
    604_800.0,
    1_209_600.0,
    2_592_000.0,
];

/// Escalation level numbers (1-based).
pub(super) const LEVEL: &[f64] = &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];

/// Byte sizes from 1 KiB to 16 MiB.
pub(super) const BYTES: &[f64] = &[
    1024.0,
    4096.0,
    16_384.0,
    65_536.0,
    262_144.0,
    1_048_576.0,
    4_194_304.0,
    16_777_216.0,
];
