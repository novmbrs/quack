# Quack

**Consistent, customizable moderation for Discord.**

> Admins define the server's moderation rules. Moderators apply those rules.
> Quack chooses and carries out the configured result.

Instead of moderators picking `/warn`, `/timeout`, or `/ban` and guessing at
durations, rules managers describe each kind of problem once as a **rule**
with escalation levels. A moderator picks the rule that was broken, and
Quack counts the member's cases under it, chooses the level, records the
case, carries out its action, and sends the member one DM, the same way
every time.

## Features

- **Rules with escalation.** One rule per kind of problem (spam,
  harassment, advertising). Levels start at a case count, with an optional
  counting window, and each level runs at most one timeout, kick, or ban.
- **Cases that stay honest.** Every case keeps the rule version, reason,
  context, and evidence it used. Mistakes are voided, never deleted, and a
  void undoes its timeout or ban.
- **Reliable actions.** Every Discord action is idempotent and retried
  automatically; nothing is punished twice, and nothing is lost on a
  restart.
- **Evidence.** Add a case from a message, or paste links; Quack saves the
  messages and copies their files to a staff-only channel.
- **Appeals.** Members sign in with Discord to see their cases and appeal.
  Accepting an appeal voids the case.
- **Moderation log.** An append-only record of who did what, also posted to
  a log channel.
- **Optional modules.** Tickets, the server activity log, and a honeypot,
  each turned on per server from `/setup` or the dashboard.

## Repository

| Path | What it is |
| --- | --- |
| `crates/` | The Rust workspace. One `quack` binary runs the gateway, interactions, the HTTP API, the dashboard's files, and the job workers. See [`AGENTS.md`](AGENTS.md) for the crate map. |
| `apps/dashboard` | The Vite+ React dashboard, served by `quack serve`. |
| `contracts/http/openapi.json` | The HTTP API contract, generated from `quack-api`. |
| `docs/design.md` | How the whole system works. The source of truth. |

## Quick start

With Docker and a Discord application:

```sh
cp .env.example .env                 # fill in QUACK_DISCORD_* and QUACK_SESSION_SECRET
docker compose --profile app up -d   # PostgreSQL 18 and Quack on http://localhost:8080
```

Add `<QUACK_PUBLIC_URL>/api/auth/callback` to the Discord application's
OAuth2 redirects. Add the `telemetry` profile for an OpenTelemetry backend
with Grafana on <http://localhost:3001> (set
`QUACK_OTLP_ENDPOINT=http://telemetry:4318` in `.env`).

For development, run only the database in Docker:

```sh
docker compose up -d                                  # PostgreSQL 18 on :54418
(cd apps/dashboard && bun install && bun run build)   # the dashboard's files
set -a; . ./.env; set +a
cargo run -p quack -- serve --migrate
```

`bun run dev` in `apps/dashboard` serves the dashboard with live reload on
<http://localhost:3000> and proxies `/api` to `localhost:8080`.

## Configuration

Quack reads `QUACK_*` environment variables, or a TOML file with the same
names (`quack serve --config quack.toml`). [`.env.example`](.env.example)
lists them; [`docs/design.md` §17.7](docs/design.md#177-configuration)
documents each one.

## Commands

```text
quack serve [--migrate]          the bot, the API, the dashboard, and the workers
quack migrate                    apply database migrations
quack import-mysql [--dry-run]   one-time import from the old MySQL database
quack commands sync              register the slash commands
```

## Development

```sh
docker compose up -d
export QUACK_TEST_DATABASE_URL=postgres://quack:quack@127.0.0.1:54418/quack SQLX_OFFLINE=true
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cd apps/dashboard && bun run check && bun run test && bun run build
```

[`AGENTS.md`](AGENTS.md) has the full list (sqlx offline data, the OpenAPI
document and the dashboard's client, snapshots, the importer's tests).

## Documentation

- [`docs/design.md`](docs/design.md): product rules, architecture,
  configuration, testing, migrations, and operations.
- [`crates/quack-telemetry/METRICS.md`](crates/quack-telemetry/METRICS.md):
  every metric, its attributes, and where it is recorded.
- [`AGENTS.md`](AGENTS.md): how to work in this repository.

## License

[MIT](LICENSE)
