//! Callback sources for observed instruments, and instrument builders that
//! read the catalog.
//!
//! The crate that owns a value (the job queue, the pool, a cache) installs a
//! cheap, non-blocking closure once; the SDK calls it at each collection on
//! its own thread. A closure should read a value the owner already keeps
//! (an atomic, `len()` of a bounded structure, or a snapshot refreshed by the
//! owner), never do I/O.

use std::sync::{Arc, PoisonError, RwLock};

use opentelemetry::metrics::{
    AsyncInstrument, Counter, Histogram, Meter, ObservableCounter, ObservableGauge,
    ObservableUpDownCounter, UpDownCounter,
};

use crate::catalog::{InstrumentKind, InstrumentSpec};

type Supplier<T> = Box<dyn Fn() -> T + Send + Sync>;

/// One replaceable supplier of an observed value.
pub(crate) struct Source<T>(Arc<RwLock<Option<Supplier<T>>>>);

impl<T> Clone for Source<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T> Source<T> {
    pub(crate) fn new() -> Self {
        Self(Arc::new(RwLock::new(None)))
    }

    /// Installs or replaces the supplier.
    pub(crate) fn set(&self, supplier: impl Fn() -> T + Send + Sync + 'static) {
        let mut slot = self.0.write().unwrap_or_else(PoisonError::into_inner);
        *slot = Some(Box::new(supplier));
    }

    /// Calls the supplier, if one is installed.
    pub(crate) fn read(&self) -> Option<T> {
        let slot = self.0.read().unwrap_or_else(PoisonError::into_inner);
        slot.as_ref().map(|supplier| supplier())
    }
}

type Keyed<K, T> = Vec<(K, Supplier<T>)>;

/// Suppliers keyed by a label, such as one per cache.
pub(crate) struct Sources<K, T>(Arc<RwLock<Keyed<K, T>>>);

impl<K, T> Clone for Sources<K, T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<K: PartialEq + Copy, T> Sources<K, T> {
    pub(crate) fn new() -> Self {
        Self(Arc::new(RwLock::new(Vec::new())))
    }

    /// Installs the supplier for `key`, replacing any earlier one.
    pub(crate) fn set(&self, key: K, supplier: impl Fn() -> T + Send + Sync + 'static) {
        let mut slots = self.0.write().unwrap_or_else(PoisonError::into_inner);
        slots.retain(|(existing, _)| *existing != key);
        slots.push((key, Box::new(supplier)));
    }

    /// Calls every supplier with its key.
    pub(crate) fn for_each(&self, mut f: impl FnMut(K, T)) {
        let slots = self.0.read().unwrap_or_else(PoisonError::into_inner);
        for (key, supplier) in slots.iter() {
            f(*key, supplier());
        }
    }
}

/// Converts a count to the `i64` that up-down counters take, saturating.
pub(crate) fn saturating_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn check(spec: &InstrumentSpec, kind: InstrumentKind) {
    debug_assert_eq!(spec.kind, kind, "{} is built as the wrong type", spec.name);
}

pub(crate) fn counter(meter: &Meter, spec: &'static InstrumentSpec) -> Counter<u64> {
    check(spec, InstrumentKind::Counter);
    meter
        .u64_counter(spec.name)
        .with_unit(spec.unit)
        .with_description(spec.description)
        .build()
}

pub(crate) fn up_down_counter(meter: &Meter, spec: &'static InstrumentSpec) -> UpDownCounter<i64> {
    check(spec, InstrumentKind::UpDownCounter);
    meter
        .i64_up_down_counter(spec.name)
        .with_unit(spec.unit)
        .with_description(spec.description)
        .build()
}

pub(crate) fn histogram(meter: &Meter, spec: &'static InstrumentSpec) -> Histogram<f64> {
    check(spec, InstrumentKind::Histogram);
    meter
        .f64_histogram(spec.name)
        .with_unit(spec.unit)
        .with_description(spec.description)
        .with_boundaries(spec.buckets.to_vec())
        .build()
}

pub(crate) fn u64_gauge(
    meter: &Meter,
    spec: &'static InstrumentSpec,
    callback: impl Fn(&dyn AsyncInstrument<u64>) + Send + Sync + 'static,
) -> ObservableGauge<u64> {
    check(spec, InstrumentKind::Gauge);
    meter
        .u64_observable_gauge(spec.name)
        .with_unit(spec.unit)
        .with_description(spec.description)
        .with_callback(callback)
        .build()
}

pub(crate) fn f64_gauge(
    meter: &Meter,
    spec: &'static InstrumentSpec,
    callback: impl Fn(&dyn AsyncInstrument<f64>) + Send + Sync + 'static,
) -> ObservableGauge<f64> {
    check(spec, InstrumentKind::Gauge);
    meter
        .f64_observable_gauge(spec.name)
        .with_unit(spec.unit)
        .with_description(spec.description)
        .with_callback(callback)
        .build()
}

pub(crate) fn u64_observable_counter(
    meter: &Meter,
    spec: &'static InstrumentSpec,
    callback: impl Fn(&dyn AsyncInstrument<u64>) + Send + Sync + 'static,
) -> ObservableCounter<u64> {
    check(spec, InstrumentKind::ObservableCounter);
    meter
        .u64_observable_counter(spec.name)
        .with_unit(spec.unit)
        .with_description(spec.description)
        .with_callback(callback)
        .build()
}

pub(crate) fn f64_observable_counter(
    meter: &Meter,
    spec: &'static InstrumentSpec,
    callback: impl Fn(&dyn AsyncInstrument<f64>) + Send + Sync + 'static,
) -> ObservableCounter<f64> {
    check(spec, InstrumentKind::ObservableCounter);
    meter
        .f64_observable_counter(spec.name)
        .with_unit(spec.unit)
        .with_description(spec.description)
        .with_callback(callback)
        .build()
}

pub(crate) fn observable_up_down_counter(
    meter: &Meter,
    spec: &'static InstrumentSpec,
    callback: impl Fn(&dyn AsyncInstrument<i64>) + Send + Sync + 'static,
) -> ObservableUpDownCounter<i64> {
    check(spec, InstrumentKind::ObservableUpDownCounter);
    meter
        .i64_observable_up_down_counter(spec.name)
        .with_unit(spec.unit)
        .with_description(spec.description)
        .with_callback(callback)
        .build()
}
