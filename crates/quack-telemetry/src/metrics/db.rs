//! Database metrics, following the OpenTelemetry database conventions.

use std::time::Duration;

use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Histogram, Meter, ObservableUpDownCounter};

use super::observe::{Source, counter, histogram, observable_up_down_counter};
use crate::catalog;
use crate::keys;
use crate::labels::{
    Attribute, ConnectionState, DbCollection, DbErrorClass, DbOperation, PoolName,
    TransactionOutcome,
};

/// A snapshot of one connection pool, taken by the pool's owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolSample {
    /// The pool name.
    pub pool: PoolName,
    /// The most connections the pool allows.
    pub max: u32,
    /// Open connections waiting in the pool.
    pub idle: u32,
    /// Open connections checked out.
    pub used: u32,
}

/// Database metrics: queries, the pool, and transactions.
pub struct DbMetrics {
    operations: Histogram<f64>,
    wait_time: Histogram<f64>,
    timeouts: Counter<u64>,
    transactions: Histogram<f64>,
    retries: Counter<u64>,
    pools: Source<Vec<PoolSample>>,
    _count: ObservableUpDownCounter<i64>,
    _max: ObservableUpDownCounter<i64>,
}

impl DbMetrics {
    pub(crate) fn new(meter: &Meter) -> Self {
        let pools: Source<Vec<PoolSample>> = Source::new();
        let source = pools.clone();
        let count =
            observable_up_down_counter(meter, &catalog::DB_CONNECTION_COUNT, move |observer| {
                for sample in source.read().unwrap_or_default() {
                    observer.observe(
                        i64::from(sample.idle),
                        &[sample.pool.kv(), ConnectionState::Idle.kv()],
                    );
                    observer.observe(
                        i64::from(sample.used),
                        &[sample.pool.kv(), ConnectionState::Used.kv()],
                    );
                }
            });
        let source = pools.clone();
        let max = observable_up_down_counter(meter, &catalog::DB_CONNECTION_MAX, move |observer| {
            for sample in source.read().unwrap_or_default() {
                observer.observe(i64::from(sample.max), &[sample.pool.kv()]);
            }
        });
        Self {
            operations: histogram(meter, &catalog::DB_OPERATION_DURATION),
            wait_time: histogram(meter, &catalog::DB_CONNECTION_WAIT_TIME),
            timeouts: counter(meter, &catalog::DB_CONNECTION_TIMEOUTS),
            transactions: histogram(meter, &catalog::DB_TRANSACTION_DURATION),
            retries: counter(meter, &catalog::DB_TRANSACTION_RETRIES),
            pools,
            _count: count,
            _max: max,
        }
    }

    /// Records one query. Pass the error class when it failed.
    pub fn query(
        &self,
        operation: DbOperation,
        table: DbCollection,
        duration: Duration,
        error: Option<DbErrorClass>,
    ) {
        let system = KeyValue::new(keys::DB_SYSTEM_NAME, "postgresql");
        let seconds = duration.as_secs_f64();
        match error {
            Some(error) => self
                .operations
                .record(seconds, &[system, operation.kv(), table.kv(), error.kv()]),
            None => self
                .operations
                .record(seconds, &[system, operation.kv(), table.kv()]),
        }
    }

    /// Records the time spent waiting for a pool connection.
    pub fn connection_wait(&self, pool: PoolName, wait: Duration) {
        self.wait_time.record(wait.as_secs_f64(), &[pool.kv()]);
    }

    /// Counts a pool acquire that timed out.
    pub fn connection_timeout(&self, pool: PoolName) {
        self.timeouts.add(1, &[pool.kv()]);
    }

    /// Records a finished transaction.
    pub fn transaction(&self, outcome: TransactionOutcome, duration: Duration) {
        self.transactions
            .record(duration.as_secs_f64(), &[outcome.kv()]);
    }

    /// Counts a transaction run again after a serialization failure or deadlock.
    pub fn transaction_retry(&self, error: DbErrorClass) {
        self.retries.add(1, &[error.kv()]);
    }

    /// Installs the supplier of pool snapshots.
    pub fn observe_pools(&self, supplier: impl Fn() -> Vec<PoolSample> + Send + Sync + 'static) {
        self.pools.set(supplier);
    }
}
