//! Tokio runtime, process, and build metrics.
//!
//! Tokio metrics use only the stable `RuntimeMetrics` API: worker count,
//! alive tasks, global queue depth, and per-worker busy time and park counts
//! (summed). Local queue depths, steal and overflow counts, blocking-pool
//! figures, and poll-time histograms need `--cfg tokio_unstable` and are not
//! exported. The busy ratio is `rate(quack.runtime.worker.busy_time) /
//! quack.runtime.workers`.

use std::sync::{Arc, PoisonError, RwLock};
use std::time::{Duration, Instant};

use opentelemetry::KeyValue;
use opentelemetry::metrics::{Meter, ObservableCounter, ObservableGauge, ObservableUpDownCounter};
use tokio::runtime::Handle;

use super::observe::{
    f64_gauge, f64_observable_counter, observable_up_down_counter, saturating_i64, u64_gauge,
    u64_observable_counter,
};
use super::process;
use crate::catalog;
use crate::keys;
use crate::labels::{Attribute, CpuMode};

type SharedHandle = Arc<RwLock<Option<Handle>>>;

/// Runtime metrics: Tokio, the process, and build info. Process and build
/// metrics start reporting as soon as the catalog exists; Tokio metrics
/// start after [`Self::observe_tokio`].
pub struct RuntimeMetrics {
    handle: SharedHandle,
    _gauges: Vec<ObservableGauge<u64>>,
    _f64_gauges: Vec<ObservableGauge<f64>>,
    _counters: (
        ObservableCounter<f64>,
        ObservableCounter<u64>,
        ObservableCounter<f64>,
    ),
    _process: Vec<ObservableUpDownCounter<i64>>,
}

impl RuntimeMetrics {
    pub(crate) fn new(meter: &Meter, version: &str) -> Self {
        let handle: SharedHandle = Arc::new(RwLock::new(None));
        let started = Instant::now();
        let version = KeyValue::new(keys::SERVICE_VERSION, version.to_owned());

        let gauges = vec![
            tokio_gauge(
                meter,
                &handle,
                &catalog::RUNTIME_WORKERS,
                tokio::runtime::RuntimeMetrics::num_workers,
            ),
            tokio_gauge(
                meter,
                &handle,
                &catalog::RUNTIME_TASKS,
                tokio::runtime::RuntimeMetrics::num_alive_tasks,
            ),
            tokio_gauge(meter, &handle, &catalog::RUNTIME_GLOBAL_QUEUE_DEPTH, |m| {
                m.global_queue_depth()
            }),
            u64_gauge(meter, &catalog::BUILD_INFO, move |observer| {
                observer.observe(1, std::slice::from_ref(&version));
            }),
        ];
        let f64_gauges = vec![f64_gauge(
            meter,
            &catalog::PROCESS_UPTIME,
            move |observer| {
                observer.observe(started.elapsed().as_secs_f64(), &[]);
            },
        )];

        let source = Arc::clone(&handle);
        let busy =
            f64_observable_counter(meter, &catalog::RUNTIME_WORKER_BUSY_TIME, move |observer| {
                if let Some(metrics) = runtime_metrics(&source) {
                    let busy: Duration = (0..metrics.num_workers())
                        .map(|worker| metrics.worker_total_busy_duration(worker))
                        .sum();
                    observer.observe(busy.as_secs_f64(), &[]);
                }
            });
        let source = Arc::clone(&handle);
        let parks =
            u64_observable_counter(meter, &catalog::RUNTIME_WORKER_PARKS, move |observer| {
                if let Some(metrics) = runtime_metrics(&source) {
                    let parks = (0..metrics.num_workers())
                        .map(|worker| metrics.worker_park_count(worker))
                        .fold(0_u64, u64::saturating_add);
                    observer.observe(parks, &[]);
                }
            });
        let cpu = f64_observable_counter(meter, &catalog::PROCESS_CPU_TIME, |observer| {
            if let Some(cpu) = process::cpu_time() {
                observer.observe(cpu.user, &[CpuMode::User.kv()]);
                observer.observe(cpu.system, &[CpuMode::System.kv()]);
            }
        });

        let process = vec![
            status_counter(meter, &catalog::PROCESS_MEMORY_USAGE, |s| s.resident_bytes),
            status_counter(meter, &catalog::PROCESS_MEMORY_VIRTUAL, |s| s.virtual_bytes),
            status_counter(meter, &catalog::PROCESS_THREADS, |s| s.threads),
            observable_up_down_counter(meter, &catalog::PROCESS_FILE_DESCRIPTORS, |observer| {
                if let Some(fds) = process::open_fds() {
                    observer.observe(saturating_i64(fds), &[]);
                }
            }),
        ];

        Self {
            handle,
            _gauges: gauges,
            _f64_gauges: f64_gauges,
            _counters: (busy, parks, cpu),
            _process: process,
        }
    }

    /// Starts reporting metrics for the Tokio runtime behind `handle`
    /// (normally `Handle::current()` from inside the runtime).
    pub fn observe_tokio(&self, handle: Handle) {
        let mut slot = self.handle.write().unwrap_or_else(PoisonError::into_inner);
        *slot = Some(handle);
    }
}

fn runtime_metrics(handle: &SharedHandle) -> Option<tokio::runtime::RuntimeMetrics> {
    let slot = handle.read().unwrap_or_else(PoisonError::into_inner);
    slot.as_ref().map(Handle::metrics)
}

fn tokio_gauge(
    meter: &Meter,
    handle: &SharedHandle,
    spec: &'static catalog::InstrumentSpec,
    read: fn(&tokio::runtime::RuntimeMetrics) -> usize,
) -> ObservableGauge<u64> {
    let source = Arc::clone(handle);
    u64_gauge(meter, spec, move |observer| {
        if let Some(metrics) = runtime_metrics(&source) {
            observer.observe(u64::try_from(read(&metrics)).unwrap_or(u64::MAX), &[]);
        }
    })
}

fn status_counter(
    meter: &Meter,
    spec: &'static catalog::InstrumentSpec,
    read: fn(&process::Status) -> Option<u64>,
) -> ObservableUpDownCounter<i64> {
    observable_up_down_counter(meter, spec, move |observer| {
        if let Some(value) = process::status().as_ref().and_then(read) {
            observer.observe(saturating_i64(value), &[]);
        }
    })
}
