# Quack dashboard

The web dashboard for Quack: a React single-page app built with
[Vite+](https://viteplus.dev) and styled with CSS Modules.

- Staff moderate their servers: cases, appeals, failed actions, the
  moderation log, rules, settings, and the optional modules (tickets, the
  server activity log, the honeypot).
- Members sign in to see their own cases (Your cases, `/cases`) and appeal
  them. Quack's DMs link to `/guilds/{guildId}/cases/{caseId}/appeal`.

It's built to sit comfortably next to Discord without copying it: a calm
dark palette with Quack's duck yellow as the accent, rounded surfaces, quick
motion, and Quack's own Signals icon set (`assets/icons/quack`), the same
icons the bot uses in its messages.

## How it runs

```
browser ──> quack serve (:8080) ── /api/*, /livez, /readyz: the HTTP API
                 │
                 └── everything else: the built app from QUACK_DASHBOARD_DIR,
                     falling back to index.html (design §14.10)
```

There is no separate dashboard server. `quack serve` serves the built app and
the API from one origin, so the host-only session cookie and the CSRF and
Origin checks just work and no CORS is involved. In development `vp dev`
serves the app and proxies `/api`, `/livez`, and `/readyz` to the backend.

## Development

You need [Bun](https://bun.sh) (or npm). Start the backend first (see
[`docs/design.md`](../../docs/design.md), §20.2), then:

```sh
bun install
bun run dev        # http://localhost:3000, proxies /api to localhost:8080
```

Set `QUACK_DASHBOARD_API_URL` to proxy somewhere else. Sign-in goes through
`/api/auth/login`, and Discord redirects to `<QUACK_PUBLIC_URL>/api/auth/callback`,
so point the backend's public URL at the origin you open in the browser.

## Commands

| Command         | What it does                                                                                                                   |
| --------------- | ------------------------------------------------------------------------------------------------------------------------------ |
| `bun run dev`   | Dev server with hot reload.                                                                                                    |
| `bun run build` | Production build into `dist/app`; point `QUACK_DASHBOARD_DIR` at it.                                                           |
| `bun run check` | Format check, lint (Oxlint, type-aware), and type check. `vp check --fix` formats.                                             |
| `bun run test`  | Unit tests (Vitest), including the vocabulary lint over user-facing text.                                                      |
| `bun run api`   | Regenerate `src/api/schema.gen.ts` from `contracts/http/openapi.json`. Run it whenever the backend's OpenAPI document changes. |

## Layout

```
src/
  main.tsx          QueryClient, router, global error handling
  api/              typed client (openapi-fetch), query options, directory lookups
  routes/           TanStack Router file routes (routeTree.gen.ts is generated)
  features/         screens' building blocks, grouped by domain
  ui/               design system primitives (Button, Field, Dialog, DataTable…)
  lib/              pure helpers (formatting, escalation preview, permissions)
  styles/           tokens.css (colors, radii, motion) and global.css
```

### Conventions

- **Data.** Every request goes through `api` in `src/api/client.ts`, typed
  from the generated contract (paths start with `/api`). Wrap reads in
  `unwrap()` so failures become `ApiError`. Send writes with `write()`: it
  makes one `Idempotency-Key` per write the reader asked for and reuses it on
  automatic retries (network errors, 502, 503), so a retry never applies a
  change twice; the client adds `X-CSRF-Token` from `GET /api/auth/me`.
  File uploads use `upload()`, the same write over `XMLHttpRequest` so the
  page can show how much has been sent. Reads
  are `queryOptions` factories in `src/api/queries.ts` keyed under `keys`, so
  a write can invalidate a whole area (`keys.cases(guildId)`). Route loaders
  call `ensureQueryData` so navigation (and hover preloading) has data
  before render; components read with `useSuspenseQuery` or `useQuery`.
- **Errors.** The API answers every failure with one envelope (`code`,
  `message`, `fields`, `problems`, `trace_id`). Show `message` as is; it is
  already a sentence for the reader. Map `fields` onto form fields with
  `ApiError.field(path)`. `ErrorState`, `InlineError`, and toasts add the
  trace ID in small print for unexpected (network or 5xx) failures. A 401
  sends the reader to sign in again.
- **Lists.** Lists page by cursor (`next_cursor`, no totals or offsets).
  Tables use `useCursorPager` with `Pager` for previous and next; feeds use
  infinite queries with `nextPageParam` and a "Load more" button.
- **Users and channels.** The API returns Discord IDs. Show people with
  `UserChip`/`Mention` (`features/people/User.tsx`), which batch lookups into
  one request per tick. Pick people with `MemberPicker`.
- **Capabilities.** `useCan(guildId)` reads the four capabilities
  (`moderate`, `manage_rules`, `manage_settings`, `manage_staff_roles`) from
  `GET /api/guilds/{id}/me`. It only hides what the reader can't do; the API
  checks again on every request.
- **Styling.** CSS Modules: each component has a `Name.module.css` next to
  it, imported as `s` and applied with `className={cx(s.a, cond && s.b)}`
  (`~/lib/cx`). Use the custom properties in `src/styles/tokens.css` for
  every color, radius, shadow, and duration; never hard-code a palette
  color. Prefer state attributes (`data-active`, `aria-current`,
  `aria-checked`) over extra classes, and animate with transitions or the
  shared keyframes in `global.css` (`fade-in`, `rise-in`, `spin`,
  `shimmer`). Inline `style` is fine for truly dynamic values (sizes,
  percentages). Shared primitives take a `className` to be adjusted.
- **Motion.** Things that appear should animate in, and things that leave
  should animate out (`usePresence` in `~/lib/usePresence` keeps an element
  mounted for its exit animation). Keep it quick: 120–260ms.
- **Copy.** Short, plain, and specific, like Quack's Discord messages. Say
  what happened and what the reader can do; a failure never hides a success.
  Use the glossary in `docs/design.md` §1.3 and nothing else: rule (never
  template or policy), level, case count, case, action, outcome, void,
  archive, DM (never notification), case context, evidence, appeal (accept,
  reject, request more information, close, reopen), moderation log (never
  audit log), log channel, server activity log, member (never user),
  moderator, rules manager, administrator, server. Where the API sends a
  sentence (`outcome_text`, `state_text`, `text`, `dm_text`), show it as
  given. A vocabulary test in `src` fails on the banned words.
- **Mutations.** Show errors inline in dialogs (`onError`); anything else
  falls back to a toast from the global `MutationCache` handler. Confirm
  consequential actions with `ConfirmDialog`.
- **Tests.** Keep logic you test in plain `.ts` files (`draft.ts`,
  `decisions.ts`, `format.ts`) and import them from the components.
