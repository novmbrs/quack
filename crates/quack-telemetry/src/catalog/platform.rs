//! Catalog entries for the platform: jobs, Discord, HTTP, the database,
//! caches and queues, the Tokio runtime, and the process.

use super::buckets;
use crate::keys;
use crate::labels::{
    Attribute, CacheName, CacheResult, ConnectionState, CpuMode, DbCollection, DbErrorClass,
    DbOperation, GatewayConnectionEvent, GatewayEvent, HttpMethod, HttpRoute, JobKind, JobOutcome,
    PoolName, QueueName, RateLimitClass, RateLimitScope, RestOutcome, SignInOutcome,
    TransactionOutcome, UrlTemplate,
};

// Jobs.
spec!(JOBS_ENQUEUED: Counter "quack.jobs.enqueued", unit "{job}",
    [JobKind::KEY],
    "Jobs added to the queue.");
spec!(JOBS_STARTED: Counter "quack.jobs.started", unit "{job}",
    [JobKind::KEY],
    "Job runs started (a job that retries starts more than once).");
spec!(JOB_DURATION: Histogram "quack.job.duration", unit "s",
    [JobKind::KEY, JobOutcome::KEY],
    "Time to run a job once; its count is finished runs, and `retry`/`dead` outcomes count retries and dead jobs.",
    buckets buckets::WORK);
spec!(JOB_QUEUE_WAIT: Histogram "quack.job.queue.wait", unit "s",
    [JobKind::KEY],
    "Time a job was ready to run before a worker started it.",
    buckets buckets::LAG);
spec!(JOBS_READY: Gauge "quack.jobs.ready", unit "{job}",
    [JobKind::KEY],
    "Jobs ready to run now and not yet claimed.");
spec!(JOBS_OLDEST_READY_AGE: Gauge "quack.jobs.oldest_ready.age", unit "s",
    [JobKind::KEY],
    "How long the oldest ready job has been waiting.");

// Discord REST.
spec!(HTTP_CLIENT_REQUEST_DURATION: Histogram "http.client.request.duration", unit "s",
    [HttpMethod::KEY, UrlTemplate::KEY, keys::HTTP_RESPONSE_STATUS_CODE, RestOutcome::KEY],
    "Discord REST and OAuth calls (semconv); its count is requests by route, method, status, and outcome.",
    buckets buckets::REQUEST);
spec!(DISCORD_RATE_LIMIT_HITS: Counter "quack.discord.rate_limit.hits", unit "{hit}",
    [UrlTemplate::KEY, RateLimitScope::KEY],
    "Discord REST calls that had to wait for a rate limit, by route and scope.");
spec!(DISCORD_RATE_LIMIT_WAIT: Histogram "quack.discord.rate_limit.wait", unit "s",
    [UrlTemplate::KEY, RateLimitScope::KEY],
    "Time spent waiting for a Discord rate limit before a call.",
    buckets buckets::WAIT);

// Gateway.
spec!(GATEWAY_EVENT_DURATION: Histogram "quack.gateway.event.duration", unit "s",
    [GatewayEvent::KEY],
    "Time handling one gateway event; its count is events handled by type.",
    buckets buckets::WORK);
spec!(GATEWAY_HEARTBEAT_LATENCY: Histogram "quack.gateway.heartbeat.latency", unit "s",
    [],
    "Time between a gateway heartbeat and Discord's acknowledgement.",
    buckets buckets::WAIT);
spec!(GATEWAY_CONNECTION_EVENTS: Counter "quack.gateway.connection.events", unit "{event}",
    [GatewayConnectionEvent::KEY],
    "Gateway identifies, resumes, reconnects, and invalid sessions.");
spec!(GATEWAY_CONNECTED: Gauge "quack.gateway.connected", unit "{connection}",
    [],
    "1 while the gateway session is connected, else 0.");
spec!(GATEWAY_GUILDS: Gauge "quack.gateway.guilds", unit "{guild}",
    [],
    "Servers Quack is in.");

// HTTP API.
spec!(HTTP_SERVER_REQUEST_DURATION: Histogram "http.server.request.duration", unit "s",
    [HttpMethod::KEY, HttpRoute::KEY, keys::HTTP_RESPONSE_STATUS_CODE],
    "API requests (semconv); its count is requests by route, method, and status.",
    buckets buckets::REQUEST);
spec!(HTTP_SERVER_ACTIVE_REQUESTS: UpDownCounter "http.server.active_requests", unit "{request}",
    [HttpMethod::KEY],
    "API requests in flight (semconv).");
spec!(SIGN_INS: Counter "quack.auth.sign_ins", unit "{sign_in}",
    [SignInOutcome::KEY],
    "Dashboard sign-ins with Discord, by outcome.");
spec!(HTTP_RATE_LIMITED: Counter "quack.http.rate_limited", unit "{request}",
    [RateLimitClass::KEY],
    "API requests rejected by Quack's own rate limits, by class.");
