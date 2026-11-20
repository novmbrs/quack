# Agent Guidelines

## What Quack is

Quack is a Discord moderation bot built around one rule:

> Admins define the server's moderation rules. Moderators apply those rules.
> Quack chooses and carries out the configured result.

Rules managers build **rules** (one per kind of problem, not per
punishment) with escalation **levels** keyed on case count. A moderator
applies a rule to a member; Quack counts that member's valid cases for the
rule, picks the level, records the **case**, runs its single timeout, kick,
or ban **action**, sends at most one **DM**, and writes the **moderation
log**. Members can **appeal** from the dashboard. Discord and the dashboard
share the same behavior; Discord stays the authority for permissions. Data
never crosses servers, cases keep the rule version they used, and cases and
log entries are voided or appended to, never deleted.

## The system is documented in `docs/design.md`

`docs/design.md` is the source of truth for the whole system: product rules,
architecture, configuration, and operations. Read it before making changes.
When code and the design disagree, treat it as drift and resolve it
deliberately. Any change that affects behavior, configuration, APIs, or
anything else the docs describe must update `docs/design.md` (and any other
affected docs) in the same commit. Words follow the glossary (§1.3): rule,
level, case, action, DM, appeal, moderation log, server activity log.

## Crates are lego pieces

One Rust workspace in `crates/` (edition 2024, Rust 1.100, pinned
dependencies in the root `Cargo.toml`), one binary, `quack`.

| Crate | Owns | May depend on |
| --- | --- | --- |
| `quack-core` | The pure domain: ids, rules and escalation, permissions, state machines, moderation log kinds, limits, domain errors. No I/O. | — |
| `quack-text` | Every string a person reads, as a neutral message model; snapshot-tested. | core |
| `quack-telemetry` | OpenTelemetry setup and the typed instrument catalog (`METRICS.md`). | — |
| `quack-db` | PostgreSQL: migrations, every query (sqlx, offline data in `.sqlx/`), the job queue, sessions. | core, telemetry |
| `quack-discord` | Twilight: gateway, caches, REST with typed outcomes, rendering, custom ids, commands. | core, text, telemetry |
| `quack-engine` | Use cases and job handlers; defines the `Discord` port. | core, text, db, telemetry |
| `quack-tickets`, `quack-activity-log`, `quack-honeypot` | The optional modules, as service functions plus `ModuleJobs`/`ModuleHooks`. | core, text, db, engine, telemetry |
| `quack-ops` | Ops in Discord: the ops server's channels, durable posts, alerts, the status board, the error feed, `/ops` answers. | core, text, db, engine, telemetry |
| `quack-api` | axum HTTP API, Discord sign-in, sessions, CSRF, rate limits, OpenAPI, dashboard files. | core, text, db, engine, modules, telemetry |
| `quack-bot` | Interactions and gateway events into engine calls and replies; the module adapters, `/setup`, and `/ops`. | core, text, db, engine, discord, modules, ops, telemetry |
| `quack-import` | The one-time MySQL importer (the only crate with sqlx's `mysql`). | core, db |
| `quack-testkit` | The fake `Discord` port, the PostgreSQL test harness, the queue driver. Dev-dependency only. | core, text, engine, db |
| `quack` | Config, CLI, the `DiscordPort` adapter, composition (`compose::App`), `serve` startup and shutdown. | everything |

The dependency rule: dependencies point toward `quack-core`; adapters never
import each other; the engine reaches Discord only through its `Discord`
port (the binary's `DiscordPort` in production, `quack-testkit`'s fake in
tests). Define an interface where it is consumed, and add a trait only when
it has two real implementations. Keep files small and every public item
documented.

`apps/dashboard` is the Vite+ React dashboard; `quack serve` serves its
built files. Its README has the frontend conventions.

## Rust style

- `cargo fmt`; clippy `all` + `pedantic` with the workspace's short allow
  list; no `unwrap`/`expect`/`panic!` outside tests; `unsafe` is
  forbidden.
- Every crate has a crate doc and every public item a doc comment that
  says what it is for; keep comments short, human, and accurate.
- Libraries use `thiserror` enums; `anyhow` only in the binary and tests.
  User-visible failures are typed variants mapped to `quack-text`.

## Commits

- Conventional prefixes (`feat:`, `fix:`, `test:`, `docs:`, `chore:`,
  `refactor:`, `build:`).
- Never add a `Co-Authored-By` trailer or any other attribution.
- Docs changes ship in the same commit as the code they describe.
- Don't commit generated binaries or temporary files; preserve unrelated
  user changes. Stage only your own paths (`git add <path>`, never `-A`).
- Turn the hooks on once per clone: `git config core.hooksPath .githooks`.
  They check formatting and the commit message on commit, and run clippy
  and the dashboard's `check` on push for what the push changes. Fix what
  they report; don't pass `--no-verify`.

## Verification

Tests need PostgreSQL 18. `docker compose up -d` starts one on port 54418.
`QUACK_TEST_DATABASE_URL` must name the server: the tests create and drop
databases there and never fall back to a default. Run from the repository
root:

```sh
export QUACK_TEST_DATABASE_URL=postgres://quack:quack@127.0.0.1:54418/quack
export DATABASE_URL=$QUACK_TEST_DATABASE_URL
export SQLX_OFFLINE=true

cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked          # or the narrowest -p <crate> first
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
cargo deny check
```

- The importer's end-to-end tests also need a scratch MySQL 8.4:
  `QUACK_IMPORT_TEST_MYSQL_URL=mysql://root:root@127.0.0.1:3306` and
  `QUACK_IMPORT_TEST_PG_URL=$QUACK_TEST_DATABASE_URL`. Without them they
  pass without checking anything (each prints `SKIPPED` on stderr; run
  with `--nocapture` to see it), and with `CI` set they fail instead.
- After changing a query: `cargo sqlx prepare --workspace -- --all-targets`
  (sqlx-cli 0.9.0) with `DATABASE_URL` pointing at a migrated database, and
  commit `.sqlx/`.
- After changing an API route or wire type:
  `UPDATE_OPENAPI=1 cargo test -p quack-api --test openapi openapi_is_current`,
  then `bun run api` in `apps/dashboard`, and commit both.
- After changing the instrument catalog:
  `UPDATE_METRICS_DOC=1 cargo test -p quack-telemetry metrics_doc`.
- Text changes: review the `insta` snapshots (`cargo insta review`); list a
  new message id in the design's catalog (§12) and give it a sample: the
  coverage test fails when an id named in §12 has no sample (it doesn't
  find ids missing from §12).
- Dashboard changes: `bun run check`, `bun run test`, and `bun run build`
  in `apps/dashboard`.
- The whole system: `cargo test -p quack --test system` (the composed
  application over PostgreSQL and the fake Discord); `quack serve
  --discord-offline <url>` starts it without a bot token or any call to
  Discord.

CI (`.github/workflows/ci.yml`) runs the same commands in parallel jobs and
skips the jobs a pull request doesn't touch; `ok` is the required check.
When you add a top-level path that a job depends on, add it to that job's
filter in the `changes` job.

## tmux and Docker

Existing tmux sessions, windows, and panes belong to the user. Check for an
already-running server or watcher before starting one. Never kill, restart,
interrupt, or send input to a tmux process, or change layouts or config,
without explicit permission. The same goes for running Docker containers:
start your own, uniquely named, and remove them when done.
