# Quack metrics

<!-- Generated from crates/quack-telemetry/src/catalog. Do not edit by hand.
     Regenerate with: UPDATE_METRICS_DOC=1 cargo test -p quack-telemetry metrics_doc -->

Every instrument Quack exports over OTLP. Names start with `quack.` unless an
OpenTelemetry semantic convention defines the instrument. Durations are in
seconds. Metric attributes never carry ids; ids go on spans and logs.
Histograms also count: use a histogram's count for the rate of the thing it
times (requests, job runs, interactions handled).

## Instruments

| Name | Type | Unit | Attributes | Description |
| --- | --- | --- | --- | --- |
| `quack.interaction.ack.duration` | histogram | `s` | `quack.interaction.kind`, `quack.command` | Time from receiving an interaction to acknowledging it (Discord allows 3 s). |
| `quack.interaction.duration` | histogram | `s` | `quack.interaction.kind`, `quack.command`, `quack.interaction.outcome` | Total time handling an interaction, through the final reply; its count is interactions handled. |
| `quack.interaction.errors` | counter | `{error}` | `quack.interaction.kind`, `quack.command`, `quack.reason` | Interactions that ended refused or failed, by reason. |
| `quack.cases.created` | counter | `{case}` | `quack.case.source`, `quack.action.kind` | Cases created, by where they came from and the level's action (`none` for a warning). |
| `quack.cases.refused` | counter | `{refusal}` | `quack.case.source`, `quack.reason` | Case creations refused (member checks, capabilities, 2FA, Quack's access), by source and reason. |
| `quack.cases.voided` | counter | `{case}` | `quack.void.cause` | Cases voided, by cause. |
| `quack.case.create.duration` | histogram | `s` | `quack.case.source` | Time to validate, count, choose the level, and commit a new case. |
| `quack.case.level` | histogram | `{level}` | `quack.action.kind` | The escalation level number (1-based) chosen for each new case. |
| `quack.action.attempts` | counter | `{attempt}` | `quack.action.kind`, `quack.action.attempt.outcome`, `quack.reason` | Attempts to carry out an action, by attempt outcome and failure reason. |
| `quack.actions.finished` | counter | `{action}` | `quack.action.kind`, `quack.action.state` | Actions that reached a terminal state (or failed), by kind and state. |
| `quack.action.completion.duration` | histogram | `s` | `quack.action.kind` | Time from case creation until its action is done, retries included. |
| `quack.action.retries` | counter | `{retry}` | `quack.action.kind`, `quack.reason` | Action attempts scheduled again after a retryable failure. |
| `quack.actions.failed` | gauge (observed) | `{action}` | `quack.action.kind` | Actions currently in the failed state, waiting for staff to retry or dismiss. |
| `quack.action.undo.skipped` | counter | `{action}` | `quack.action.kind` | Undo actions left in place because another valid case still applies. |
| `quack.dm.outcomes` | counter | `{dm}` | `quack.dm.kind`, `quack.dm.outcome`, `quack.reason` | Final DM outcomes, by kind and failure reason. |
| `quack.dm.send.duration` | histogram | `s` | `quack.dm.kind` | Time to open the DM channel and send the message. |
| `quack.appeal.transitions` | counter | `{transition}` | `quack.appeal.transition` | Appeal changes, including submissions and staff decisions. |
| `quack.appeal.decision.duration` | histogram | `s` | `quack.appeal.transition` | Time from submission to the staff decision (accept, reject, or close). |
| `quack.appeals.open` | gauge (observed) | `{appeal}` | — | Appeals waiting for staff. |
| `quack.evidence.captures` | counter | `{capture}` | `quack.outcome` | Evidence capture jobs, by outcome. |
| `quack.evidence.files` | counter | `{file}` | — | Files copied to the evidence channel. |
| `quack.evidence.bytes` | counter | `By` | — | Bytes copied to the evidence channel. |
| `quack.modlog.entries` | counter | `{entry}` | `quack.modlog.kind`, `quack.modlog.result` | Moderation-log entries written, by entry kind and the result they record. |
| `quack.modlog.posts` | counter | `{post}` | `quack.outcome` | Moderation-log entries posted to the log channel, by outcome. |
| `quack.message.syncs` | counter | `{sync}` | `quack.message.kind`, `quack.outcome` | Runs that bring a tracked Discord message up to date, by outcome. |
| `quack.message.sync.lag` | histogram | `s` | `quack.message.kind` | Time from a change to the tracked message showing it. |
| `quack.jobs.enqueued` | counter | `{job}` | `quack.job.kind` | Jobs added to the queue. |
| `quack.jobs.started` | counter | `{job}` | `quack.job.kind` | Job runs started (a job that retries starts more than once). |
| `quack.job.duration` | histogram | `s` | `quack.job.kind`, `quack.job.outcome` | Time to run a job once; its count is finished runs by outcome (`retry` and `dead` count retries and dead jobs). |
| `quack.job.queue.wait` | histogram | `s` | `quack.job.kind` | Time a job was ready to run before a worker started it. |
| `quack.job.lease_expired` | counter | `{job}` | `quack.job.kind` | Claimed jobs whose lease expired before their run settled them. |
| `quack.jobs.ready` | gauge (observed) | `{job}` | `quack.job.kind` | Jobs ready to run now and not yet claimed. |
| `quack.jobs.oldest_ready.age` | gauge (observed) | `s` | `quack.job.kind` | How long the oldest ready job has been waiting. |
| `quack.jobs.dead` | gauge (observed) | `{job}` | `quack.job.kind` | Dead jobs kept for inspection (7 days). |
| `http.client.request.duration` | histogram | `s` | `http.request.method`, `url.template`, `http.response.status_code`, `quack.discord.outcome` | Discord REST and OAuth calls (semconv); its count is requests by route, method, status, and outcome. |
| `quack.discord.rate_limit.hits` | counter | `{hit}` | `url.template`, `quack.discord.rate_limit.scope` | Discord REST calls that had to wait for a rate limit, by route and scope. |
| `quack.discord.rate_limit.wait` | histogram | `s` | `url.template`, `quack.discord.rate_limit.scope` | Time spent waiting for a Discord rate limit before a call. |
| `quack.gateway.event.duration` | histogram | `s` | `quack.gateway.event` | Time handling one gateway event; its count is events handled by type. |
| `quack.gateway.heartbeat.latency` | histogram | `s` | — | Time between a gateway heartbeat and Discord's acknowledgement. |
| `quack.gateway.connection.events` | counter | `{event}` | `quack.gateway.connection.event` | Gateway identifies, resumes, reconnects, and invalid sessions. |
| `quack.gateway.connected` | gauge (observed) | `{connection}` | — | 1 while the gateway session is connected, else 0. |
| `quack.gateway.guilds` | gauge (observed) | `{guild}` | — | Servers Quack is in. |
| `http.server.request.duration` | histogram | `s` | `http.request.method`, `http.route`, `http.response.status_code` | API requests (semconv); its count is requests by route, method, and status. |
| `http.server.active_requests` | updowncounter | `{request}` | `http.request.method` | API requests in flight (semconv). |
| `quack.auth.sign_ins` | counter | `{sign_in}` | `quack.sign_in.outcome` | Dashboard sign-ins with Discord, by outcome. |
| `quack.http.rate_limited` | counter | `{request}` | `quack.rate_limit.class` | API requests rejected by Quack's own rate limits, by class. |
| `quack.sessions.active` | gauge (observed) | `{session}` | — | Unexpired dashboard sessions. |
| `db.client.operation.duration` | histogram | `s` | `db.system.name`, `db.operation.name`, `db.collection.name`, `error.type` | PostgreSQL queries (semconv); `error.type` is set only on failure. |
| `db.client.connection.count` | updowncounter (observed) | `{connection}` | `db.client.connection.pool.name`, `db.client.connection.state` | Pool connections by state (semconv). |
| `db.client.connection.max` | updowncounter (observed) | `{connection}` | `db.client.connection.pool.name` | The most connections the pool allows (semconv). |
| `db.client.connection.wait_time` | histogram | `s` | `db.client.connection.pool.name` | Time waiting to get a connection from the pool (semconv). |
| `db.client.connection.timeouts` | counter | `{timeout}` | `db.client.connection.pool.name` | Times no pool connection was free in time (semconv). |
| `quack.db.transaction.duration` | histogram | `s` | `quack.db.transaction.outcome` | Time from BEGIN to COMMIT or ROLLBACK. |
| `quack.db.transaction.retries` | counter | `{retry}` | `error.type` | Transactions run again after a serialization failure or deadlock. |
| `quack.cache.size` | gauge (observed) | `{entry}` | `quack.cache.name` | Entries in each bounded in-memory cache. |
| `quack.cache.lookups` | counter | `{lookup}` | `quack.cache.name`, `quack.cache.result` | Cache lookups, by hit or miss. |
| `quack.cache.evictions` | counter | `{entry}` | `quack.cache.name` | Entries dropped from a cache to stay within its bound. |
| `quack.queue.depth` | gauge (observed) | `{item}` | `quack.queue.name` | Items waiting in each bounded in-memory queue. |
| `quack.queue.dropped` | counter | `{item}` | `quack.queue.name` | Items dropped because a bounded queue was full. |
| `quack.tickets.opened` | counter | `{ticket}` | — | Tickets opened. |
| `quack.tickets.closed` | counter | `{ticket}` | — | Tickets closed. |
| `quack.tickets.open` | gauge (observed) | `{ticket}` | — | Tickets currently open. |
| `quack.ticket.transcript.size` | histogram | `By` | — | Size of each saved ticket transcript. |
| `quack.activity_log.events` | counter | `{event}` | `quack.activity.event`, `quack.activity.stage` | Server activity log events seen, posted, dropped, and failed, by event type. |
| `quack.honeypot.hits` | counter | `{hit}` | `quack.honeypot.outcome` | Posts in a honeypot channel, by what Quack did. |
| `quack.honeypot.cleanups` | counter | `{cleanup}` | `quack.outcome` | Honeypot cleanup runs, by outcome. |
| `quack.runtime.workers` | gauge (observed) | `{thread}` | — | Tokio worker threads. |
| `quack.runtime.tasks` | gauge (observed) | `{task}` | — | Tokio tasks alive. |
| `quack.runtime.global_queue.depth` | gauge (observed) | `{task}` | — | Tasks waiting in Tokio's global (injection) queue. |
| `quack.runtime.worker.busy_time` | counter (observed) | `s` | — | Total time all workers spent busy; its rate divided by `quack.runtime.workers` is the busy ratio. |
| `quack.runtime.worker.parks` | counter (observed) | `{park}` | — | Times workers parked for lack of work, summed over workers. |
| `process.cpu.time` | counter (observed) | `s` | `cpu.mode` | CPU time used by the process, by mode. |
| `process.memory.usage` | updowncounter (observed) | `By` | — | Resident memory (RSS). |
| `process.memory.virtual` | updowncounter (observed) | `By` | — | Virtual memory size. |
| `process.unix.file_descriptor.count` | updowncounter (observed) | `{file_descriptor}` | — | Open file descriptors. |
| `process.thread.count` | updowncounter (observed) | `{thread}` | — | Threads in the process. |
| `process.uptime` | gauge (observed) | `s` | — | Time since the process started telemetry. |
| `quack.build.info` | gauge (observed) | `{build}` | `service.version` | Always 1; carries the running version as an attribute. |

## Histogram buckets

| Name | Bucket boundaries |
| --- | --- |
| `quack.interaction.ack.duration` | 0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.15, 0.2, 0.3, 0.4, 0.5, 0.75, 1, 1.5, 2, 2.5, 3, 5 |
| `quack.interaction.duration` | 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1, 2.5, 5, 10, 30, 60, 120, 300 |
| `quack.case.create.duration` | 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1, 2.5, 5, 10, 30, 60, 120, 300 |
| `quack.case.level` | 1, 2, 3, 4, 5, 6, 7, 8, 9, 10 |
| `quack.action.completion.duration` | 0.01, 0.05, 0.1, 0.25, 0.5, 1, 2.5, 5, 10, 30, 60, 300, 900, 3600 |
| `quack.dm.send.duration` | 0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1, 2.5, 5, 7.5, 10 |
| `quack.appeal.decision.duration` | 60, 300, 900, 1800, 3600, 10800, 21600, 43200, 86400, 172800, 259200, 604800, 1209600, 2592000 |
| `quack.message.sync.lag` | 0.01, 0.05, 0.1, 0.25, 0.5, 1, 2.5, 5, 10, 30, 60, 300, 900, 3600 |
| `quack.job.duration` | 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1, 2.5, 5, 10, 30, 60, 120, 300 |
| `quack.job.queue.wait` | 0.01, 0.05, 0.1, 0.25, 0.5, 1, 2.5, 5, 10, 30, 60, 300, 900, 3600 |
| `http.client.request.duration` | 0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1, 2.5, 5, 7.5, 10 |
| `quack.discord.rate_limit.wait` | 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1, 2, 5, 10, 30, 60 |
| `quack.gateway.event.duration` | 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1, 2.5, 5, 10, 30, 60, 120, 300 |
| `quack.gateway.heartbeat.latency` | 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1, 2, 5, 10, 30, 60 |
| `http.server.request.duration` | 0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1, 2.5, 5, 7.5, 10 |
| `db.client.operation.duration` | 0.0005, 0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1, 2.5, 5, 10 |
| `db.client.connection.wait_time` | 0.0005, 0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1, 2.5, 5, 10 |
| `quack.db.transaction.duration` | 0.0005, 0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1, 2.5, 5, 10 |
| `quack.ticket.transcript.size` | 1024, 4096, 16384, 65536, 262144, 1048576, 4194304, 16777216 |

## Attributes

| Key | Values | Description |
| --- | --- | --- |
| `quack.interaction.kind` | `slash_command`, `user_command`, `message_command`, `component`, `modal`, `autocomplete` | Kind of Discord interaction. |
| `quack.interaction.outcome` | `done`, `refused`, `failed` | How handling an interaction ended. |
| `quack.command` | static names chosen in code | Application command name. |
| `quack.outcome` | `done`, `failed`, `unknown`, `skipped` | Terminal outcome of the work. |
| `quack.reason` | `missing_permission`, `role_hierarchy`, `member_is_owner`, `member_not_found`, `dms_closed`, `no_shared_server`, `not_allowed`, `invalid`, `conflict`, `rate_limited`, `timeout`, `discord`, `database`, `retries_exhausted`, `internal` | Why it failed or was refused; absent on success. |
| `quack.action.kind` | `timeout`, `kick`, `ban`, `remove_timeout`, `unban`, `none` | Action kind. |
| `quack.action.attempt.outcome` | `done`, `no_change`, `refused`, `not_sent`, `unknown`, `interrupted` | How one action attempt ended. |
| `quack.action.state` | `done`, `failed`, `cancelled`, `skipped` | The state an action finished in. |
| `quack.case.source` | `discord`, `dashboard`, `honeypot` | Where the case was created. |
| `quack.void.cause` | `staff`, `appeal_accepted` | Why the case was voided. |
| `quack.dm.kind` | `case`, `appeal_update`, `ticket_closed` | What the DM is about. |
| `quack.dm.outcome` | `sent`, `failed`, `unknown`, `not_needed` | Terminal state of the DM. |
| `quack.appeal.transition` | `submit`, `accept`, `reject`, `request_info`, `reply`, `close`, `reopen` | Appeal change. |
| `quack.modlog.kind` | static names chosen in code | Moderation-log entry kind. |
| `quack.modlog.result` | `ok`, `failed`, `refused` | The result a moderation-log entry records. |
| `quack.message.kind` | `case_receipt`, `appeal_post`, `ticket_panel`, `ticket_post`, `honeypot_warning` | Tracked Discord message kind. |
| `quack.job.kind` | `run_action`, `send_dm`, `sync_message`, `post_mod_log`, `capture_evidence`, `honeypot_case`, `honeypot_cleanup`, `open_ticket`, `close_ticket`, `housekeeping` | Background job kind. |
| `quack.job.outcome` | `done`, `retry`, `deferred`, `continued`, `rescheduled`, `finalized`, `dead`, `lost_lease`, `timed_out` | How one job run ended. |
| `http.request.method` | `GET`, `HEAD`, `POST`, `PUT`, `PATCH`, `DELETE`, `OPTIONS`, `_OTHER` | HTTP method (semconv). |
| `http.route` | static names chosen in code | API route template (semconv). |
| `url.template` | static names chosen in code | Discord REST route template (semconv). |
| `http.response.status_code` | static names chosen in code | HTTP status code (semconv); absent when no response arrived. |
| `quack.discord.outcome` | `done`, `refused`, `unknown`, `rate_limited`, `not_sent` | Outcome class of a Discord REST call. |
| `quack.discord.rate_limit.scope` | `route`, `global`, `shared` | Which Discord rate limit applied. |
| `quack.gateway.event` | static names chosen in code | Gateway event type. |
| `quack.gateway.connection.event` | `identify`, `resume`, `reconnect`, `invalid_session` | Gateway connection change. |
| `quack.sign_in.outcome` | `done`, `cancelled`, `refused`, `failed` | How a sign-in ended. |
| `quack.rate_limit.class` | static names chosen in code | API rate-limit class. |
| `db.system.name` | `postgresql` | Database system (semconv). |
| `db.operation.name` | `SELECT`, `INSERT`, `UPDATE`, `DELETE`, `NOTIFY`, `MIGRATE` | Database operation (semconv). |
| `db.collection.name` | static names chosen in code | Database table (semconv). |
| `error.type` | `serialization_failure`, `deadlock`, `unique_violation`, `foreign_key_violation`, `check_violation`, `connection`, `timeout`, `pool_timeout`, `decode`, `_OTHER` | Database error class (semconv `error.type`); absent on success. |
| `db.client.connection.pool.name` | static names chosen in code | Database pool name (semconv). |
| `db.client.connection.state` | `idle`, `used` | Pool connection state (semconv). |
| `quack.db.transaction.outcome` | `commit`, `rollback` | How a transaction ended. |
| `quack.cache.name` | static names chosen in code | Bounded in-memory cache. |
| `quack.cache.result` | `hit`, `miss` | Cache lookup result. |
| `quack.queue.name` | static names chosen in code | Bounded in-memory queue. |
| `quack.activity.event` | static names chosen in code | Server activity log event type. |
| `quack.activity.stage` | `seen`, `posted`, `dropped`, `failed` | How far the event got. |
| `quack.honeypot.outcome` | `actioned`, `exempt`, `failed` | What Quack did about a honeypot post. |
| `cpu.mode` | `user`, `system` | CPU mode (semconv). |
| `service.version` | static names chosen in code | The running Quack version (semconv). |