spec!(SESSIONS_ACTIVE: Gauge "quack.sessions.active", unit "{session}",
    [],
    "Unexpired dashboard sessions.");

// Database.
spec!(DB_OPERATION_DURATION: Histogram "db.client.operation.duration", unit "s",
    [keys::DB_SYSTEM_NAME, DbOperation::KEY, DbCollection::KEY, DbErrorClass::KEY],
    "PostgreSQL queries (semconv); `error.type` is set only on failure.",
    buckets buckets::DB);
spec!(DB_CONNECTION_COUNT: ObservableUpDownCounter "db.client.connection.count", unit "{connection}",
    [PoolName::KEY, ConnectionState::KEY],
    "Pool connections by state (semconv).");
spec!(DB_CONNECTION_MAX: ObservableUpDownCounter "db.client.connection.max", unit "{connection}",
    [PoolName::KEY],
    "The most connections the pool allows (semconv).");
spec!(DB_CONNECTION_WAIT_TIME: Histogram "db.client.connection.wait_time", unit "s",
    [PoolName::KEY],
    "Time waiting to get a connection from the pool (semconv).",
    buckets buckets::DB);
spec!(DB_CONNECTION_TIMEOUTS: Counter "db.client.connection.timeouts", unit "{timeout}",
    [PoolName::KEY],
    "Times no pool connection was free in time (semconv).");
spec!(DB_TRANSACTION_DURATION: Histogram "quack.db.transaction.duration", unit "s",
    [TransactionOutcome::KEY],
    "Time from BEGIN to COMMIT or ROLLBACK.",
    buckets buckets::DB);
spec!(DB_TRANSACTION_RETRIES: Counter "quack.db.transaction.retries", unit "{retry}",
    [DbErrorClass::KEY],
    "Transactions run again after a serialization failure or deadlock.");

// Caches and queues.
spec!(CACHE_SIZE: Gauge "quack.cache.size", unit "{entry}",
    [CacheName::KEY],
    "Entries in each bounded in-memory cache.");
spec!(CACHE_LOOKUPS: Counter "quack.cache.lookups", unit "{lookup}",
    [CacheName::KEY, CacheResult::KEY],
    "Cache lookups, by hit or miss.");
spec!(CACHE_EVICTIONS: Counter "quack.cache.evictions", unit "{entry}",
    [CacheName::KEY],
    "Entries dropped from a cache to stay within its bound.");
spec!(QUEUE_DEPTH: Gauge "quack.queue.depth", unit "{item}",
    [QueueName::KEY],
    "Items waiting in each bounded in-memory queue.");
spec!(QUEUE_DROPPED: Counter "quack.queue.dropped", unit "{item}",
    [QueueName::KEY],
    "Items dropped because a bounded queue was full.");

// Tokio runtime (stable `RuntimeMetrics` only).
spec!(RUNTIME_WORKERS: Gauge "quack.runtime.workers", unit "{thread}",
    [],
    "Tokio worker threads.");
spec!(RUNTIME_TASKS: Gauge "quack.runtime.tasks", unit "{task}",
    [],
    "Tokio tasks alive.");
spec!(RUNTIME_GLOBAL_QUEUE_DEPTH: Gauge "quack.runtime.global_queue.depth", unit "{task}",
    [],
    "Tasks waiting in Tokio's global (injection) queue.");
spec!(RUNTIME_WORKER_BUSY_TIME: ObservableCounter "quack.runtime.worker.busy_time", unit "s",
    [],
    "Total time all workers spent busy; its rate divided by `quack.runtime.workers` is the busy ratio.");
spec!(RUNTIME_WORKER_PARKS: ObservableCounter "quack.runtime.worker.parks", unit "{park}",
    [],
    "Times workers parked for lack of work, summed over workers.");

// Process (semconv; read from /proc on Linux).
spec!(PROCESS_CPU_TIME: ObservableCounter "process.cpu.time", unit "s",
    [CpuMode::KEY],
    "CPU time used by the process, by mode.");
spec!(PROCESS_MEMORY_USAGE: ObservableUpDownCounter "process.memory.usage", unit "By",
    [],
    "Resident memory (RSS).");
spec!(PROCESS_MEMORY_VIRTUAL: ObservableUpDownCounter "process.memory.virtual", unit "By",
    [],
    "Virtual memory size.");
spec!(PROCESS_FILE_DESCRIPTORS: ObservableUpDownCounter "process.unix.file_descriptor.count", unit "{file_descriptor}",
    [],
    "Open file descriptors.");
spec!(PROCESS_THREADS: ObservableUpDownCounter "process.thread.count", unit "{thread}",
    [],
    "Threads in the process.");
spec!(PROCESS_UPTIME: Gauge "process.uptime", unit "s",
    [],
    "Time since the process started telemetry.");
spec!(BUILD_INFO: Gauge "quack.build.info", unit "{build}",
    [keys::SERVICE_VERSION],
    "Always 1; carries the running version as an attribute.");
