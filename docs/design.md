# Quack design

This document is the source of truth for Quack: what it does, how the Rust
code does it, and how it is configured, operated, and migrated from the Go
version. It describes the system as built. When code and this document
disagree, one of them is wrong: fix the code, or change this document in the
same commit.

It does not repeat what the code already states precisely. Look there for:

| What | Where |
| --- | --- |
| Table definitions | `crates/quack-db/migrations/` |
| Message wording | `crates/quack-text` and its `insta` snapshots |
| Request and response fields | `contracts/http/openapi.json` |
| Metric names, units, attributes, buckets | `crates/quack-telemetry/METRICS.md` |
| Crate public surfaces | `cargo doc --workspace --no-deps` |

**Must** and **never** mark requirements; **may** marks a permitted option.
Every limit is a number in §3.9.

## Contents

1. [What Quack is](#1-what-quack-is)
2. [People and permissions](#2-people-and-permissions)
3. [Rules and escalation](#3-rules-and-escalation)
4. [Cases](#4-cases)
5. [Actions](#5-actions)
6. [DMs](#6-dms)
7. [Evidence](#7-evidence)
8. [Appeals](#8-appeals)
9. [Moderation log](#9-moderation-log)
10. [Tracked messages](#10-tracked-messages)
11. [Discord surface](#11-discord-surface)
12. [Message catalog](#12-message-catalog)
13. [Settings and optional modules](#13-settings-and-optional-modules)
14. [HTTP API](#14-http-api)
15. [Database schema](#15-database-schema)
16. [Job queue](#16-job-queue)
17. [Architecture](#17-architecture)
18. [Telemetry](#18-telemetry)
19. [MySQL to PostgreSQL import](#19-mysql-to-postgresql-import)
20. [Testing, development, operations, and decisions](#20-testing-development-operations-and-decisions)

---

## 1. What Quack is

Quack is a Discord moderation bot with a web dashboard. Its one rule:

> Rules managers define the server's moderation rules. Moderators apply those
> rules. Quack chooses and carries out the configured result.

A moderator applies a **rule** (one per kind of problem, never per
punishment) to a member; Quack counts the member's cases for it, picks the
**level**, records the **case**, runs its single **action**, sends at most
one **DM**, and writes the **moderation log**. Members can **appeal**.
Discord and the dashboard behave the same; Discord is the authority for
identity, roles, and permissions.

### 1.1 Principles

1. **The server owns its rules**; Quack ships one editable starter rule.
2. **Moderators apply rules**: never choosing the level, reason, or action.
3. **Discord is the authority for access**: no Quack roles or staff accounts.
4. **History stays understandable**: a case keeps its immutable rule version
   and decision.
5. **Server data stays inside the server**, escalation included.
6. **Records are kept**: cases are voided, never deleted; the moderation log
   is append-only, enforced by the database.
7. **Every external effect has a durable, terminal outcome.** Effects are
   recorded as started before Discord is called and a retry first reads
   Discord's state, so a lost result is not applied twice (the one
   exception is in §20.4).
8. **One lock point per record**: the case row (§4.12). A job writes only
   while the row is still in the state it read (§16.1).

### 1.2 Firm boundaries

Not part of Quack: cross-server history; public APIs; moderator-chosen
levels; direct punishment commands; reason overrides; several actions per
level; severity points; escalation across rules; private staff notes; hard
deletion; Quack-managed roles; rule editing in Discord.

### 1.3 Glossary

The only words allowed in user-facing text and public APIs; code identifiers
follow them too.

| Term | Meaning | Never say |
| --- | --- | --- |
| **Rule** | A server-defined kind of problem with escalation levels. | template, policy |
| **Level** | One rung of a rule: starts at a **case count**; optional action; DM setting. | step, tier |
| **Case count** | Counted cases for one member and rule, including the new one. | offense count |
| **Case**, **case #N** | A rule applied to a member. No action means a warning. | infraction, record, incident |
| **Action** | **timeout**, **kick**, **ban**; undo actions **remove timeout**, **unban**. | punishment, enforcement, execution |
| **Outcome** | What happened to an action or DM: in progress, done, failed, couldn't confirm. | status code names |
| **Attempt** | One try at carrying out an action. | |
| **Void** | Keep the case but stop counting it; undo its timeout or ban. | delete, remove, revoke |
| **Archive** (a rule) | Keep it but stop new use. | disable, delete |
| **DM** | The one direct message about a case or an appeal update. | notification, notice |
| **Case context** | Answers to a rule's extra fields; visible to the member. | metadata |
| **Evidence** | Saved message snapshots and files; staff-only except in the member's own case view. | attachment (for snapshots) |
| **Appeal** | A member's request to reconsider: **accept**, **reject**, **request more information**, **close**, **reopen**. | decline, deny |
| **Moderation log** | Quack's append-only record of who did what. | audit log (UI), audit mirror |
| **Log channel** | Where moderation log entries are posted. | mirror |
| **Server activity log** | The optional module posting Discord events. | general logging |
| **Member** | The person a case is about; "you" in member text. | user, target, offender |
| **Moderator**, **rules manager**, **administrator** | The staff roles; **staff** means all. | mod, admin (alone) |
| **Server** | A guild, in user text; code and API say `guild`. | |

Someone not discussed as a server member (e.g. the dashboard user) is an
**account** (`account_id`).

### 1.4 Message style

All human-read text lives in `quack-text` (§12):

- Plain sentences: what happened, then what to do. Name the actor honestly
  ("Quack couldn't …", "Discord didn't let Quack …"); no "request",
  "resource", "execution", "delivery updated".
- A failure never hides a success: "Case #12 is saved. Quack couldn't send
  @Sam a DM because their DMs are closed. They can still see the case by
  signing in to the dashboard."
- **couldn't** = Discord said no; **couldn't confirm** = unknown.
- One fact, one sentence on every surface. Machine codes stay in structured
  fields (`Action.discord_code`, API `error.code`).
- Staff-only information is ephemeral; only a non-private case receipt is
  public, with fixed fields (§11.7).

### 1.5 Surfaces

| Surface | Used by | What it does |
| --- | --- | --- |
| Discord commands and buttons | Staff, members | Apply rules, view and void cases, review appeals, `/setup`, appeal from a DM. |
| Dashboard (`/`, served by `quack`) | Staff, members | Everything in Discord plus rules, settings, statistics, the moderation log, modules, and the member's own cases and appeals. |
| HTTP API (`/api`) | The dashboard only | Not a public API. |
| Log channel | Staff | Selected moderation log entries. |
| DMs | Members | One per case (if the level says so), per appeal decision, per closed ticket. |


## 2. People and permissions

Staff operations are authorized from current facts before any read or
write; a lost role counts from the next operation.

| Fact | Source |
| --- | --- |
| Actor in an interaction | The payload (computed by Discord at the click). Every click and modal submit is re-authorized; a custom ID never carries authority. |
| Actor on the dashboard | Member cache while the gateway is connected and the server loaded since the last (re)connect, else REST. "Not a member" is never cached (§11.10). |
| Server facts (owner, roles, permissions, MFA level, overwrites) | Guild cache under the same condition, else REST. |
| Staff role settings, 2FA (§2.2) | Read inside the write's transaction. |

Channel permissions come from one function,
`quack_core::permissions::in_channel`: server permissions, @everyone, role,
and member overwrites, then the timeout restriction. A thread uses its
parent's overwrites and Send Messages in Threads; an unknown parent gives no
permissions. Queued work (an action, a DM) is authorized at commit; the job
checks only Quack's own access (§5.4).

### 2.1 Capabilities

`quack_core::permissions` derives `Capabilities` from facts (permission bits,
ownership, roles, `moderator_role_ids` and `rules_manager_role_ids` minus
deleted roles, MFA requirement, recorded 2FA); nothing else grants access.

| Capability | Who has it | Grants |
| --- | --- | --- |
| `moderate` | Owner; Administrator; a configured moderator role; if none of those roles still exists, Moderate Members. | Apply rules, view cases and history, void, edit context, add evidence, retry/dismiss/undo, review appeals, moderation log and statistics, tickets as staff, list rules. |
| `manage_rules` | Owner; Administrator; Manage Server; a configured rules manager role (no fallback). | Create, edit, archive, restore, import, export, list rules. |
| `manage_settings` | Owner; Administrator; Manage Server. | Settings except moderator roles; `/setup`; modules; server health; view moderator roles. |
| `manage_staff_roles` | Owner; Administrator. | Change `moderator_role_ids` (resending the same set is no change). |
| (member self) | The case's member, signed in or clicking their DM. | Own cases and evidence, own appeal, own ticket; works after leaving or a ban. |

Capabilities combine. Staff-only destinations and the honeypot exemption use
the **staff destination predicate** (owner, Administrator, Moderate Members,
or a moderator role; no 2FA). Only `/setup` sets a default permission.

### 2.2 The 2FA rule

When `guilds.mfa_required`, every staff capability (owner included) also
needs the actor's recorded 2FA: `mfa_enabled` from `GET /users/@me` at each
dashboard sign-in, stored in `discord_accounts` (`mfa_checked_at`). Never
seen means no 2FA; the last sign-in holds. Member self operations are
exempt. Refusal: `error.mfa_required`.

### 2.3 Member checks

`quack_core::permissions::check_member(facts, purpose)` is pure; checks run
in order and the first failure wins. `ApplyRule`: 1–8, then 9–10 once the
level is known (§4.2 step 13). `ApplyHoneypotRule`: 1, 3, 5, 6, 8, then
9–10. `RetryAction(kind)` (§5.8): 2, 4, 6, 7, plus 3 for a timeout; a remove
timeout only 7 (member present); an unban none. `RequestUndo(kind)`: as
`RetryAction` for the undo kinds.

| # | Check | Refusal (`MemberRefusal`) | Quack's access? |
| --- | --- | --- | --- |
| 1 | Quack is in the server. | `QuackNotInServer` | yes |
| 2 | The actor is in the server (not for Quack's work). | `ActorNotInServer` | no |
| 3 | The member is in the server (where required, §2.4). | `MemberNotInServer` | no |
| 4 | The member is not the actor. | `MemberIsSelf` | no |
| 5 | The member is not a bot (Quack included). | `MemberIsBot` | no |
| 6 | The member is not the owner. | `MemberIsOwner` | no |
| 7 | Member's top role strictly below the actor's (unless owner); ties refuse. | `MemberRoleTooHigh` | no |
| 8 | Member's top role strictly below Quack's. | `QuackRoleTooLow` | yes |
| 9 | Quack has Moderate Members (timeout, remove timeout), Kick Members, or Ban Members (ban, unban); Administrator covers all. | `QuackMissingPermission(Permission)` | yes |
| 10 | Timeout: the member lacks Administrator. | `MemberIsAdministrator` | no |

Checks 7–8 apply to warnings too; an absent member skips role checks.
Refusals caused by Quack's access are logged (`case_create_refused`); others
never are.

### 2.4 Operations and their checks

Every check on stored state runs inside the changing transaction, after the
§4.12 lock.

| Operation | Capability | Extra checks |
| --- | --- | --- |
| Apply a rule (`/case add`, "Add case", dashboard) | `moderate` + 2FA | Rule active under lock (§4.2); member checks 1–10, 3 required. |
| Apply the honeypot rule | none (Quack acts) | Member checks 1, 3, 5, 6, 8, 9, 10. |
| View/list cases, history, evidence; edit context, add evidence (valid or voided); moderation log, statistics | `moderate` + 2FA | none |
| Void a case | `moderate` + 2FA | Case valid. |
| Retry, dismiss, undo an action | `moderate` + 2FA | `quack_core::action::controls` (§5.8), in the transaction; also picks the buttons shown. |
| Appeal decisions (accept, reject, request info, close, reopen) | `moderate` + 2FA | Transition allowed (§8.2); `expected_version` matches; case valid. |
| Submit or reply to an appeal | member self | §8.1; case valid. |
| Rule create, edit, archive, restore, import, export | `manage_rules` + 2FA | §3. |
| Read settings; server health | `manage_settings` + 2FA | none |
| Change settings | `manage_settings` + 2FA | `manage_staff_roles` to change `moderator_role_ids`; channel checks (§13.1.3). |
| `/setup`, module settings, enable/disable | `manage_settings` + 2FA | Module checks (§13). |
| Open own ticket | server member | Tickets on; none of theirs opening, open, or closing. |
| Close a ticket; fix ticket access | its member (close only), or `moderate` + 2FA | Ticket `open`. |


## 3. Rules and escalation

### 3.1 Storage model

`rules` holds identity (`current_version`, name, `archived_at`, editors);
`rule_versions` holds each version's JSON `definition`, immutable (triggers
reject UPDATE, DELETE, TRUNCATE).

An edit locks the rule `FOR UPDATE`, checks `expected_version` (else
`409 rule_changed`), and inserts `current_version + 1`. Archive and restore
change only `archived_at`; there is no delete, draft, or enable switch. Case
creation locks the rule `FOR SHARE` (§4.2 step 10); caches never decide the
version a case uses. Create, import, and restore lock `guild_settings` first
(the rule limit); archive and edit lock `honeypot_settings` `FOR SHARE`
before the rule (§3.3, §3.5).

### 3.2 The rule definition document

`quack_core::rule::RuleDefinition`: the `definition` column, the API body,
and the export file's `rule`.

| Field | Meaning |
| --- | --- |
| `name` | Unique (case-insensitive, trimmed) among active rules. |
| `description` | Staff-facing; may be empty; cut to 100 characters in pickers. |
| `reason` | The official reason the member sees. No placeholders. |
| `appealable` | Whether this version's cases can be appealed. |
| `counting_window_days` | null = all past cases; else the last N days. |
| `fields` | Context fields in order: `key`, `label`, `kind` (`short_text`, `long_text`, `boolean`, `number`, `message_link`), `required`. With none, `context_fields()` returns one optional `long_text` field `details`, never stored. |
| `levels` | Ascending `from_case_count`, first 1. `action`: null (warning), `{"kind":"timeout","duration_seconds":N}`, `{"kind":"kick"}`, `{"kind":"ban","delete_message_seconds":N}`. `send_dm`: bool. |

### 3.3 Validation

`RuleDefinition::validate(Strictness)` runs before every write and reports
all problems. Text is trimmed; characters are Unicode scalar values.

| Field | Rule | Problem code |
| --- | --- | --- |
| `name` | 1–100 characters; unique among active rules (database). | `name_length`, `rule_name_taken` (409) |
| `description` | 0–500 characters. | `description_length` |
| `reason` | 1–1,000 characters. | `reason_length` |
| `counting_window_days` | null or 1–36,500. | `counting_window` |
| `fields` | 0–10. | `too_many_fields` |
| `fields[].key` | `^[a-z0-9][a-z0-9_-]{1,63}$`, unique. | `field_key`, `field_key_duplicate` |
| `fields[].label` | 1–100 characters. | `field_label_length` |
| `levels` | 1–10; first exactly 1; then 2–1,000,000, strictly increasing. | `level_count`, `first_level_not_one`, `level_case_count`, `level_order` |
| timeout `duration_seconds` | 1–2,419,200 (28 days). | `timeout_duration` |
| ban `delete_message_seconds` | 0–604,800 (7 days). | `ban_delete_window` |
| honeypot rule | No required field while the honeypot uses it. | `required_field_on_honeypot_rule` (409) |
| active rules | At most 100 (create, import, restore). | `too_many_rules` (409) |

`Strictness::New` (everything written through Quack) applies every row;
restore re-checks only name and count. `Strictness::Imported
{ reconstructed }` (importer only, §19.5) checks structure without size
limits, and a first level of 1 unless `reconstructed`. A legacy rule over a
limit keeps working; its first edit must fix it.

### 3.4 Choosing the level

`quack_core::escalation::select_level(def, case_count)` is pure.
`case_count` is the counted cases plus one: same server, member, and
`rule_id` (any version), not voided, not `imported`, and inside the applied
version's counting window if any. The level is the one with the highest
`from_case_count` ≤ `case_count` (below all levels, only possible in a
reconstructed version, the first). `None` only for a level-less definition,
treated as an internal error. Escalation never looks at other rules, other
servers, weights, or the moderator.

### 3.5 Archive and restore

Archive hides the rule from pickers (`409 rule_used_by_honeypot` while the
honeypot names it, §13.4.1); restore can be `rule_name_taken` or
`too_many_rules`. Both are logged and reload the rules cache (§17.6).

### 3.6 Import and export

Export: `{"format": "quack-rule", "format_version": 1, "exported_at", "rule"}`,
no IDs, cases, or settings. Import takes it with `"confirm": true`, validates
as new, and creates version 1 (`409 rule_name_taken` on a clash). Import is
logged (`rule_imported`); export is not.

### 3.7 The starter rule

Bootstrap's first `guild_settings` row (§17.5) comes with
`quack_core::rule::starter_rule()` ("General rule violation": warning at 1,
1-day timeout at 3, ban deleting 1 day at 5, all with a DM), stored as
`starter_rule_id`. The dashboard asks for review until someone with
`manage_settings` acknowledges it. Never created twice.

### 3.8 The honeypot rule

Turning the honeypot on in `/setup` with no rule set creates
`quack_core::rule::honeypot_rule()` ("Honeypot": ban at 1, no message
deletion, with a DM). An existing active rule named "Honeypot" is never
adopted: the turn-on is refused (`module.problem.honeypot_rule_missing`).

### 3.9 Limits

Constants in `quack_core::limits`, checked before every write. CHECK
constraints mirror a limit only where imported data cannot exceed it; legacy
values are imported as they were and reported (§19.10).

| Limit | Value | Where enforced |
| --- | --- | --- |
| Rule fields (name, description, reason, fields, key, label, levels, timeout, ban window, counting window, active rules) | as in the §3.3 table | §3.3 |
| Context `short_text` / `long_text` / free-text `details` | 1–500 / 1–4,000 / 1–4,000 characters | §4.6 |
| Context `message_link` | input ≤ 4,000 characters, stored canonical | §4.6 |
| Context `number` | absolute value ≤ 10^15, ≤ 6 decimals, exact | §4.6 |
| Case number | ≤ 2^53 − 1 | §4.4 |
| Void reason / appeal text / decision reason | 1–1,000 / 1–4,000 / 1–2,000 characters | §4.7, §8.1, §8.2 |
| Evidence items / files per case / links per call | 50 / 100 / 10 | §7.1 |
| Per evidence item: text / embeds / files | 4,000 characters / 10 / 10 | §7.2 |
| Largest file Quack copies | 10,485,760 bytes (10 MiB) | §7.3 |
| Moderator roles, rules manager roles | 0–25 each | §13.1 |
| DM introduction / footer | 0–1,000 / 0–500 characters | §13.1 |
| Rejoin invite code | 2–64 of `[A-Za-z0-9_-]` | §13.1 |
| Honeypot warning | 0–1,500 characters | §13.4 |
| Ticket transcript retention / file | 1–365 days (default 90) / 8,388,608 bytes | §13.2 |
| `Idempotency-Key`; API page size | 8–200 printable ASCII; default 50, max 100 | §14.1 |
| Discord list page; case draft lifetime | 10 rows; 15 minutes | §11, §11.6 |
| Discord nonce retry window | 60 s from the first attempt | §10.4 |
| HTTP request body | 262,144 bytes | §14.8 |
| Statistics range | 1 second to 366 days | §14.3 |


## 4. Cases

A case is the outcome itself; its action, DM, evidence, and appeal have
their own outcomes and never change whether it is valid.

### 4.1 Case sources

| `case_source` | Created by | Operation key |
| --- | --- | --- |
| `discord` | `/case add`, "Add case" (message, member) | Interaction ID, or `draft:<draft_id>` (§11.6). |
| `dashboard` | `POST /api/guilds/{guild_id}/cases` | `Idempotency-Key`. |
| `honeypot` | The honeypot module (`moderator_id` NULL) | `honeypot:<first message id>` (§13.4.2). |
| `imported` | The importer (Go's v4 imports) | none; never counted; no rule, action, DM, or appeal. |

### 4.2 Creating a case, end to end

`quack_engine::cases::apply_rule(engine, ApplyRule)` is the only way to
create a case; the caller has already acknowledged the interaction.

**Before the transaction** (no locks; may call Discord):

1. **Authorize**: `moderate` in this server + 2FA (§2). Nothing about the
   key is revealed first.
2. **Replay check** (§4.3): same hash → `Applied::Replayed`; different →
   `OperationKeyReused`.
3. **Early checks** on the rules cache: `RuleNotFound`, `RuleArchived`,
   `ContextInvalid` (§4.6); repeated in step 10.
4. **Evidence sources**, deduplicated: the source message, `message_link`
   values, links inside text answers, explicit links. Explicit and
   `message_link` links must be in this server (`other_server`); links in
   text elsewhere are ignored. Over 50 → `EvidenceLimitReached` (again in
   step 18). Empty `message_link` fields take the source message.
5. **Facts** (`ApplyFacts`) from the `Discord` port, including the member's
   `joined_at`; an error is `DiscordUnavailable`.
6. **Member checks 1–8** (§2.3); a refusal caused by Quack's access is
   logged in its own transaction (`case_create_refused`; honeypot:
   `honeypot_case_failed`).

**One transaction** (READ COMMITTED, §4.12 lock order, retried up to 3 times
on serialization failure or deadlock):

7. Reserve the operation key (a concurrent winner → replay). Step 22's
   deadline starts before this.
8. Staff: re-check `moderate` + 2FA from rows in the transaction. Honeypot:
   `honeypot_settings` `FOR SHARE` must still match (on, `version`,
   channel, rule), else `HoneypotPolicyChanged`.
9. `UPDATE case_counters … RETURNING` takes the number and serializes
   creation per server; its CHECK past the maximum →
   `CaseNumbersExhausted`.
10. Lock the rule `FOR SHARE`: `RuleNotFound`, `RuleArchived`, or
    `RuleChanged` (pinned ≠ current); validate context against it unless
    step 3 did.
11. Count counted cases (§3.4, `cases_escalation` index), plus one.
12. `select_level` (§3.4).
13. With an action: member checks 9–10; a refusal rolls back (9 is logged as
    in step 6).
14. `replaces`: a voided, unreplaced case of this server, else
    `ReplacementInvalid`.
15. Insert the case: decision (§4.4), number, `member_joined_at`, scoped key
    and hash, `context_revision = 1`, `created_at` (captured once).
16. Action `pending`; a timeout's `timeout_ends_at = created_at +
    duration`, whole seconds.
17. DM (`case`, `pending`) if the level sends one.
18. Evidence rows (`saving`).
19. Receipt row (§11.7): private → the token, expiring after 14 minutes;
    public → the channel, `message_id` NULL, and the token for the private
    "posted"/"refused" note (§10.3).
20. Log `case_created`, with `post_mod_log` if a log channel is set.
21. Jobs (§16): `send_dm`; `run_action` (a DM'd kick or ban waits until
    `created_at + 120 s` or the DM settles, §6.3); `capture_evidence` per
    item; `sync_message`. Bump the replaced case's receipt, delete the draft
    (§11.6), complete the key.
22. Commit. 2-second `statement_timeout` and `lock_timeout`; 3-second
    deadline over steps 7–22. Before COMMIT, or none left for it: rolled
    back, `error.internal`. No answer to COMMIT: `error.unconfirmed` (the
    same key returns the case if saved).

**After:**

23. `Applied::Created`. On Discord the receipt's `sync_message` job is now
    the only writer of the interaction's reply (§10.3).
24. `quack.cases.created`, `quack.case.create.duration`, `quack.case.level`
    (refusals: `quack.cases.refused`); span event `case.created`.

### 4.3 Operation keys

Every state-changing command stores its key in `operation_keys` within the
write's transaction, scoped to server, account, and operation.

| Origin | `guild_id` | `account_id` | `operation` | `key` |
| --- | --- | --- | --- | --- |
| Command, button, select, modal | server, or 0 in DMs | clicker | command or custom-ID code (`case_add`, `vdm`, …) | interaction ID |
| `/case add` form | server | moderator | `case_add` | `draft:<draft_id>` |
| Dashboard write | server, or 0 for `/api/me` | signed-in account | `<METHOD> <route pattern>` | `Idempotency-Key` |
| Honeypot | server | 0 | `honeypot_case` | `honeypot:<first message id>` |

- Authorize against current state before looking the key up.
- `request_hash`: SHA-256 of the resolved request's canonical JSON (path IDs
  resolved, query, validated body; or options, custom-ID arguments, modal
  values). Context is sorted by key and tagged by kind (`5` ≠ `"5"`).
- Same hash: render the stored result's **current** state for this actor
  with the original status (`Idempotency-Replayed: true`). Different:
  `409 idempotency_key_reused`.
- `reserve` inserts a placeholder (a concurrent holder blocks, then
  replays); `complete` fills it before commit.
- Rows expire after 24 hours; cases keep theirs forever
  (`cases.operation_key` = `<account_id>:<operation>:<key>`, with
  `request_hash`). A begun HTTP write finishes past the deadline (§14.8).

### 4.4 Case numbers and the decision

`case_counters.last_number` only increments: numbers start at 1 and are
never reused; the importer sets it to the highest import. Past 2^53 − 1 →
`409 case_numbers_exhausted`. The decision never changes (fix = void +
replacement): `rule_id`, `rule_version`, `case_count`, `level_index`
(1-based), `level_from_case_count`, `action_kind` (NULL = warning),
`timeout_seconds`, `ban_delete_message_seconds`, `sends_dm`. The case also
keeps `member_joined_at`; a kick only targets that membership (§5.4).

### 4.5 Case states

| From | Event | To | Effects |
| --- | --- | --- | --- |
| (none) | §4.2 commits | valid | §4.2 steps 15–21. |
| valid | Staff void, or appeal accepted (`appeal_accepted`) | voided | §4.7. |
| voided | Any void | voided | `409 case_already_voided` (same key replays). |

There is no un-void.

### 4.6 Case context validation

`quack_core::case::validate_context` checks answers against
`context_fields()`. Inputs are `Text` (Discord) or, from the API,
`JsonNumber` (original text, never a float) or `Json`. Unknown key →
`unknown_field`; repeated key → `duplicate_field`. Blank optional answers
are absent; `false` and `0` are answers. All problems are reported (§12.5).

| Kind | Accepted | Stored as | Problems |
| --- | --- | --- | --- |
| `short_text` | 1–500 characters. | string | `required`, `too_long`, `not_text` |
| `long_text` | 1–4,000 characters. | string | `required`, `too_long`, `not_text` |
| `boolean` | API `true`/`false`; Discord `yes`/`no`. | bool | `required`, `not_yes_no` |
| `number` | `-`? digits (`.` digits)?; no exponent or `+`; ≤ 10^15, ≤ 6 decimals; exact `Decimal`. | canonical string (`"2.5"`) | `required`, `not_a_number`, `number_out_of_range` |
| `message_link` | `https://discord.com/channels/<guild>/<channel>/<message>` (also `ptb.`, `canary.`, `www.`, `discordapp.com`); this server; IDs < 2^63. | canonical URL, no query | `required`, `not_a_message_link`, `other_server` |

`find_message_links(text)` turns this server's links inside text answers
into evidence sources (§4.2 step 4, §4.9); the text is unchanged.

### 4.7 Voiding

`quack_engine::cases::void_case(principal, case_ref, reason, op_key)`:

1. Authorize (`moderate` + 2FA); reason 1–1,000 characters.
2. One transaction (§4.12 order):
   1. Reserve the operation key.
   2. Lock the case; already voided → `409 case_already_voided`.
   3. Set `voided_at`, `voided_by`, `void_reason`, `void_cause` (`staff`, or
      `appeal_accepted` with `case.void.reason.appeal_accepted`).
   4. Original action: `pending` → `cancelled`; `running` settles to done
      or `cancelled` (§5.3); terminal stays.
   5. **Undo** (`quack_core::action::on_void`, §5.9): a timeout or ban not
      `cancelled`, without an undo, `done` or possibly applied (§5.2), and
      (timeout) still running gets `remove_timeout`/`unban` (`pending`,
      `requested_by` = voider) and its job. It acts only on Discord state
      this case placed (§5.4, §5.6). Kicks are never undone.
   6. A `pending` case DM → `not_needed`.
   7. An open appeal → `closed` (`closed_by_void`) with its DM, unless
      this void is its acceptance.
   8. Bump the receipt and appeal post (§10).
   9. Log `case_voided` (cause, cancelled and undo action IDs).
3. `quack.cases.voided{quack.void.cause}`.

It returns `VoidEffects`, the member, and the rule name. Later voids never
cancel an undo.

### 4.8 Replacement links

A voided case can be replaced once (§4.2 step 14), by a case under any rule
or member. Staff views link both ways ("Replaced by case #13"); member views
only when both cases are theirs.

### 4.9 Edits

On valid or voided cases; both bump the receipt:

- **Edit context**: validated against the case's own version; the request's
  `context_revision` must match under the case lock (else
  `409 context_changed`). New links become evidence within the cap. Log
  `case_context_edited` (changed keys).
- **Add evidence**: 1–10 links (§7.1); saved or saving ones are reported,
  failed ones captured again. Log `evidence_added`.

### 4.10 Reading cases

Staff use a number, UUID, or legacy ULID (§19.3); members their own case by
UUID or ULID. `quack_engine::cases::staff_card` builds every staff card.

| Field | Staff | Member |
| --- | --- | --- |
| Server, number, times, valid/voided and void reason, timeout end, appeal content, member timeline (§4.11) | yes | yes |
| Who voided, who decided the appeal, moderator, source, case count and level, staff timeline | yes | no |
| Rule | name, version, reason | name, reason |
| Action, undo, DM outcomes | with attempts and codes | sentences only |
| Context | all | `message_link` values as `member.context.link_saved` |
| Evidence | everything | text, embed type and title, file names and files; no author, link, channel, or URLs |
| Replacement links | yes | numbers, only when both cases are theirs |

A wrong owner and a missing case are both `404 not_found`.

### 4.11 Member timeline

From immutable rows, oldest first, never naming staff: case added and
voided, attempts ending `done` or `no_change`, each final failure (its
`action_failed` entry), skipped undos (dated by `action_undo_skipped`), the
settled DM, appeal events. The API returns the newest 100 and
`timeline_truncated`.

### 4.12 Locks

Lock order, skipping what isn't needed: 1 `operation_keys` (new row);
2 `guild_settings`; 3 module settings rows; 4 `case_counters`; 5 `rules`
(`FOR SHARE` to create cases, `FOR UPDATE` to change); 6 the member advisory
lock (action starts, §5.4); 7 `cases` (ascending `id`); 8 its `actions`,
`dms`, `appeals`, `evidence`; 9 `messages`; 10 `jobs` (a handler's fence and
settle always last, §16.1).

Each writer re-checks state after its locks; no lock is held across a
Discord call; database-only transactions retry up to 3 times on
serialization failure or deadlock.

## 5. Actions

One **original** (timeout, kick, ban) per case at most, and one **undo**
per original (remove timeout, unban; none for a kick).

### 5.1 Kinds

Every attempt first reads the state it would change over REST (never a
cache) and acts only if the change is still needed.

| `action_kind` | Reads | Then | Permission |
| --- | --- | --- | --- |
| `timeout` | member | Absent → `member_not_in_server`; Administrator → `member_is_administrator` (both `failed`). A timeout ending at or after `timeout_ends_at` → `done` (`no_change`; never shortened). Else PATCH `communication_disabled_until`. | Moderate Members |
| `kick` | member | Absent, or `joined_at` after `cases.member_joined_at` (imported: `created_at`) → `done` (`no_change`). Else DELETE; Unknown Member → `no_change`. | Kick Members |
| `ban` | ban | This case's marker → `done`; another ban → `done` (`no_change`). Else PUT. | Ban Members |
| `remove_timeout`, `unban` | guard, then member / ban | §5.6 | as original |

A ban sends `delete_message_seconds` only when no earlier attempt (any
round) may have applied (§5.2), else 0. Mutating calls send
`X-Audit-Log-Reason`: the **case marker** `Quack case #<n>: <rule name>`
(≤ 512 characters), or `Quack case #<n> voided` for undos
(`parse_case_marker` reads it back).

### 5.2 Idempotency and "may have applied"

`timeout_ends_at` (`created_at + timeout_seconds`, whole seconds) is fixed
at creation. Because every attempt reads first, an attempt with an unknown
result is simply retried. An attempt **may have applied** when it is open
or ended `done`, `unknown`, or `interrupted` (`may_have_applied`); the void
(§4.7), failure reasons (§5.5), ban deletion (§5.1), and the undo (§5.6)
all use this rule.

### 5.3 States

`pending` and `running` show as in progress; `done`; `failed`
(`failure_reason`); `cancelled` (an original whose case was voided);
`skipped`, shown "left in place" (an undo that changed nothing,
`skip_reason`). Transitions (`quack_core::action::transition`; other pairs
are `InvalidTransition`; start and settle rows are tried in order):

| From | Event | To | Side effects (same transaction) |
| --- | --- | --- | --- |
| (none) | Created / undo requested | `pending` | `run_action` job. |
| `pending` | Start: original, case voided | `cancelled` | Bump. |
| `pending` | Start: guard finds another case in force | `skipped` | `other_case_applies`, `skipped_because_case_id`; log `action_undo_skipped`; bump. |
| `pending` | Start: timeout end passed | `failed` | `timeout_window_passed`; log `action_failed`; bump. |
| `pending` | Start: pre-check refuses | `failed` | `failure_reason`; log; bump. |
| `pending` | Start: checks pass | `running` | Open attempt; `attempts + 1`. No bump. |
| `pending` | Case voided (original) | `cancelled` | §4.7. |
| `running` | Settle: `done`, or plain `no_change` | `done` | `no_change`; log `action_done`; bump. |
| `running` | Settle: undo found the effect changed outside the case | `skipped` | `changed_outside_case`; log; bump. |
| `running` | Settle: `refused` (even if voided meanwhile) | `failed` | Reason and details; log; bump. |
| `running` | Settle: original not `done`, case voided | `cancelled` | Bump. |
| `running` | Settle: `not_sent`/`unknown`/`interrupted`, attempts left | `pending` | `next_attempt_at` (§5.7). No bump. |
| `running` | Settle: same, none left | `failed` | `original_unconfirmed` (unresolved original, §5.6), else `unconfirmed` if the round may have applied, else `retries_exhausted`. Log; bump. |
| `failed` | Staff retry | `pending` | Clear failure; `attempts = 0`; job; log `action_retry_requested`; bump. |
| `failed` | Staff dismiss | `failed` | `dismissed_at/by`; log `action_dismissed`; bump. |

`done`, `cancelled`, `skipped` are terminal; `failed` too unless retried.

### 5.4 Running an attempt

`quack_engine::actions::run` gathers Discord facts before any transaction
and opens an attempt only with room for two calls (§16.1), else `Continue`.

1. **Start** (one transaction, §4.12 order):
   1. The member's advisory lock
      (`hashtextextended('member:' || guild || ':' || member, 0)`).
   2. Lock the case, then the action; the fence runs last (§16.1).
   3. Not `pending`/`running` → finish.
   4. An open attempt → settle it `interrupted`; continue only if `pending`
      and due.
   5. `next_attempt_at` in the future → retry then.
   6. Another action of the member `running` → defer 2 s. With 1.1 this
      serializes one member's attempts.
   7. Kick or ban whose DM is `pending`/`sending`, < 120 s after creation →
      defer (§6.3).
   8. Start rows of §5.3: voided original, guard (§5.6), timeout end, then
      `action_precheck`: Quack present, has the permission, member not the
      owner, member's top role below Quack's (member present, not `unban`).
   9. Open the attempt (`running`) and commit.
2. **Read and act** (no locks; 10 s per call): §5.1, §5.6.
3. **Settle** (one transaction): lock case, then action; require `running`
   at this run's attempt number, else roll back. Finish the attempt, apply
   §5.3, log, bump, settle the job; fence last.

Finalization (§16.1) settles an open attempt `interrupted`, then cancels a
voided case's original or fails it (§5.2). Metrics:
`quack.action.attempts{quack.action.kind,quack.action.attempt.outcome,quack.reason}`,
`quack.actions.finished`, `quack.action.completion.duration`,
`quack.action.retries`, `quack.action.undo.skipped`.

### 5.5 Attempt outcomes and failure reasons

| Result | `attempt_outcome` → next |
| --- | --- |
| Call `Done`, or this case's own ban found | `done` → `done` |
| Desired state held; Unknown Member (kick) / Unknown Ban (unban) on the call | `no_change` → `done` |
| Undo found an effect not a case's | `no_change` → `skipped` |
| Undo found nothing, original unresolved | `not_sent` → retry |
| `Refused` | `refused` → `failed` |
| Read failed, never sent, or `RetryAfter(d)` | `not_sent` → retry after `max(d, backoff)` |
| `Unknown` on the call | `unknown` → retry |
| Found open by the next run | `interrupted` → retry |

`action_failure`: `quack_not_in_server` (pre-check, or Unknown Guild
10004), `quack_missing_permission` (with the permission),
`quack_role_too_low`, `member_is_owner`, `member_not_in_server` and
`member_is_administrator` (timeout read; Unknown Member on the PATCH too),
`timeout_window_passed` (end passed before Discord applied it, including a
refused PATCH after it), `discord_refused` (other 4xx except 429, with
status and code), `retries_exhausted` (nothing may have applied),
`unconfirmed` (something may have, or imported in flight, §19.9),
`original_unconfirmed` (§5.6; retryable).

### 5.6 The undo guard and provenance

1. **Guard** (start transaction; `quack_db::actions::guard`, index
   `actions_member`): the newest other valid case of the member whose
   original of the same kind is `done` and (timeout) ends in the future →
   `skipped` (`other_case_applies`, "left in place because case #N still
   applies"). Only `done` counts: an unstarted one applies after the undo,
   one in flight makes it wait (§5.4 step 1.6).
2. **Unresolved original**: the original never reached `done` but an
   attempt may have applied. A read then finding nothing to undo (no ban;
   no timeout while the case's end is in the future) is `not_sent` and
   reads again on the backoff; a spent round is `original_unconfirmed`. An
   absent effect never finishes an undo while the original may still land.
3. **Provenance**:
   - `remove_timeout`: none or ended (Discord's end cut to whole
     seconds) → `no_change`; an end a second or more from the case's →
     `skipped` (`changed_outside_case`); within a second either way →
     PATCH to null. Quack keeps the whole-second end it sent, not the one
     Discord echoes; the second of slack covers Discord keeping it at
     another precision.
   - `unban`: no ban → `no_change`. Marker of this undo's case → remove
     (covers a manual undo of a valid case). Marker of another case →
     remove only if it is a **voided** case of this member with a ban
     (`voided_ban_case`). Otherwise `skipped` (`changed_outside_case`).

### 5.7 Retry policy

8 attempts per round (`MAX_ATTEMPTS`); a staff retry starts a new round
(attempt numbers keep counting). Backoff after attempt 1…7: 5 s, 15 s,
1 min, 5 min, 15 min, 1 h, 3 h (`RETRY_BACKOFF`, 4 h 21 min 20 s).
`RetryAfter(d)` waits `max(d, backoff)`; a defer uses no attempt.

### 5.8 Staff controls

`quack_core::action::controls(&ActionView, case_valid, now)` decides every
control; buttons render from it and each write re-evaluates it under the
case and action locks, refusing with its `Err`.

| Control | Allowed when | Actor checks (fresh facts) |
| --- | --- | --- |
| Retry | `failed`, not dismissed. Original: case valid (`case_voided`), not `timeout_window_passed`, timeout end in the future. Undo: always. | Original: member checks 2, 4, 6, 7 (and 3 for a timeout). Remove timeout: 7 if present. |
| Dismiss | `failed`, not dismissed. | none |
| Undo | `done` timeout or ban, no undo yet, timeout end in the future, and the guard (read under the write's locks) finds no other case in force. | Remove timeout: 7 if present. |

Offered ⇔ the undo lifts it (property-tested). Other refusals: `404 action_not_found`,
`409 action_not_retryable`, `409 action_not_dismissable`,
`409 action_not_undoable`. Buttons name the kind ("Retry ban"); failures
are listed at `/guilds/{guild_id}/failures`.

### 5.9 What a void does at each action state

`quack_core::action::on_void`; undo actions are never touched.

| Original state | On void |
| --- | --- |
| `pending`, nothing may have applied | `cancelled`. |
| `pending`, something may have applied | `cancelled`; undo queued (ban, or timeout ending in the future). |
| `running` | Left to settle; undo queued now, waits (§5.4 step 1.6). |
| `done` (ban, or timeout ending in the future) | Undo queued, also after `no_change`. |
| `done` (timeout ended; kick), `cancelled` | Nothing. |
| `failed` | Stays `failed`; undo queued if something may have applied. |

## 6. DMs

### 6.1 Kinds

`case` (a level with `send_dm`, one per case, §6.6), `appeal_update` (one
per staff decision or void close, §8.4), `ticket_closed` (one per ticket,
with the transcript, §13.2.4). A level without a DM has no row ("not
needed").

### 6.2 States

`pending`, `sending` (channel opened, `first_attempt_at` recorded) show in
progress; `sent`; `failed`; `unknown` ("couldn't confirm", terminal);
`not_needed` (case voided before sending).

| From | Event | To | Side effects |
| --- | --- | --- | --- |
| (none) | Created | `pending` | `send_dm` job. |
| `pending` | Case voided (case DMs) | `not_needed` | Bump. |
| `pending` | Channel opened; case still valid | `sending` | `channel_id`, `first_attempt_at`, committed before any create. |
| `pending` | Open refused, or budget spent | `failed` | `failure`; log `dm_failed`; bump; wake. |
| `sending` | Create `Done` | `sent` | `message_id`; log `dm_sent`; bump; wake. |
| `sending` | Refused, nothing possibly sent | `failed` | As above. |
| `sending` | Window over / `RetryAfter` won't fit, nothing possibly sent | `pending` | Job retried after the delay. |
| `sending` | Window over, refusal, or `RetryAfter` beyond it after a possible send; or budget spent | `unknown` | Log `dm_failed`; bump; wake. |

"Possibly sent" (`dm::SendHistory`): a create answered `Unknown`, or the
run found the DM already `sending`. Quack never reads DM history. "Wake":
move a `pending` kick or ban's job to now (§6.3).

### 6.3 Ordering with kick and ban

A kick's or ban's DM goes first. Its action job runs at `created_at + 120 s`
(`ACTION_WAIT`) or when the DM settles, whichever is first (§5.4 step 1.7);
a DM failure never blocks it. Timeouts and warnings run both at once. DMs
describe the **decision**, not the action's outcome.

### 6.4 Sending

`quack_engine::dms::send`:

1. Load; terminal → done.
2. `pending`: `open_dm` (idempotent). Refused → `failed` (§6.5); otherwise
   retry the job (3 attempts, 5 s, 30 s). Start transaction: lock case,
   then DM; voided → `not_needed`; else `sending`; fence; commit.
3. The window (§10.4) is measured from `first_attempt_at` on the
   database's clock (`dms::send_window_open`) and checked before rendering
   and right before every create, however late the task wakes.
4. Render from the DM's own record; create with nonce = the DM ID.
5. `Done` → `sent`. `Unknown` (possibly sent) or `NotSent` → wait 2 s,
   repeat. `RetryAfter(d)` → wait if it fits. Refused → `failed`, or
   `unknown` if possibly sent. Window over → `unknown` if possibly sent,
   else `pending`.
6. Settle per §6.2 with the job (§16.1).

Metrics: `quack.dm.outcomes{quack.dm.kind,quack.dm.outcome,quack.reason}`,
`quack.dm.send.duration{quack.dm.kind}`.

### 6.5 Failure reasons

`dms_closed` (50007), `not_in_shared_server` (50278), `discord_unavailable`
(budget spent, nothing sent), `discord_refused` (other 4xx). Sentences:
§12.7.

### 6.6 What a case DM contains

`dm.case`: server, rule, quoted reason, intro, context (not links), the
decided action, case number, **Appeal** link or "can't be appealed",
footer. Never evidence or the moderator. DMs are sent **at most once**.

## 7. Evidence

### 7.1 Sources and limits

Sources: the message an "Add case" message command used; `message_link`
answers and links in text answers (§4.6); dashboard links at creation;
"Add evidence" (1–10 links, own transaction). Links must be in this server.
One row per `(case_id, message_id)`: re-adding `saving`/`saved` is "already
saved", `failed` is captured again. At most 50 per case, counted under the
case lock (`422 evidence_limit_reached`, before any write). Each starts
`saving` with job `capture_evidence:<evidence_id>`.

### 7.2 The capture job

`quack_engine::evidence::capture`:

1. Not `saving` → done.
2. Added by staff: they must read the channel by current facts
   (`permissions::in_channel`: View Channel + Read Message History; threads
   use the parent; private threads also need membership or Manage
   Threads). No → `moderator_no_access`; facts unavailable → retry.
3. Fetch: Unknown Message/Channel → `message_deleted`; Missing
   Access/Permissions → `quack_no_access`; other refusal → `discord_error`;
   another server → `other_server` (all `failed`). Unknown/rate limit →
   retry (4 attempts, 10 s, 1 min, 5 min).
4. **Snapshot** (case then evidence lock; stored once, guarded on
   `message_created_at IS NULL`): author, content (4,000), times, ≤ 10
   embeds `{type, title ≤ 256, description ≤ 1,000, url}`, files. Files
   are admitted below 10 per message and 100 per case (rest counted in
   `files_not_saved`) as `link_only` (no evidence channel), `too_large`,
   `unsupported_type`, or `pending`.
5. **Copy** each `pending` file (§7.3), one per transaction.
6. None `pending` → `saved`; bump. Metric
   `quack.evidence.captures{quack.outcome}`.

Finalization: pending files → `copy_failed`; row → `saved` if the snapshot
exists, else `failed` (`discord_error`).

### 7.3 Copying files

- Eligible: ≤ 10,485,760 bytes; type `image/*`, `video/*`, `audio/*`,
  `text/plain`, `application/pdf`. Downloads only from
  `cdn.discordapp.com` / `media.discordapp.net` over HTTPS (≤ 5 redirects
  within them, 30 s), up to 3 per run; another URL or a refusal →
  `copy_failed`.
- Before **every** upload, re-read the evidence channel and check it is
  staff-only (§13.1.3). Unset or gone → the item's pending files
  `copy_failed`; not staff-only → the same plus
  `copy_skipped_not_staff_only`; facts unavailable → retry.
- Each try is reserved durably first (`copy_attempts` under the case and
  evidence locks with the fence; starts `copy_started_at`): 3 per file
  across runs. A rate-limited or unsent upload returns its try (a rate
  limit ends the run); `Unknown` repeats with the same nonce (§10.4).
- Upload `SPOILER_<filename>` alone, nonce = the file ID. `Done` →
  `copied` (copy channel, message, attachment); refused → `copy_failed`.
- A download starts only with room for it, the check, the upload (30 + 10
  + 30 s), and 5 s, else `Continue`. Metrics: `quack.evidence.files`,
  `quack.evidence.bytes`.

### 7.4 Who sees evidence

Staff see everything; the member sees their own case's evidence on the
dashboard minus authors, links, embed URLs, channels. Files `302` to a
fresh CDN URL via the copy, else the source (`evidence::resolve_file`;
else `404 file_unavailable`). Never in DMs, receipts, or the log channel.

## 8. Appeals

### 8.1 Eligibility

The account (signed in, or clicking the DM button) is the case's member;
the case is valid and not imported; its rule version is appealable; no
appeal exists. Checked under the case lock. One statement, 1–4,000
characters. Refusals: `409 appeal_exists`, `409 case_voided`,
`422 appeal_not_allowed`.

### 8.2 States

`pending`, `needs_information`, `accepted` (case voided; terminal),
`rejected`, `closed` (`closed_by_void` is terminal). Each transition locks
the case, then the appeal, and checks validity, state, and
`expected_version` (mismatch `409 appeal_changed`).

| From | Event | Actor | To | Effects |
| --- | --- | --- | --- | --- |
| (none) | `submitted` | member | `pending` | Appeal; event; `appeal_post` (§8.3); log; bump receipt. |
| `pending` | `information_requested` | staff | `needs_information` | Event; DM; bump post; log. |
| `needs_information` | `information_provided` | member, 1–4,000 | `pending` | Event; bump post; log. |
| `pending` | `accepted` | staff | `accepted` | Event; void (§4.7, `appeal_accepted`); DM; bump post, receipt; log. |
| `pending` | `rejected` | staff | `rejected` | Event; DM; bump post; log. |
| `pending`, `needs_information` | `closed` | staff | `closed` | Event; DM; bump post; log. |
| `pending`, `needs_information` | `closed` | Quack, in a void | `closed` | `closed_by_void`; DM; bump post; log (actor NULL). |
| `rejected`, `closed` (not by void) | `reopened` | staff; case valid | `needs_information` | Event; DM; bump post; log. |
| other | any | | unchanged | `409 appeal_changed` / `case_voided`. |

Staff events need a reason (1–2,000). Metrics:
`quack.appeal.transitions{quack.appeal.transition}`,
`quack.appeal.decision.duration{quack.appeal.transition}`,
`quack.appeals.open`.

### 8.3 The appeal post

One tracked `appeal_post` (§10) per appeal in `appeal_channel_id`.
Setting the channel binds open appeals' channel-less or `gone` posts;
changing it moves them (§10.3); clearing leaves them. **Accept**,
**Reject** (while `pending`), **Open appeal** re-check capability and
version and answer privately.

### 8.4 Appeal DMs

One `appeal_update` DM per decision or void close, rendered from its own
event: decision, quoted reason, next steps, case link, and for an accepted
kick or ban the rejoin invite. Never names staff.

## 9. Moderation log

### 9.1 Entries

`kind`, `result` (`ok`/`failed`/`refused`), `actor_id` (NULL = Quack),
`source`, typed links from one loaded record, and `detail` (the serde form
of `ModLogEntry`, no free text).

### 9.2 Kinds

"Posted": to the log channel (§9.4). Text: §12.10.

| `mod_log_kind` | Result | `detail` | Posted |
| --- | --- | --- | --- |
| `case_created` | ok | `{source, case_number, level_index, level_from_case_count, case_count, action_kind, timeout_seconds, ban_delete_message_seconds, sends_dm, replaces_case_number}` | yes |
| `case_create_refused` | refused | `{refusal, permission}` (Quack-side refusals) | yes |
| `case_voided` | ok | `{case_number, cause, cancelled_action_ids, undo_action_ids}` | yes |
| `case_context_edited` | ok | `{case_number, changed_keys}` | yes |
| `evidence_added` | ok | `{case_number, count}` | no |
| `action_done` | ok | `{case_number, kind, attempts, no_change}` | yes |
| `action_failed` | failed | `{case_number, kind, reason, permission, http_status, discord_code, attempts, may_have_applied}` | yes |
| `action_retry_requested`, `action_dismissed`, `action_undo_requested` | ok | `{case_number, kind}` | yes |
| `action_undo_skipped` | ok | `{case_number, kind, skip_reason, still_applies_case_number}` | yes |
| `dm_sent` / `dm_failed` | ok / failed | `{dm_kind}` / `{dm_kind, state, failure, discord_code}` | no / yes |
| `appeal_submitted`, `appeal_information_requested`, `appeal_information_provided`, `appeal_accepted`, `appeal_rejected`, `appeal_reopened` | ok | `{case_number}` | yes |
| `appeal_closed` | ok | `{case_number, because_case_voided}` | yes |
| `rule_created`, `rule_edited` / `rule_archived`, `rule_restored`, `rule_imported` | ok | `{name, version}` / `{name}` | yes |
| `settings_changed` | ok | `{fields}` (names only) | yes |
| `module_enabled`, `module_disabled`, `module_settings_changed` | ok | `{module, reason?, fields?}` | yes, except disabled by `quack_left` |
| `quack_joined` / `quack_left` | ok | `{first_join}` / `{}` | yes / no |
| `channel_setting_cleared` | ok | `{setting}` | yes |
| `log_channel_failed` | failed | `{entry_id, reason, discord_code}` | no |
| `ticket_opened` / `ticket_closed` | ok | `{}` / `{transcript_complete}` | yes |
| `honeypot_case_failed` | failed | `{refusal}` | yes |
| `legacy` | imported | `{action, resource_type, resource_id, failure_reason}` (§19.8) | no |

`action_failed.may_have_applied` (absent in old entries: false) makes the
text "Quack couldn't confirm whether Discord …". `actor_id` is the member
for their own appeal and ticket entries.

### 9.3 Append-only enforcement

Triggers on `mod_log` call `reject_change()` (migration 0001), so UPDATE,
DELETE, and TRUNCATE fail even for the owner. Entries commit with their
change; `case_create_refused` in its own transaction.

### 9.4 Posting to the log channel

- A posted entry enqueues `post_mod_log:<entry_id>` in its transaction,
  pinning the current log channel; later changes never re-post or redirect.
- `quack_engine::modlog::post`: pinned channel no longer the log channel →
  done. Staff-only check (§13.1.3); render from immutable records; re-read
  the log channel right before the create and skip if it changed. Create
  per §10.4 with nonce = entry ID.
- `Done` → `log_channel_posted_at` (only while still the log channel;
  migration 0002). Unknown Channel → compare-and-clear the setting, log
  `channel_setting_cleared`. Not staff-only or other refusal → no post.
  Unavailable, `Unknown`, rate limit, 5xx → retry (6 attempts, 10 s, 30 s,
  2 min, 10 min, 1 h).
- A finish without a post writes `log_channel_failed` **once per outage**
  (under the settings lock: none yet, or the last older than
  `log_channel_posted_at`).
- `action_failed` posts carry **Retry** (`1:ar:<action>`) if §5.8 allows;
  posts are never edited. Metric `quack.modlog.posts{quack.outcome}`.

### 9.5 Redaction

`detail` holds no content, tokens, or error bodies; staff free text stays
on its own row. Imported `legacy` rows pass through
`quack_core::modlog::redact` (secret-looking keys and reasons →
`"[REDACTED]"`, reasons cut to 240 characters).

### 9.6 Reading

`GET /api/guilds/{guild_id}/mod-log` (`moderate`): AND filters, newest
first, keyset pages (§14.1), items with `text` and `links`.

## 10. Tracked messages

### 10.1 The model

| Column | Meaning |
| --- | --- |
| `kind`; `case_id`/`appeal_id`/`ticket_id` | Kind and its owner (none for `ticket_panel`, `honeypot_warning`, one per server). |
| `channel_id`, `message_id` | Location; channel NULL only for an appeal post awaiting a channel. |
| `generation` | +1 per move, repost, or reactivation; write-backs compare it. |
| `wanted_revision`, `posted_revision`, `wanted_at` | Wanted vs on Discord; lag. |
| `private`, `interaction_token`, `token_expires_at` | Private receipts are edited only via the token (15 min − 60 s). |
| `state`, `stale` | `active`/`gone`; ≤ 5 old locations to clean up (oldest dropped). |
| `create_started_at`, `attachment_id`, `last_error` | Create window (§10.4); transcript kept on edits; machine code of the last failure, never shown. |

### 10.2 Bumping

`messages::bump(tx, guild, Owner)` increments `wanted_revision` on the
owner's `active` row and enqueues `sync_message:<id>` (§16.3).

| Change | Bumps |
| --- | --- |
| Action terminal, failed, retried, dismissed, undone; DM or evidence settles; context edit; void; appeal submitted or decided; replacement | `case_receipt` |
| Appeal transition; appeal channel set or changed | `appeal_post` |
| Ticket opened, closed, access fixed, transcript saved | `ticket_post` |
| Ticket settings or enablement | `ticket_panel` |
| Honeypot case, settings, enablement, or rule edit | `honeypot_warning` |

Giving a `gone` row (or channel-less appeal post) a destination
reactivates it: `active`, new channel, no message, `generation + 1`, bump.

### 10.3 The sync job

`quack_engine::messages::sync`, keyed per row; every transaction fenced.

1. **Read**: lock. `gone` or Quack not installed (→ `gone`) → done.
   Current with no `stale` → done. Note `rev`, `generation`, location; a
   create starts its window.
2. **Render** from current data, never the revision.
3. **Write** (room for 3 calls, else `Continue`):
   - Private receipt: edit via token; expired → `gone`.
   - No channel → done.
   - Appeal and ticket posts: staff-only check (§13.1.3); fails →
     `last_error = not_staff_only`, finish; missing → gone table;
     unavailable → retry.
   - Edit if posted (keeping `attachment_id`), else create (§10.4) with
     nonce `<row u22><generation base 36>`; a public receipt's first post
     edits the moderator's reply to `case.receipt.posted`.
4. **Record**: compare-and-set on `generation`; no match → a message just
   created goes to `stale`.
5. **Clean up** `stale` (room for 1 call each): edit to
   `ticket.panel.moved` / `appeal.post.moved`, or delete (warnings,
   receipts, ticket posts). Dropped on `Done` or gone; other refusals keep
   it with `last_error = stale_cleanup_<code>`.
6. Still behind → repeat (3 loops, then `Continue`).

**Gone** (10008, 10003, 50001; a refused first public receipt create; a
staff check finding the channel missing; `messages::on_missing`):

| Kind | Action |
| --- | --- |
| `case_receipt` | Refused first create, token valid → private, with `case.receipt.post_failed`: the reply is edited first, then `private` and the posted revision are recorded in one transaction, so a retry after an unknown edit takes the same path, note included (a refused edit → `last_error`). Else `gone`. |
| `appeal_post` | Repost in the current appeal channel; none → channel-less. |
| `ticket_panel`, `ticket_post`, `honeypot_warning` | Repost in the current entry, queue, or honeypot channel; none → `gone`. |

If the **channel itself** is gone or unreadable and still the target, no
repost: `last_error = destination_unavailable`. Reconciliation (§13.1.5)
turns modules off. `MESSAGE_DELETE` of a current location
(`messages::message_deleted`) applies the same plan in one transaction.
Moves append the old location to `stale` and bump with a new generation.
`Unknown`, rate limit, 5xx → retry (6 attempts, 2 s, 10 s, 1 min, 5 min,
30 min); other 4xx or exhaustion → `last_error`. Metrics:
`quack.message.syncs{quack.message.kind,quack.outcome}`,
`quack.message.sync.lag{quack.message.kind}`.

### 10.4 Creating messages safely

Discord's `enforce_nonce` deduplicates a repeated nonce (≤ 25 characters)
for "the past few minutes"; Quack assumes **60 s** (`NONCE_WINDOW`):

1. Before the first call, record the start (`dms.first_attempt_at`,
   `messages.create_started_at`, `evidence_files.copy_started_at`,
   `tickets.welcome_started_at`) and fix the nonce from the record's ID.
2. Within 60 s, an unknown create may be repeated with that nonce.
3. After it: member DMs settle `unknown` (§6); everything else is created
   again with a new start, so a long outage can duplicate one post (§20.4).

Channels and threads have no nonce and are looked up before each create
(§13.2.3, §13.1.4).

## 11. Discord surface

### 11.1 Commands

All commands are guild-install only (`integration_types = [0]`,
`contexts = [0]`); descriptions come from `quack-text` (§12.1). Only
`/setup` has a default member permission (Manage Server, `0x20`): staff may
hold a role rather than a Discord permission, so Quack checks capabilities
itself.

| Command | Type | Options | Capability | Reply |
| --- | --- | --- | --- | --- |
| `/case add` | slash subcommand | `member` USER req.; `rule` STRING req., autocomplete, max 100; `private` BOOLEAN | `moderate` | Private: the private receipt (staff card). Otherwise an ephemeral confirmation, and Quack posts the public receipt in the channel. |
| `/case view` | slash subcommand | `number` INTEGER req., min 1 | `moderate` | Ephemeral case card (§11.7). |
| `/case list` | slash subcommand | `member` USER; `rule` STRING, autocomplete (archived rules too) | `moderate` | Ephemeral list, 10 per page, newest first. |
| `/case void` | slash subcommand | `number` INTEGER req., min 1; `reason` STRING req., 1–1,000 | `moderate` | Ephemeral result (§4.7). |
| `/case history` | slash subcommand | `member` USER req. | `moderate` | Ephemeral totals and 10 cases per page. |
| `/appeals` | slash | — | `moderate` | Ephemeral `pending` appeals, oldest first, 10 per page, each with **Open**. |
| `/rules` | slash | — | `moderate` or `manage_rules` | Ephemeral active rules with a dashboard link each, 10 per page. |
| `/setup` | slash | — | `manage_settings` | Ephemeral guided setup panel (§11.8). |
| `/help` | slash | `topic` STRING, choices `cases`, `rules`, `appeals`, `setup` | none | Ephemeral help (§12.12). |
| Add case | message command | (the message) | `moderate` | Rule picker, then the private receipt. The message becomes evidence. |
| Add case | member command | (the member) | `moderate` | Rule picker, then an ephemeral confirmation and the public receipt. |

`rule` autocomplete answers from the rules cache (§17.6): case-insensitive
substring match on name and description, ≤ 25 choices (name cut to 100,
value the rule UUID), nothing when the payload and cached settings show no
`moderate`. A typed non-UUID value is matched exactly (ignoring case)
against active rule names, after the capability check; no match →
`error.rule_not_found`.

### 11.2 Command registration

`quack commands sync` and `quack serve` (before the gateway connects) run
`quack_discord::commands::sync`:

1. Build `quack_discord::commands::definitions()`.
2. Scope: `QUACK_DEV_GUILD_ID` set → that server's commands, else global.
3. Shape each definition for its route: a guild overwrite leaves out the
   global-only `contexts` and `integration_types`; the User and Message
   "Add case" commands never carry `description`.
4. `GET` the remote list and compare canonical forms (no ids, versions,
   or defaults; option order kept). Different → one bulk `PUT` of every
   definition, which also removes Go-era commands.
5. Store the returned command ids by `(type, name)` for command mentions
   (`</case view:ID>`). A failure stops `serve`; `commands sync` exits 1.

### 11.3 Acknowledgement rules

Every interaction is acknowledged before any database or Discord REST call,
except the prefilled-modal exception below; work happens after the ack.
`quack_bot`'s ack helper enforces this: a `Pending` interaction offers only
in-memory reads, and the engine is reachable only through the `Acked`
handle. The capability is checked from the payload and caches when the
response must be immediate, and fully (with 2FA) before any write.

| Interaction | Initial response | Final answer |
| --- | --- | --- |
| `/case add`, the cached rule has a required field and the payload shows `moderate` | Exception: write the draft row (§11.6), then modal page 1 from the rules cache. | §11.6 |
| `/case add` otherwise; "Add case" commands | Deferred channel message, ephemeral. | Ephemeral; a public receipt is a separate post (§11.7). |
| `/case view`, `list`, `void`, `history`, `/appeals`, `/rules`, `/setup` | Deferred channel message, ephemeral. | Ephemeral. |
| `/help` | Immediate ephemeral message (type 4), static text. | — |
| Autocomplete | Type 8 result from memory. | — |
| Button opening a modal with no stored values (Void, Accept, Reject, Appeal, Continue) | The modal (custom id, caches, static text). Void, Accept, Reject, Continue refuse privately, without a modal, when the payload shows no `moderate`. | — |
| **Exception:** a modal prefilled with stored values (Edit context, Try again on a draft page, Set rejoin invite, Edit warning, Add evidence) and `/case add`'s first page | Authorize (payload, caches, the actor's 2FA row) and do the one read or write the modal needs, bound to actor and server, all within 300 ms (`Pending::prefilled_modal`); then the modal. Not authorized → ephemeral refusal, no modal. Deadline missed → `interaction.busy`, no modal. | — |
| Button or select on an ephemeral message | Deferred update (type 6). | Edits that message. |
| Button on a shared message (public receipt, log post, appeal post, ticket post, ticket panel) | Deferred channel message, ephemeral (type 5, flag 64). | New ephemeral message; the shared message changes only through tracked-message sync (§10). |
| Modal submit | Deferred update when the modal came from an ephemeral message, else deferred ephemeral message. | Ephemeral. |

- Up to 256 handlers run at once (§11.9); each acknowledges first, then
  waits for one of 64 work slots (`quack_bot::MAX_WORK`), so slow work
  never delays another interaction's ack until 256 are in flight.
- The ack should go out within 2.5 s of the snowflake time
  (`quack.interaction.ack.duration`); a later ack is still sent and logged,
  never dropped by age (a skewed clock would drop every one).
- Type 5/6 callbacks carry only the ephemeral flag (omitted in DMs); the
  Components V2 flag goes on the `@original` edit or follow-up.
- A repeat of an interaction id within 15 minutes is dropped before the ack
  (§11.10); writes are protected again by operation keys (§4.3).
- An interaction Twilight can't read is answered from its envelope (id,
  token, type, server, channel): `interaction.expired_button`, or an empty
  choice list for autocomplete.

### 11.4 What is ephemeral

Everything staff-facing is ephemeral, errors included. Quack posts
publicly only the public receipt (§11.7), the ticket panel and thread
welcome, the honeypot warning, and posts in staff-only channels (appeal,
ticket, log, evidence copy, server activity). Every button on a public
message re-checks the clicker and answers privately.

### 11.5 Custom IDs

Format `1:<code>[:<arg>]…`, at most 100 characters (`1` is the scheme
version; the longest, `1:cd:<22>:<22>:<10>:<2>`, is 64). UUIDs are 22
characters of unpadded base64url (`u22`); snowflakes and integers are
decimal. `quack_discord::custom_id` has one enum variant per code, with a
property-tested round trip. A custom id carries no authority: every handler
authorizes the clicker.

| Code | Component | Arguments | Does |
| --- | --- | --- | --- |
| `cv` | Button Open case | case | Show the case card. |
| `cvs` | Select Open a case | value: case | Show that card. |
| `ce` | Button Edit context | case | Exception read: edit-context modal prefilled; over 5 fields → `case.context.too_many_fields` with a dashboard link. |
| `cem` | Modal | case, context revision | Save context (§4.9). |
| `ea` | Button Add evidence | case | Exception read (title names the case number): add-evidence modal, one link per line. |
| `eam` | Modal | case | Add evidence (§7.1). |
| `ev` | Button View evidence, pager | case, item (1-based) | Show one item. |
| `vd` / `vdm` | Button Void case / modal | case | Void modal / void (§4.7). |
| `hi` | Button history, pager | member, page | Member history page. |
| `cl` | Pager | page, member or `0`, rule or `0` | Case list page. |
| `ar` / `ad` | Button Retry / Dismiss failed {action} | action | §5.8. |
| `au` / `auc` | Button Remove timeout or Unban / "Yes, …" | action (the original) | Ask to confirm / undo (§5.8). |
| `rp` / `rpp` | Rule picker select / pager | draft (, page) | Set the draft's rule / picker page of 25 (§11.6). |
| `cd` | Modal (form page) | draft, rule, version, page | Save the page (§11.6). |
| `cn` | Button Continue / Fill in the form | draft, rule, version, page | That page's modal, no stored values. |
| `cr` | Button Try again (page with problems) | draft, rule, version, page | Exception read: the page prefilled with what was sent. |
| `cs` | Button Try again (saving failed) | draft | Apply the completed draft again. |
| `ap` / `apm` | Button Appeal case (DM) / modal | case | Appeal modal / submit (§8.1). |
| `aa` / `aam` | Button Accept / modal | appeal, version | Accept modal (default reason) / accept. |
| `aj` / `ajm` | Button Reject / modal | appeal, version | Reject modal / reject. |
| `ao` | Button Open appeal | appeal | Appeal card: the post's content, Accept and Reject while `pending`, a dashboard link. |
| `al` / `rl` | Pager | page | `/appeals` / `/rules` page. |
| `su` | Setup control | area, operation | §11.8. |
| `to` / `tc` / `tf` | Button Open a ticket / Close ticket / Restore private access | — / ticket / ticket | §13.2. |

**Legacy ids** (Go-era messages stay in Discord). Each supported shape is
decoded exactly; the ULID is mapped with §19.3 and must name a record of
this server.

| Legacy id | Handled as |
| --- | --- |
| `appeal:submit:v1:<case>` | `ap` |
| `appeal:accept:v1:<appeal>`, `appeal:reject:v1:<appeal>` | `ao` (the card shows current buttons) |
| `ticket:open:v1:` / `ticket:close:v1:<t>` / `ticket:repair:v1:<t>` | `to` / `tc` / `tf` |
| `case:view:v1:<case>`, and the same with `evidence`, `edit_context`, `void` | `cv` |
| `case:retry:v1:<execution>`, `case:dismiss:v1:<execution>` | `cv` for the mapped action's case (view only) |
| `case:user_detail:v1:<member snowflake>` | `hi` page 1 |
| anything else, or a ULID naming nothing here | `interaction.legacy_button` |

An undecodable `1:` id answers `interaction.expired_button`.

### 11.6 Multi-page `/case add` drafts

A modal holds 5 inputs, so a rule's up to 10 fields take up to 2 pages. A
draft is a `case_drafts` row (moderator, server, member, rule
version, receipt choice and channel, source message), expiring 15 minutes
after creation; housekeeping deletes it (§16.6). No form state is kept in
memory, so a restart never expires an open form.

**Starting.** `/case add` with required fields writes the draft
(`drafts::start`, UUIDv7) under the prefilled-modal exception (§11.3),
then opens page 1; if only the deferred path learns the rule needs input,
it writes the draft and answers `case.picker.fill_form` with **Fill in the
form** (`cn`). Without required fields it applies at once with operation
key = the interaction id. The "Add case" commands defer, authorize, and
write the draft (the message command adds its source message and
`private = true`; the member command `false`; no rule yet), then show the
rule picker (25 per page, `rpp`; `case.picker.no_rules` when empty; skipped
with exactly one active rule). On `rp` the draft takes the rule and current
version (`… AND rule_id IS NULL`) and empty `message_link` fields are
filled from the source message; required fields left →
`case.picker.fill_form`, else apply with key `draft:<id>`. The message
command's private receipt replaces the picker; the member command's picker
becomes `case.receipt.posted` and the public receipt is posted.

**Saving a page** (`cd`, deferred; `drafts::save_page`):

1. Authorize (fresh payload, 2FA). A case already exists for `draft:<id>`
   (a resubmitted last page) → answer with it.
2. One transaction: insert the row if missing, lock it `FOR UPDATE`. Not
   this moderator's, or expired → `case.draft.expired` (the bot first
   tries to apply, so a draft that became a case answers with it).
3. Its rule version differs from the page's → delete,
   `case.draft.rule_changed`. Already past this page (double click) →
   `case.draft.page_already_saved` with the current page's button.
4. Store and validate the values (§4.6). Problems → commit,
   `case.draft.page_invalid` with **Try again** (`cr`). Pages left →
   `case.draft.page_saved` with **Continue** (`cn`).
5. Last page → apply (§4.2) with key `draft:<id>`, version pinned; the
   case transaction deletes the draft. A failure without a case
   (`error.internal`, `error.discord_unavailable`, `error.unconfirmed`)
   keeps it and offers **Try again** (`cs`), safe through the key.

### 11.7 The case card and receipts

The **staff card** (`quack_engine::cases::staff_card`, §12.4) is only ever
ephemeral: `/case view`, Open case, list and history selects, and the
**private receipt** (the interaction's original response, kept current
through the token for 15 minutes). Its buttons are rendered from current
state and re-authorized on click: Retry / Dismiss failed per action
(`controls(action)`, §5.8), Edit context, Add evidence, View evidence (if
any), Member's case history, Void case (if valid), Remove timeout / Unban
(`controls(original).undo`), View failed actions (if one is failed and not
dismissed), and the dashboard link. Long context and evidence are cut and
continue on the dashboard.

The **public receipt** is a tracked message (§10.4) with only
`ReceiptView`'s fields: case number, rule, "· Voided", member, official
reason, the action line's first sentence, and `case.receipt.staff_hint`
when the action failed or is unconfirmed; never context, evidence,
moderator, void reason, appeal, DM, or failure detail. On a voided case
whose undo is done, that sentence also says the ban or timeout was lifted
(`case.receipt.action_lifted`). Its only button is
Open case (`cv`). If Quack can't post it, the case stands and the
moderator gets `case.receipt.post_failed` with the card.

### 11.8 Guided `/setup`

One ephemeral panel, one button per area; every click re-checks
`manage_settings` + 2FA, saves at once through the same functions as the
dashboard, and re-renders with a result line. The bot's setup handler
calls the module crates' settings functions, `use_default_rule`,
`route_all`, and `set_enabled` with their hooks, then reloads the config
cache. Channel creation ("Create …", overwrites per §13.1.4) lives in
`quack-bot`.

| Area (`su` code) | State shown | Controls (`su` operation) |
| --- | --- | --- |
| Home (`home`) | One line per area. | A button per area; server settings link. |
| Log channel (`log`) | Channel or "Not set". | Select `ch`; Create #moderation-log `new`; Stop posting `clr`. |
| Appeals (`ap`) | Appeal channel; rejoin invite set or not. | Select `ch`; Create #appeals `new`; Stop posting `clr`; Set rejoin invite `inv` (modal `invm`, exception read); Remove invite `clrinv`. |
| Evidence copies (`ev`) | Channel or "Off". | Select `ch`; Create #evidence `new`; Stop copying `clr`. |
| Tickets (`tk`) | On/off; entry and queue channels. | Selects `entry`, `queue`; Create channels `new` (#support, #ticket-log); `on`; `off`. |
| Honeypot (`hp`) | On/off; channel; rule; warning. | Select `ch`; Create #honeypot `new`; rule select `rule` (active rules without required fields); Clear rule `clrrule` (only while off); Edit warning `warn` (modal `warnm`, exception read); `on`; `off`. |
| Server activity log (`al`) | On/off; channel, or "Several channels". | Select `ch` (routes every event there); Create #server-log `new`; `on`; `off`. |

Selects offer text channels only; a staff destination must pass §13.1.3.

### 11.9 Gateway intents and events

Intents, fixed: `GUILDS`, `GUILD_MEMBERS` and `MESSAGE_CONTENT`
(privileged: enabled in the developer portal; approval needed at 100+
servers), `GUILD_MODERATION`, `GUILD_MESSAGES`. Close code 4014
(disallowed intents) stops `serve` with a log line naming them; 4004
(bad token) stops it with a line naming `QUACK_DISCORD_TOKEN`. One shard
(`ShardId::ONE`); when `GET /gateway/bot` recommends more, `serve` stops
(§20.4). `quack_discord::Gateway::run` yields Quack's typed `Event`s and
updates the guild and member caches in the shard loop.

| Event | Handling |
| --- | --- |
| `READY` | Mark connected; record Quack's own account id; clear the member cache; mark every server unloaded until its `GUILD_CREATE`. Installed servers missing from the list are treated as left (logged and skipped on error, the pass goes on). After an invalid session, every `open` ticket gets `journal_gaps`. |
| `RESUMED` | Mark connected; nothing was missed. |
| `GUILD_CREATE` | Guild cache; seed members; bootstrap the server (§17.5); reconcile channel settings (§13.1.5); warm rules and config caches; mark loaded. |
| `GUILD_UPDATE` | Update `guilds` (name, icon, owner, MFA level); activity `server_changed`. |
| `GUILD_DELETE` | `unavailable` → mark unloaded (outage); else Quack left (§17.5). |
| `GUILD_ROLE_*` | Guild cache only; deleted roles are ignored as staff roles. |
| `CHANNEL_CREATE/UPDATE` | Guild cache; activity `channel_changed`. |
| `CHANNEL_DELETE` | Guild cache; clear settings pointing at it (§13.1.5); activity. |
| `THREAD_DELETE` | A ticket thread (`opening`/`open`) → close the ticket (§13.2.4). |
| `GUILD_MEMBER_ADD/UPDATE/REMOVE` | Member cache; activity `member_joined` / `member_left`. |
| `MESSAGE_CREATE` | Activity message cache; ticket journal; honeypot hit. |
| `MESSAGE_UPDATE` | Activity `message_edited`. |
| `MESSAGE_DELETE`, `MESSAGE_DELETE_BULK` | Activity; a deleted tracked message is handled by `messages::message_deleted` (§10.3). |
| `GUILD_AUDIT_LOG_ENTRY_CREATE` | Ban (22) / unban (23) by someone other than Quack → activity `member_banned` / `member_unbanned` (needs View Audit Log; health says when it's missing). |
| `INTERACTION_CREATE` | Interaction router (§11.3). |

**Dispatch.** The shard loop only updates caches and hands events off; it
never waits. Queues:

| Queue (`quack.queue.name`) | Bound | Contents |
| --- | --- | --- |
| `gateway.interactions` | 256, drained by at most 256 handlers | Interactions (§11.3). |
| `gateway.lifecycle` | 4,096, one ordered consumer | `READY`, `GUILD_CREATE`, `GUILD_DELETE`, `CHANNEL_DELETE`, so a message flood never drops a join, leave, or channel removal (ordered among themselves only). |
| `gateway.events` | 4 queues of 1,024, chosen by server id | Everything else, one server's events in order. |

A full queue drops the event and counts `quack.queue.dropped`. A dropped
`MESSAGE_CREATE` only records its channel in a bounded set (10,000); a task
then marks that ticket's `journal_gaps` (the journal tells threads from
channels by the guild cache, else the database).

**Unreadable frames.** An interaction is recovered from its envelope
(§11.3). Any other unreadable event except messages and typing may have
been a cache update a resume won't replay, so the guild cache is untrusted
until the next `READY` (facts come from REST meanwhile). A compression or
reconnect failure, or the shard stopping, marks the session disconnected
until it resumes.

**Reconnects.** Twilight reconnects and resumes by itself and waits
between failed connection attempts (1 s, doubling, up to about 4 minutes).
A connection that opens and closes again before `READY`/`RESUMED` gets
Quack's own wait (1 s, doubling, at most 1 minute), so the shard loop never
spins. Each drop logs one line at `info` ("the gateway connection dropped;
reconnecting", with why) and counts `quack.gateway.connection.event =
reconnect`; each failed attempt logs at `info` with its cause and counts
`connect_failed`, and from the fifth in a row logs at `warn` ("still can't
reconnect to the gateway"). The session coming back logs one line
("resumed the gateway session" or "reconnected to the gateway with a new
session", with how long it was down and how many attempts failed); the
first session logs "connected to the gateway".

### 11.10 Caches

| Cache | Contents | Bound | Freshness | `quack.cache.name` |
| --- | --- | --- | --- | --- |
| Guild cache (`twilight-cache-inmemory`: `GUILD`, `CHANNEL`, `ROLE`, `USER_CURRENT`) | Owner, MFA level, name, icon, channels with overwrites and parents, roles | Servers on the shard | Gateway; trusted for a server only while connected, loaded, and not tainted (§11.9) | `guilds` |
| Member cache (`quack-discord`) | (server, account) → roles, joined, timeout end, bot flag, names, avatar | 200,000, LRU | Gateway entries exact while trusted; REST entries 5 min; removed on leave; "not a member" never cached | `members` |
| Rules cache (`quack-engine`) | Every rule's name, description, archived flag; current definition of active rules | Per server | Reloaded on `NOTIFY quack_rules` | `rules` |
| Config cache (`quack-engine`) | `guild_settings` and module settings | Per installed server | Reloaded on `NOTIFY quack_settings` | `config` |
| Name cache (`quack-engine`) | Account → display name for dashboard sentences (§12.0) | 10,000; full → expired dropped, else cleared | 5 min; only names Discord returned | `names` |
| Activity message cache (`quack-activity-log`) | (server, channel, message) → author, redacted text, files, embed types | 50,000 and 64 MiB | 24 h, LRU | `activity_messages` |
| Interaction dedupe (`quack-bot`) | Interaction ids | 100,000 | 15 min | `interactions` |
| Account servers (`quack-api`) | Session → the account's Discord servers | 1,000 | 60 s | (no metrics) |

- **Member revisions.** Every gateway member change bumps a per-server
  revision; a REST read or search stores its answer (a member or its
  absence) only if the revision is unchanged since before the request, so
  a late answer never resurrects a departed member or erases a rejoin.
- **Facts** (`quack_discord::facts::Facts`): the caches under §2's
  freshness rules, else REST. One permission calculation,
  `quack_core::permissions::in_channel`, serves facts and evidence access:
  a thread uses its parent's overwrites and Send Messages in Threads; the
  timeout restriction applies after every overwrite; a thread with an
  unknown parent gets nothing (fail closed); voice Connect isn't modelled.
- Named caches count `quack.cache.lookups` and `quack.cache.evictions`.
  Twilight's other caches are off. There is no dashboard directory cache.

### 11.11 Rendering

`quack_discord::render` turns `quack-text`'s neutral `Msg` into one
Components V2 Container (no `content`, no embeds): consecutive text blocks
become one Text Display, Divider a Separator, File a File component, rows
Action Rows; `Msg::tone` sets the accent (green done, amber in progress,
red failed). `quack_text::budget` keeps every message within 40 components
and 4,000 characters: `budget::fit` drops subtext, cuts the longest quote
or text, then trailing fields, and appends the message's own note or
`render.trimmed`; lists are paged first (10 rows). Typed text is escaped;
mentions never ping except the ticket welcome's. Labels: buttons ≤ 80,
select options ≤ 100, modal titles and labels ≤ 45. Channel message edits
also send `"content": null` and `"embeds": []` (§17.3).


## 12. Message catalog

All wording lives in `crates/quack-text`, one function per message or group
(`quack_text::case::card` renders every `case.card` line), matching every
state and failure exhaustively; its `insta` snapshots (both surfaces, every
variant, crates/quack-text/src/tests/snapshots) are the wording's
reference. The dashboard shows API text fields (`message`, `text`,
`outcome_text`, `state_text`) as given. New message: function, sample in
crates/quack-text/src/tests/samples (`id` or `covers`), reviewed snapshot,
and its id or group listed below (the coverage test fails for any id named
here without a sample).

### 12.0 Notation

| Placeholder | Discord | Dashboard / API text |
| --- | --- | --- |
| `{member}`, `{actor}`, `{author}` | `<@id>`, never pings | Display name, else "a member (ID {id})"; NULL actor "Quack" |
| `{case}` | `**[case #N](<url>)**` in log posts, else `case #N` | "case #N" |
| `{rule}`, `{server}` | Bold name | Name |
| `{channel}`, `{role}` | `<#id>`, `<@&id>` | `#name` / `@name`, else "an unknown channel (ID {id})" |
| `{ends_at}`, `{time}`, `{datetime}`, `{date}` | `<t:unix:f> (<t:unix:R>)`, `<t:unix:R>`, `<t:unix:f>`, `<t:unix:d>` | "17 Nov 2026 at 14:05 UTC" (date only for `{date}`) |
| `{duration}`, `{last}` | `quack_text::fmt::duration` ("1 hour 30 minutes"), `fmt::last` ("the last day") | same |
| `{permission}` | Discord's English UI name (Timeout Members, …) | same |

Typed text is escaped (no Markdown, mentions, or links); the server's own
Markdown (DM intro and footer, honeypot warning) keeps formatting but can't
mention.

Dashboard sentences (the staff case view, the actions page, the moderation
log, the void message) name people by global name, else username:
`quack_engine::names::Names` takes the names the name cache (§11.10) read
in the last 5 minutes and reads the rest once per response through the
port's `accounts`; when Discord can't answer, the "a member (ID {id})"
wording stands. Member pages say "you" and never name staff.

### 12.1 Commands

`command.*`, `command.case.*`, `command.help.*`, `command.add_case`.

### 12.2 Shared words

`word.action.*`, `word.outcome.*`, `word.page`, `button.*`,
`select.open_case`, `render.trimmed`, `level.summary.item`,
`level.outcome.*`.

### 12.3 Errors and refusals

Ephemeral in Discord and the API `message` of the matching code (§14.6).
`error.for_domain` renders every `DomainError` from `ErrorFacts` (scene,
case, rule, action, member); other `error.*` are the engine's own refusals
and dashboard variants. `interaction.*` (busy, expired, legacy button);
`refusal.*` (member refusals, each starting "No case was added.");
`access.*` (one sentence per permission or role problem, shared by every
surface).

### 12.4 Case card and receipt

`case.card.*` (in order: what staff act on before context, so trimming
drops context first; ends with `case.card.trimmed` when cut),
`case.receipt.*` (`case.receipt.action_lifted`: a voided case's action line
once its unban or timeout removal is done), `case.button.*`,
`case.saved_unconfirmed`.

### 12.5 Adding a case: picker, form, drafts, context problems

`case.picker.*`, `case.form.*`, `case.draft.*` (§11.6), `context.*` (one
per §4.6 problem), `member.context.link_saved`.

### 12.6 Actions

A pending action after an attempt reads `action.retry_scheduled`; `failed`
reads `action.<kind>.failed` + `action.failure.*`; an unknown outcome
(`unconfirmed`, or maybe applied before a later refusal, §5.2) reads
`action.unconfirmed` then the reason, never both "couldn't" and "couldn't
confirm"; `action.cancelled_uncertain`; `action.dismissed_suffix` (staff
only). Example (timeout): "Quack will time out Sam until 18 Nov 2026 at
15:05 UTC." / "Quack couldn't confirm whether Discord timed out Sam. Check
Sam's timeout in Discord." Groups: `action.*`, `action.check.*`,
`action.<kind>.*`, `member.action.*`, `action.undo.*` (promises a check,
§5.6), `case.void.*`, `case.context.*`, `evidence.*`.

### 12.7 DM outcome sentences

`dm.state.*` + `dm.failure.*` (`quack_text::dm::outcome_sentence`), the
same on every staff surface; `member.dm.*` for the member. The failed-DM
story: "Quack couldn't send @Sam a DM about case #12. Discord didn't allow
the DM. @Sam can sign in to the Quack dashboard to see the case."

### 12.8 DMs to members

`dm.case`: the facts and the configured outcome (never Discord's result,
§6.3) before the server's intro, context, and footer, so trimming keeps
them. `dm.appeal.*`: one per appeal event.

### 12.9 Appeals

`appeal.*` (form, submit, information, post, buttons, modals, decisions),
`appeals.*` (`/appeals`), `timeline.*` and `appeal.event` (members never
see a staff name).

### 12.10 Log channel entries

A post is a lead sentence, an optional quote (a staff void's reason), and
`-# {datetime} · Open moderation log`; failed and refused entries are red.
Example: "Ana added case #12 for Sam under Spam (level 2, case count 4).
Action for this case: timeout for 1 hour." `log.*`: one per §9.2 kind;
`log.action_failed` opens with "Quack couldn't confirm whether Discord …"
when its detail's `may_have_applied` is true (absent reads false).
`setting.*` and `module.*` name changed settings and modules.

### 12.11 Setup

`setup.*`, `channel.*` (created channels), `module.problem.*`,
`setting.problem` (one per `SettingsProblem`).

### 12.12 Help, rules, lists

`help.*`, `rules.*`, `case.list.*`, `case.history.*`, `rule.problem`
(one per `RuleProblemCode`).

### 12.13 Module messages

`ticket.*`, `dm.ticket_closed.*`, `activity.*`, `honeypot.warning.*`
(says what the rule can do, never that it will succeed),
`honeypot.outcome.*`.

### 12.14 API messages

`api.*` for API-only texts (including `api.value_invalid` and the sign-in
page's `api.sign_in_failed`, `api.sign_in_denied`); otherwise the matching
`error.*` or `refusal.*` text. Codes reusing another text: snapshot
`api.code`.

## 13. Settings and optional modules

### 13.1 Server settings and shared pieces

#### 13.1.1 Core settings

One `guild_settings` row per server, created by bootstrap (§17.5). Several
settings may name the same channel.

| Setting (API field) | Default | Validation | Who changes it |
| --- | --- | --- | --- |
| `moderator_role_ids` | `[]` | 0–25 distinct current roles (saved IDs of deleted roles are kept, ignored); not @everyone. | `manage_staff_roles` (others may resend the same set) |
| `rules_manager_role_ids` | `[]` | Same. | `manage_settings` |
| `log_channel_id`, `appeal_channel_id` | NULL | Staff-only (§13.1.3); Quack needs View Channel, Send Messages. | `manage_settings` |
| `evidence_channel_id` | NULL | Staff-only; also Attach Files. | `manage_settings` |
| `appeal_invite_url` | NULL | `discord.gg/`, `discord.com/invite/`, or `discordapp.com/invite/` + code (2–64 of `[A-Za-z0-9_-]`), optional `https://`, `www.`; stored as `https://discord.gg/<code>`. | `manage_settings` |
| `dm_intro` / `dm_footer` | NULL | 0–1,000 / 0–500 characters; empty stores NULL. | `manage_settings` |
| `starter_rule_id` | set at bootstrap | read-only | — |
| `starter_notice_acknowledged_at` | NULL | Set by route 38; the first time is kept. | `manage_settings` |

Rejected values go in `400 validation_failed` `fields` with a
`SettingsProblem` code (`setting.problem`): `too_many_roles`,
`duplicate_role`, `everyone_role`, `unknown_role` (first problem in list
order), `invite_url`, `too_long`, `retention_days`. Channels report
`ChannelProblem` codes (§13.1.3).

#### 13.1.2 Updating settings

`PATCH …/settings` (route 37) writes only the fields it contains:

1. Gather the Discord facts the sent fields need, before any lock.
2. One transaction: lock the `guild_settings` row `FOR UPDATE`.
3. Sending either role list requires `expected_version`; a mismatch is
   `409 settings_changed`, nothing saved.
4. Validate every sent field (pure) against the locked row and facts;
   channel checks use the moderator roles being saved, else the locked
   ones. Problems are per field; nothing saved.
5. Update the sent columns, `version + 1`, `updated_at`, `updated_by`; bind
   or move affected tracked messages (§8.3, §10.2); log `settings_changed`
   with the field names; `NOTIFY quack_settings`.

Module settings (routes 42, 44, 46) follow the same steps on their own row,
but the body is a full replacement that always carries `expected_version`,
and every field is validated even while the module is off.

#### 13.1.3 Staff-only channel check

`quack_core::permissions::check_staff_channel(channel, moderator_role_ids,
quack_needs) -> Vec<ChannelProblem>`, pure. A destination passes when:

1. It is a text channel (type 0) of this server (`not_text`, reported
   alone).
2. @everyone is denied View Channel (`everyone_can_view`).
3. Every role overwrite allowing View Channel is for Administrator, Moderate
   Members, or a moderator role (`role_can_view`).
4. Every member overwrite allowing View Channel is for Quack, the owner, or
   staff for Discord access (`member_can_view`).
5. Quack's channel permissions (§2) include `quack_needs` (`quack_missing`).

Health adds `inaccessible` (403; the setting is kept) and `missing`. It runs
on save and, from current facts, before every post to a staff destination.
It protects new posts only; staff role changes never edit overwrites
(§20.4).

#### 13.1.4 Creating channels

"Create …" in `/setup` (`quack-bot` `handlers::setup_channels`, one at a
time per server under that server's lock):

1. **Find first.** A text channel with the catalog name, an ID under 10
   minutes old, and a member overwrite for Quack is adopted
   (`setup.adopted`): double clicks and lost answers are harmless.
2. **Check.** Manage Channels, plus every permission the overwrites set
   (Discord allows only held ones); else `setup.create_failed.permission`.
3. **Create** with name, topic, and the overwrites below. `Unknown` →
   `setup.create_unconfirmed` (the next click adopts it).
4. **Save** through the normal update (§13.1.2), which binds the channel's
   tracked message (ticket panel, honeypot warning). A staff destination's
   intro message follows §10.4; its failure undoes nothing.
5. Answer `setup.created`, or `setup.created_public` (ticket entry,
   honeypot).

| Kind | @everyone | Quack (member overwrite) | Staff roles* |
| --- | --- | --- | --- |
| Staff destination | deny View Channel | allow View Channel, Send Messages, Read Message History, Attach Files, Embed Links | allow View Channel, Send Messages, Read Message History |
| Ticket entry | allow View Channel, Read Message History, Send Messages in Threads; deny Send Messages | allow View Channel, Send Messages, Read Message History, Create Private Threads, Manage Threads, Send Messages in Threads | — |
| Honeypot | allow View Channel, Send Messages, Read Message History | allow View Channel, Send Messages, Read Message History, Manage Messages | — |

\* Moderator roles that still exist, else every role with Administrator or
Moderate Members.

#### 13.1.5 Channel reconciliation

A channel deleted while Quack runs (`CHANNEL_DELETE`), or missing at
`GUILD_CREATE` and confirmed 404 by `GET /channels/{id}`, is cleared from
every setting in one transaction per server (`guilds::channel_deleted`),
each a compare-and-clear; a 403 is not a deletion (health shows
`inaccessible`). Core channels → NULL, log `channel_setting_cleared`.
Ticket entry or queue, or honeypot channel → NULL and the module disabled
(`channel_deleted`, log `module_disabled`; the honeypot warning goes
`gone`). An activity route → removed, and the module disabled when none
remain. A ticket's thread (`THREAD_DELETE`, or absent at `GUILD_CREATE` and
confirmed 404) closes the ticket (§13.2.2). Tracked messages follow §10.3.

#### 13.1.6 The `Module` contract

No module trait exists in the engine. Each module crate (`quack-tickets`,
`quack-activity-log`, `quack-honeypot`) exports service functions (called
by `quack-bot` and thin `quack-api` handlers), plus `jobs() ->
ModuleJobs` (its job kinds and one handler, also called with
`Phase::Finalize`, §16.1) and `hooks() -> ModuleHooks` (`enable_problems`
and `on_enabled_changed`, run inside `settings::set_enabled`). The Discord
seam is `quack_bot::Module` (`name`, `owns`, `wants`, `on_component`,
`on_event`), one adapter per crate in `quack_bot::modules`; only tickets
owns components (`to`, `tc`, `tf`; unregistered → `interaction.expired_button`).
The `/setup` module areas are the bot's (`handlers::setup_modules`), calling
the crates' settings functions, `use_default_rule`, `route_all`, and
`set_enabled`, and reloading the config cache after each change.
`quack::compose::App` wires it all (§17.5).

#### 13.1.7 Enable and disable

Routes 39–40 (`manage_settings` + 2FA; also `/setup`). Enable gathers facts;
one transaction locks the module row (honeypot: also its rule `FOR SHARE`,
§4.12) and runs the enable check: problems → `409 module_not_ready` with
`problems` (`module.problem.*`); none → `enabled`, clear
`disabled_reason`, bump `version`, `on_enabled_changed`, log
`module_enabled`, `NOTIFY`. Disable mirrors it (log `module_disabled`) and
keeps settings. Asking for the current state writes nothing. Queued work
ends by its own rule: honeypot hits fail `policy_changed`, open tickets
stay closable, queued activity posts are dropped.

### 13.2 Tickets

Private support threads between a member and staff, separate from cases.

#### 13.2.1 Settings (`ticket_settings`)

`enabled`, `disabled_reason` (`channel_deleted`, `quack_left`),
`entry_channel_id` (a text channel), `queue_channel_id` (staff-only; View
Channel, Send Messages, Attach Files), `transcript_retention_days` (90,
1–365), `version`. Enable problems: entry or queue missing or equal; Quack
lacking View Channel, Send Messages, Read Message History, Create Private
Threads, Send Messages in Threads, or Manage Threads in the entry channel;
a bad queue. A new entry channel moves the panel (§10.3).

#### 13.2.2 States

`opening`, `open`, `closing`, `closed`. The partial unique index
`tickets_one_live` allows one non-`closed` ticket per member.

| From | Event | To |
| --- | --- | --- |
| (none) | Member clicks **Open a ticket** | `opening` (+ `open_ticket`) |
| `opening` | Thread ready, member added, welcome posted | `open` (+ `ticket_post` and its sync) |
| `opening` | Definite refusal, thread deleted, or budget spent; no thread | `closed` (`open_failed`) |
| `opening` | The same with a thread | `closing` (`open_failed`; only deletes the thread) |
| `open` | Member or moderator closes | `closing` (+ `close_ticket`) |
| `open` | `THREAD_DELETE` | `closing` (by Quack) |
| `closing` | Close job settles or its budget is spent | `closed` |

#### 13.2.3 Opening

Component `to` defers ephemerally; tickets off → `ticket.open.disabled`.
One transaction inserts the ticket and its `open_ticket` job; a
`tickets_one_live` violation answers with the live ticket
(`ticket.open.exists` / `.closing` / `.opening`); else `ticket.open.opening`.

`open_ticket` (budget 4, backoff 2 s, 10 s, 30 s; writes compare-and-set on
`opening`):

1. Not `opening` → `Done`.
2. **Thread.** Look up `ticket-<ticket u22>`: only **private** threads
   (type 12) under the entry channel with that exact name **created by
   Quack** (`owner_id`), active then archived newest first, until no more
   or threads archived before the ticket; the 10-page cap or a cursorless
   "more" page is `Unknown` (retry, never create). Adopt the oldest,
   delete the rest; none → create (`invitable: false`,
   `auto_archive_duration: 1440`). Save `thread_id` at once. Refused →
   open fails.
3. Add the member (idempotent `PUT`). Refused → open fails.
4. Post `ticket.welcome` with **Close ticket** per §10.4 (nonce = ticket
   u22 + `w`).
5. Transaction: `open`, `opened_at`, the `ticket_post` row (queue channel:
   member, state, transcript, **Close ticket** `tc`, **Fix access** `tf`,
   **Open thread**) and its sync, log `ticket_opened`.

Finalization: the open fails.

#### 13.2.4 Journal and closing

- **Journal.** `MESSAGE_CREATE` in a live ticket's thread inserts
  `ticket_messages` (`ON CONFLICT DO NOTHING`) until the freeze, and after
  it for messages sent before `frozen_at` until the transcript is saved.
  Edits and deletes never change rows. Threads are told from channels by
  the guild cache (database only when the cache doesn't know).
  `journal_gaps` is set by a dropped event (§11.9), an insert failing 3
  times, or an invalidated session while live.
- **Closing.** `tc` or route 50, by the member or `moderate` + 2FA: lock;
  not `open` → `409 ticket_not_open`; else `closing`, `closed_by`,
  `close_requested_at`, `close_ticket` job; `ticket.close.closing`. The
  operation key (`Idempotency-Key` or interaction), scoped to server and
  account, is hashed with the ticket: replay answers the ticket, another
  key is `ticket_not_open`, the key on another ticket is
  `idempotency_key_reused`.

`close_ticket` (budget 6, backoff 5 s, 30 s, 2 min, 10 min, 1 h; each step
records a completion column):

1. **Freeze**: `{archived: true, locked: true}`; done on success or 404. An
   archived thread (400) is unarchived and locked in one request, then
   archived; other refusals retry. `frozen_at`.
2. **Transcript** (not for `open_failed`): history newest first, at most 10
   pages of 100, merged with the journal by message ID (journal text
   wins); no wait for the event queue. `complete` = history reached the
   first message or the journal has no gaps. Over 8 MiB → cut, partial
   `trimmed`. Save with `expires_at = now + retention`.
3. **Close** (one transaction): `closed`; journal `expires_at`; unless
   `open_failed`, the `ticket_closed` DM; bump `ticket_post` (attaches the
   transcript once, §10.1); log `ticket_closed`.
4. **Delete the thread**: 404 → done; refused → `thread_delete_failed`.

Finalization (database only): a ticket not `closed` freezes its journal,
saves a journal-only partial transcript if none, and runs step 3; an
undeleted thread is kept with `thread_delete_failed`.

**Fix access** (`tf`, `moderate` + 2FA, `open`): unarchive, re-add the
member, remove every thread member who is not the member, Quack, the owner,
or staff for Discord access; `ticket.fix.done` or `ticket.fix.failed`. Not
logged.

#### 13.2.5 Storage, jobs, retention

Tables `ticket_settings`, `tickets`, `ticket_messages`,
`ticket_transcripts`; tracked messages `ticket_panel`, `ticket_post`.
Housekeeping deletes transcripts and journal rows past `expires_at` and
trims tickets open over 90 days to their newest 50,000 journal rows
(`journal_gaps`). After `expires_at` route 49 is `404 file_unavailable`;
Discord copies stay.

### 13.3 Server activity log

Posts Discord events to staff-only channels; logs only its own settings
changes.

#### 13.3.1 Settings

`activity_log_settings`: `enabled`, `disabled_reason`,
`include_message_content`, `include_file_names`, `include_embed_types` (all
true), `version`; `activity_log_routes (guild_id, event, channel_id)`, one
channel per event. Events (`activity_event`, text `activity.<event>`):
`message_edited` (`MESSAGE_UPDATE` with changed text or files),
`message_deleted`, `messages_bulk_deleted`, `member_joined`,
`member_left`, `member_banned` / `member_unbanned` (audit log entries 22 /
23 not by Quack), `server_changed`, `channel_changed`. `/setup` routes all
to one channel (`route_all`). Enable problems: no route, or a route channel
that fails §13.1.3 (View Channel, Send Messages). Without View Audit Log
the ban events are skipped (`module.problem.activity_needs_audit_log`).
Quack's own account comes from `READY`, else guild facts.

#### 13.3.2 Behaviour

- **Cache** (the only message cache, §11.10): `MESSAGE_CREATE` in servers
  with the module on.
- **Redaction** (`redact_secrets`: bot tokens, webhook URLs, `token=`,
  `password=`, `secret=`, `authorization:`, `bearer `) before caching or
  posting; privacy flags apply at both.
- **Filtering.** Quack's, webhooks', and route-channel messages are cached
  as tombstones and never posted; other bots are.
- **Edits.** Missing fields are unknown, empty text is known-empty
  (`activity.after_empty`); unchanged text and files post nothing.
- **Deletes.** Cached → posted at once. Uncached → batched per channel for
  60 s or 50 IDs (≤ 1,000 open batches).
- **Delivery.** In memory, not durable: 10,000 queued posts (full drops),
  2 senders. Before **every** try a post re-reads route and flags from the
  config cache (changed route follows, removed route or module off drops),
  re-checks the destination from current facts (fails closed), and renders
  with the flags as they are then; created per §10.4 (25-character
  event-hash nonce). `RetryAfter`, `NotSent`, `Unknown` → 2 more tries;
  refusals are final; Unknown Channel reconciles the route (§13.1.5).
  Counted as
  `quack.activity_log.events{quack.activity.event,quack.activity.stage}`.
- Disabling or leaving empties that server's batches and cache; leaving
  retires its in-process posted/dropped counters (route 43). Shutdown
  flushes batches and drains the queue (§17.5).

### 13.4 Honeypot

A post in the trap channel by a non-staff member adds a case under the
honeypot's rule through the normal case path.

#### 13.4.1 Settings (`honeypot_settings`)

`enabled`, `disabled_reason`, `channel_id` (text; Quack needs View Channel,
Send Messages, Read Message History, Manage Messages), `rule_id` (active,
no required fields, §3.8; cleared only while off), `warning_text` (0–1,500;
empty uses the generated text), `version` (bumped by every change). Enable
problems: channel or rule missing, required fields, missing permissions.
Enabling from `/setup` without a rule creates "Honeypot"
(`use_default_rule`); re-enabling reactivates a `gone` warning. The page's
`eligible_rules` carry levels for the warning preview.

#### 13.4.2 From post to case

1. `MESSAGE_CREATE` in the channel with the honeypot on (config cache).
   Bots, webhooks, and staff for Discord access are ignored, judged from
   the message's roles and guild facts (without facts, roles only; step
   3.2 re-checks).
2. One transaction: `honeypot_hits` (message ID unique, `pending`, current
   `version` and `rule_id`) and the coalesced job
   `honeypot_case:<guild>:<member>`. Startup `recover` re-queues pending
   members.
3. `honeypot_case` (budget 5, backoff 5 s, 30 s, 2 min, 10 min):
   1. Batch = leading pending hits sharing the first hit's version and
      rule; the rest wait (`Continue`).
   2. Not installed → `quack_not_in_server`. Poster now staff or a bot →
      `exempt` (not logged); a departed poster is not exempt.
   3. A valid case from this member's hits, same version, whose first hit
      is under 30 s older than the batch's (measured from the case's first
      hit, so a slow drip can't stretch a burst) → attach (a burst is one
      case), if under the `FOR SHARE` settings lock the module is still on
      with the hits' version, channel, and rule; else `policy_changed`.
   4. Else apply the rule (§4.2): source `honeypot`, actor Quack, key
      `honeypot:<first message ID>`, version and channel checked under lock
      (§4.2 step 8), the first message as evidence.
   5. Created, replayed, or attached → `case_created`; one
      `honeypot_cleanup` per hit; bump the warning.
   6. Member refusal, `policy_changed` (also a reused key), or
      `rule_unavailable` → `failed`, messages kept, log
      `honeypot_case_failed`.
   7. `DiscordUnavailable` → retry. Finalization → `discord_unavailable`
      (logged); later hits get a fresh budget.

   `failure` is a `HoneypotFailure`: `Refused(HoneypotRefusal)` (the logged
   values), `exempt`, or `imported_pending` (§19.9).
4. `honeypot_cleanup` (keyed per message; budget 10, backoff 1–64 s)
   deletes the post only once its evidence is `saved` (burst posts without
   evidence directly). `saving` → `Defer` 5 s for up to 10 minutes; a
   failed or still-saving capture, or a non-404 refusal, records
   `cleanup_failed_at` and keeps the post. 404 counts as deleted.

#### 13.4.3 The warning

`honeypot_warning` renders `honeypot.warning.*` from the rule's levels (or
custom text) and the honeypot case count; `honeypot.warning.off` while off.
Bumped by honeypot cases, settings, enable/disable, rule edits, and its own
deletion (§10.3).


## 14. HTTP API

For the dashboard only, under `/api` on its origin (`quack serve`, axum,
utoipa).

### 14.1 Conventions

- JSON bodies need `Content-Type: application/json` (else `415`).
- Discord IDs, UUIDs, and context numbers are strings; case numbers and
  versions are numbers; RFC 3339 UTC timestamps; snake_case enums. Bad IDs
  or filter values → `400 validation_failed` (`api.value_invalid`).
- Staff `{case_ref}`: number, UUID, or legacy ULID; member `{case_id}`:
  UUID or legacy ULID (§19.3).
- Keyset pagination: `limit` (1–100, default 50), opaque `cursor` bound to
  the filters. Lists fetch one extra row (`quack_db::Page`), so
  `next_cursor` is null on an exactly full last page.
- Writes under `/api/guilds` and `/api/me` need `Idempotency-Key` (8–200
  printable ASCII; else `428`) via `operation_keys` (§4.3).
- Edits send the read `version` as `expected_version`
  (`expected_context_revision` for context); stale → 409.
- Text fields (`message`, `text`, `*_text`) are `quack-text` sentences.
  Responses carry `traceparent`; errors carry `error.trace_id`.

### 14.2 Routes

"staff:X" = session, membership in an installed server (current facts,
§2), capability X, and the 2FA rule. `…` = `/api/guilds/{guild_id}`.
Shapes: `contracts/http/openapi.json`. Rates §14.7.

| # | Method | Path | Auth | Idem. | Rate |
| --- | --- | --- | --- | --- | --- |
| 1 | GET | `/api/auth/login` | public | — | sign_in |
| 2 | GET | `/api/auth/callback` | public | — | sign_in |
| 3 | GET | `/api/auth/me` | session | — | read |
| 4 | POST | `/api/auth/logout` | session + CSRF | naturally | write |
| 5 | GET | `/api/guilds` | session | — | read |
| 6 | GET | `…/me` | session + member of server | — | read |
| 7 | GET | `…/health` | staff:manage_settings | — | read |
| 8 | GET | `…/statistics` | staff:moderate | — | read |
| 9 | GET | `…/directory/members` | staff:moderate | — | read |
| 10 | GET | `…/directory/channels` | staff:manage_settings | — | read |
| 11 | GET | `…/directory/roles` | staff:manage_settings | — | read |
| 12 | GET | `…/rules` | staff:moderate or manage_rules | — | read |
| 13 | POST | `…/rules` | staff:manage_rules | yes | write |
| 14 | GET | `…/rules/{rule_id}` | staff:moderate or manage_rules | — | read |
| 15 | PUT | `…/rules/{rule_id}` | staff:manage_rules | yes | write |
| 16 | POST | `…/rules/{rule_id}/archive` | staff:manage_rules | yes | write |
| 17 | POST | `…/rules/{rule_id}/restore` | staff:manage_rules | yes | write |
| 18 | GET | `…/rules/{rule_id}/export` | staff:manage_rules | — | read |
| 19 | POST | `…/rules/import` | staff:manage_rules | yes | write |
| 20 | GET | `…/cases` | staff:moderate | — | read |
| 21 | POST | `…/cases` | staff:moderate | yes | case_create |
| 22 | GET | `…/cases/{case_ref}` | staff:moderate | — | read |
| 23 | POST | `…/cases/{case_ref}/void` | staff:moderate | yes | write |
| 24 | PUT | `…/cases/{case_ref}/context` | staff:moderate | yes | write |
| 25 | POST | `…/cases/{case_ref}/evidence` (1–10 links) | staff:moderate | yes | write |
| 26 | GET | `…/cases/{case_ref}/evidence/{file_id}` (302) | staff:moderate | — | read |
| 27 | GET | `…/members/{member_id}/summary` | staff:moderate | — | read |
| 28 | GET | `…/actions` (default `failed`) | staff:moderate | — | read |
| 29 | POST | `…/actions/{action_id}/retry` | staff:moderate | yes | action_control |
| 30 | POST | `…/actions/{action_id}/dismiss` | staff:moderate | yes | action_control |
| 31 | POST | `…/actions/{action_id}/undo` | staff:moderate | yes | action_control |
| 32 | GET | `…/appeals` | staff:moderate | — | read |
| 33 | GET | `…/appeals/{appeal_id}` | staff:moderate | — | read |
| 34 | POST | `…/appeals/{appeal_id}/decision` | staff:moderate | yes | write |
| 35 | GET | `…/mod-log` (§9.6) | staff:moderate | — | read |
| 36 | GET | `…/settings` | staff:manage_settings | — | read |
| 37 | PATCH | `…/settings` | staff:manage_settings (+ manage_staff_roles for moderator roles) | yes | write |
| 38 | POST | `…/settings/starter-notice/acknowledge` | staff:manage_settings | yes | write |
| 39 | POST | `…/modules/{module}/enable` | staff:manage_settings | yes | write |
| 40 | POST | `…/modules/{module}/disable` | staff:manage_settings | yes | write |
| 41 | GET | `…/modules/tickets` | staff:manage_settings | — | read |
| 42 | PUT | `…/modules/tickets/settings` | staff:manage_settings | yes | write |
| 43 | GET | `…/modules/activity-log` | staff:manage_settings | — | read |
| 44 | PUT | `…/modules/activity-log/settings` | staff:manage_settings | yes | write |
| 45 | GET | `…/modules/honeypot` | staff:manage_settings | — | read |
| 46 | PUT | `…/modules/honeypot/settings` | staff:manage_settings | yes | write |
| 47 | GET | `…/tickets` (`state` comma-separated) | staff:moderate | — | read |
| 48 | GET | `…/tickets/{ticket_id}` | the ticket's member, or staff:moderate | — | read |
| 49 | GET | `…/tickets/{ticket_id}/transcript` | the ticket's member, or staff:moderate | — | read |
| 50 | POST | `…/tickets/{ticket_id}/close` | the ticket's member, or staff:moderate | yes | write |
| 51 | GET | `/api/me/cases` | member | — | read |
| 52 | GET | `/api/me/cases/{case_id}` | member | — | read |
| 53 | GET | `/api/me/cases/{case_id}/evidence/{file_id}` (302) | member | — | read |
| 54 | POST | `/api/me/cases/{case_id}/appeal` | member | yes | member_write |
| 55 | POST | `/api/me/cases/{case_id}/appeal/information` | member | yes | member_write |
| 56 | GET | `/livez` | public | — | none |
| 57 | GET | `/readyz` | public | — | none |
| 58 | GET | `…/cases/preview` | staff:moderate | — | read |
| 59 | GET | `…/rules/{rule_id}/versions/{version}` | staff:moderate or manage_rules | — | read |

`{module}` is `activity-log` in URLs, `activity_log` in JSON. Member routes
find the case by ID and member in one query (else `404`); ticket routes
give the member a projection without staff IDs. Route 21 pins the version
route 58 previewed (`409 rule_changed`). Routes 23 and 25 answer the case
plus the bot's sentence.

### 14.3 Wire types

`contracts/http/openapi.json` defines every shape (§14.12). Not visible
there: `Capabilities` are effective after the 2FA rule (`mfa_blocked` =
held back for lack of 2FA); `can_retry`/`can_dismiss`/`can_undo` use the
Discord control logic (§5.8); `MemberCase` replaces message-link values
with `member.context.link_saved`; `Health.channels` covers core settings
and enabled modules' channels (entry and honeypot checked only for
Quack's access); module pages carry `version` and empty `problems`;
`Ticket.transcript` is null after housekeeping.

**Statistics** (route 8): `[from, to)`, default the last calendar month,
1 s–366 days (else `api.range_invalid`); counted by current state per UTC
day, rule ("Imported"), valid/voided, source, action kind and state,
appeal state, and log kind, in one `REPEATABLE READ` read-only
transaction. No per-moderator ranking.

### 14.4 Authentication

Discord OAuth 2.0 code flow with PKCE (S256), scopes `identify guilds`,
redirect `<QUACK_PUBLIC_URL>/api/auth/callback`. Cookies `__Host-quack_oauth`
and `__Host-quack_session` (`Secure`) on `https`, unprefixed without
`Secure` on an `http` loopback URL; `HttpOnly; SameSite=Lax`.

1. **Login** (route 1). `redirect_to` (≤ 2,048) must resolve to the public
   origin (path, query, fragment kept), else `/guilds`. Store hashed
   `state` and browser nonce, PKCE verifier, and `redirect_to` in
   `oauth_states` (10 min); nonce cookie `Max-Age=600`; 302 to Discord.
2. **Callback** (route 2). Delete-and-return the state row matching the
   cookie's nonce, unexpired (single use, same browser); none →
   `/login?error=failed`; `error` → `/login?error=denied`. Exchange (10 s,
   ≤ 1 MiB, no redirects), `GET /users/@me`, then one transaction: upsert
   `discord_accounts` (2FA and `mfa_checked_at`), create the session, drop
   the browser's old one; 302 to `redirect_to`. Metric
   `quack.auth.sign_ins{quack.sign_in.outcome}`.
3. **Session.** 32 random bytes in the cookie, `SHA-256` stored, with a
   CSRF token, the Discord token (AES-256-GCM under
   `QUACK_SESSION_SECRET`; route 5 only), the key fingerprint, and
   `expires_at = now + min(token lifetime, 7 days)`; no refresh. Lookup by
   hash, unexpired, current fingerprint (rotation signs everyone out);
   `last_seen_at` touched at most every 5 minutes.
4. **Cookie.** `Max-Age` = seconds until `expires_at`.
5. **Logout** (route 4) deletes the row and clears the cookie.

Staff membership comes from current facts (§2). Route 5 merges the
account's servers (cached 60 s) with installed servers and capabilities
(`can_install` = not installed + Manage Server).

### 14.5 CSRF and origin

Every POST, PUT, PATCH, DELETE under `/api`: `Origin` must equal the public
origin (or, without it, `Sec-Fetch-Site: same-origin`), else
`403 origin_rejected`; `X-CSRF-Token` must match the session's, else
`403 csrf_rejected`. No CORS.

### 14.6 Errors

Envelope `{"error": {code, message, trace_id, fields?, problems?,
retry_after_seconds?}}`; `message` is always a `quack-text` sentence.

| Status | `code` | Message id | Extra |
| --- | --- | --- | --- |
| 400 | `validation_failed` | `api.validation_failed`, `api.value_invalid`, `api.range_invalid`, `api.rule_file_wrong_format`, `api.rule_file_unsupported_version`, `error.no_links` | `fields` |
| 400 | `invalid_json` | `api.invalid_json` | |
| 401 | `sign_in_required` | `api.sign_in_required` | |
| 401 | `session_expired` | `api.session_expired` | |
| 403 | `origin_rejected` | `api.origin_rejected` | |
| 403 | `csrf_rejected` | `api.csrf_rejected` | |
| 403 | `missing_capability` | `error.missing_capability.<capability>` | |
| 403 | `mfa_required` | `error.mfa_required` | |
| 404 | `not_found` | `api.not_found`, `error.{case,appeal,rule}_not_found`, `error.{case,appeal}_not_found_dashboard` | |
| 404 | `action_not_found` | `error.action_not_found` | |
| 404 | `server_not_found` | `api.server_not_found` | |
| 404 | `file_unavailable` | `api.file_unavailable` | |
| 409 | `idempotency_key_reused` | `api.idempotency_key_reused` | |
| 409 | `rule_changed` | `api.rule_changed`, `error.rule_changed` (case creation) | |
| 409 | `context_changed` | `error.context_changed` | |
| 409 | `rule_name_taken` | `api.rule_name_taken` | |
| 409 | `rule_used_by_honeypot` | `api.rule_used_by_honeypot` | |
| 409 | `required_field_on_honeypot_rule` | `api.required_field_on_honeypot_rule` | |
| 409 | `too_many_rules` | `api.too_many_rules` | |
| 409 | `settings_changed` | `api.settings_changed` | |
| 409 | `case_already_voided` | `error.case_already_voided` | |
| 409 | `case_voided` | `error.case_voided`, `appeal.submit.voided` | |
| 409 | `action_not_retryable` | `error.action_not_retryable` | |
| 409 | `action_not_dismissable` | `error.action_not_dismissable` | |
| 409 | `action_not_undoable` | `error.action_not_undoable` | |
| 409 | `appeal_changed` | `error.appeal_changed`, `appeal.information.outdated` (member) | |
| 409 | `appeal_exists` | `api.appeal_exists` | |
| 409 | `ticket_not_open` | `ticket.close.not_open` | |
| 409 | `module_not_ready` | `api.module_not_ready` | `problems` |
| 409 | `replacement_invalid` | `error.replacement_invalid` | |
| 409 | `case_numbers_exhausted` | `error.case_numbers_exhausted` | |
| 413 | `body_too_large` | `api.body_too_large` | |
| 415 | `unsupported_media_type` | `api.unsupported_media_type` | |
| 422 | `rule_archived` | `error.rule_archived` | |
| 422 | `member_refused` | `refusal.*` | `fields: [{field: "member_id", code}]` |
| 422 | `appeal_not_allowed` | `api.appeal_not_allowed` | |
| 422 | `timeout_window_passed` | `error.timeout_window_passed` | |
| 422 | `evidence_limit_reached` | `error.evidence_limit_reached` | |
| 422 | `too_many_links` | `error.too_many_links` | |
| 428 | `idempotency_key_required` | `api.idempotency_key_required` | |
| 429 | `rate_limited` | `api.rate_limited` | `retry_after_seconds`, `Retry-After` |
| 502 | `discord_unavailable` | `api.discord_unavailable` | |
| 503 | `unavailable` | `api.unavailable` | |
| 500 | `internal_error` | `api.internal_error` | |

`member_refused` codes are the §2.3 refusals. `DomainError::code()` /
`http_status()` give a domain refusal's row and
`quack_text::error::for_domain` its message (§12.3). Unknown `/api` paths
and methods are `404 not_found`.

### 14.7 Rate limits

In-process token buckets (≤ 100,000 keys per class, LRU); 429 with
`Retry-After`. `sign_in` 20 per 10 min per client IP; `read` 120/min per
account; `write` 30/min, `case_create` 20/min, `action_control` 10/min
per account + server; `member_write` 10/min per account. Client IP is the
socket peer, or the n-th `X-Forwarded-For` entry from the right with
`QUACK_TRUSTED_PROXY_HOPS = n`. Metric
`quack.http.rate_limited{quack.rate_limit.class}`.

### 14.8 Limits, timeouts, headers

Body ≤ 262,144 bytes (`413`); a panic is `500`. `GET`/`HEAD` end at 10 s
(`503 unavailable`); writes have no request deadline, so "nothing changed"
is never said about a committed write. Header and keep-alive timeouts are
axum's defaults. Statement timeout 5 s (2 s in case creation, §4.2). Every
response sets `nosniff`, `Referrer-Policy: no-referrer`,
`X-Frame-Options: DENY`, a restrictive `Permissions-Policy`, and HSTS on
`https`; `/api` and health add `no-store` and `default-src 'none'` CSP.
Text is gzip-compressed. Telemetry uses route patterns, never raw paths.

### 14.9 Health endpoints

`/livez` is 200 while serving HTTP. `/readyz` is 200 only when all are
`ok`: `database` (a pooled connection within 1 s, checked by the API),
`migrations`, `fence` (§17.5), `gateway` (`READY`/`RESUMED`), `workers`
(every dispatcher passed within 15 s; passes never wait for permits),
`shutdown`.

### 14.10 Dashboard files

From `QUACK_DASHBOARD_DIR` (must hold `index.html`): `/assets/*` cached
immutable for a year; other existing files `no-cache`; extensionless paths
outside `/api`, `/livez`, `/readyz` get `index.html` with the dashboard CSP
(`DASHBOARD_CSP` in `crates/quack-api/src/static_files.rs`); a missing file
is a plain 404; other methods 405. A file is served only when its real
path (symlinks resolved) is inside the directory.

### 14.11 Dashboard URL map

Stable paths Discord messages link to, under `/guilds/{guild_id}` (staff):
the root (from `/help`, `/setup`), `/cases` (`/case list`), `/cases/{number}`
(case card, receipt, log posts), `/members/{member_id}` (`/case history`),
`/appeals/{appeal_id}` (appeal post, `/appeals`), `/failures`, `/rules`,
`/rules/{rule_id}`, `/mod-log`, `/settings`, `/modules/{tickets|activity-log|honeypot}`.
Members: `/guilds/{guild_id}/cases/{case_id}/appeal` (DMs, appeal replies).
Go-era `/audit` and `/modules/logging` redirect; old ULID member and staff
links resolve by ULID (§19.3).

### 14.12 OpenAPI and the TypeScript client

utoipa annotations build `quack_api::openapi::document()` (OpenAPI 3.1);
`contracts/http/openapi.json` is committed and checked by
`openapi_is_current` (regenerate with `UPDATE_OPENAPI=1`). `bun run api`
in `apps/dashboard` regenerates `src/api/schema.gen.ts`; CI fails on a
diff.

## 15. Database schema

PostgreSQL 18; the schema is `crates/quack-db/migrations/`
(`0001_initial.sql` plus §15.10). Keys are UUIDv7 except `guilds` (the
server ID); snowflakes are positive `bigint`s (strings in JSON); times are
UTC. Every tenant table has `guild_id`, and children reference parents by
composite keys (`(guild_id, parent_id)`, or `(guild_id, case_id,
member_id)` where the child must be about the case's member), so no row
crosses servers. States are enums, invariants `CHECK`s; text limits are
enforced on write (§3.9). A `reject_change()` trigger makes `mod_log` and
`rule_versions` append-only, even for the owner. `serve` verifies the
migrations (applies them with `--migrate`). Lock order is §4.12. `NOTIFY`
channels: `quack_jobs` (kind), `quack_rules`, `quack_settings` (guild ID).

### 15.1 Enums

```sql
CREATE TYPE case_source            AS ENUM ('discord', 'dashboard', 'honeypot', 'imported');
CREATE TYPE void_cause             AS ENUM ('staff', 'appeal_accepted');
CREATE TYPE action_kind            AS ENUM ('timeout', 'kick', 'ban', 'remove_timeout', 'unban');
CREATE TYPE action_state           AS ENUM ('pending', 'running', 'done', 'failed', 'cancelled', 'skipped');
CREATE TYPE action_failure         AS ENUM ('quack_not_in_server', 'quack_missing_permission', 'quack_role_too_low',
                                            'member_is_owner', 'member_not_in_server', 'member_is_administrator',
                                            'timeout_window_passed', 'discord_refused', 'retries_exhausted',
                                            'unconfirmed', 'original_unconfirmed');
CREATE TYPE action_skip_reason     AS ENUM ('other_case_applies', 'changed_outside_case');
CREATE TYPE attempt_outcome        AS ENUM ('done', 'no_change', 'refused', 'not_sent', 'unknown', 'interrupted');
CREATE TYPE discord_permission     AS ENUM ('moderate_members', 'kick_members', 'ban_members');
CREATE TYPE dm_kind                AS ENUM ('case', 'appeal_update', 'ticket_closed');
CREATE TYPE dm_state               AS ENUM ('pending', 'sending', 'sent', 'failed', 'unknown', 'not_needed');
CREATE TYPE dm_failure             AS ENUM ('dms_closed', 'not_in_shared_server', 'discord_unavailable', 'discord_refused');
CREATE TYPE evidence_state         AS ENUM ('saving', 'saved', 'failed');
CREATE TYPE evidence_failure       AS ENUM ('message_deleted', 'moderator_no_access', 'quack_no_access', 'other_server', 'discord_error');
CREATE TYPE file_copy_state        AS ENUM ('pending', 'copied', 'link_only', 'too_large', 'unsupported_type', 'copy_failed');
CREATE TYPE appeal_state           AS ENUM ('pending', 'needs_information', 'accepted', 'rejected', 'closed');
CREATE TYPE appeal_event_kind      AS ENUM ('submitted', 'information_requested', 'information_provided',
                                            'accepted', 'rejected', 'closed', 'reopened');
CREATE TYPE mod_log_kind           AS ENUM ('case_created', 'case_create_refused', 'case_voided', 'case_context_edited',
                                            'evidence_added', 'action_done', 'action_failed', 'action_retry_requested',
                                            'action_dismissed', 'action_undo_requested', 'action_undo_skipped',
                                            'dm_sent', 'dm_failed', 'appeal_submitted', 'appeal_information_requested',
                                            'appeal_information_provided', 'appeal_accepted', 'appeal_rejected',
                                            'appeal_closed', 'appeal_reopened', 'rule_created', 'rule_edited',
                                            'rule_archived', 'rule_restored', 'rule_imported', 'settings_changed',
                                            'module_enabled', 'module_disabled', 'module_settings_changed',
                                            'quack_joined', 'quack_left', 'channel_setting_cleared',
                                            'log_channel_failed', 'ticket_opened', 'ticket_closed',
                                            'honeypot_case_failed', 'legacy');
CREATE TYPE mod_log_result         AS ENUM ('ok', 'failed', 'refused');
CREATE TYPE mod_log_source         AS ENUM ('discord', 'dashboard', 'quack', 'imported');
CREATE TYPE message_kind           AS ENUM ('case_receipt', 'appeal_post', 'ticket_panel', 'ticket_post', 'honeypot_warning');
CREATE TYPE message_state          AS ENUM ('active', 'gone');
CREATE TYPE job_kind               AS ENUM ('run_action', 'send_dm', 'sync_message', 'post_mod_log', 'capture_evidence',
                                            'honeypot_case', 'honeypot_cleanup', 'open_ticket', 'close_ticket',
                                            'housekeeping');
CREATE TYPE ticket_state           AS ENUM ('opening', 'open', 'closing', 'closed');
CREATE TYPE ticket_close_reason    AS ENUM ('open_failed');
CREATE TYPE transcript_partial     AS ENUM ('history', 'trimmed');
CREATE TYPE module_disabled_reason AS ENUM ('channel_deleted', 'quack_left');
CREATE TYPE activity_event         AS ENUM ('message_edited', 'message_deleted', 'messages_bulk_deleted', 'member_joined',
                                            'member_left', 'member_banned', 'member_unbanned', 'server_changed',
                                            'channel_changed');
CREATE TYPE honeypot_hit_state     AS ENUM ('pending', 'case_created', 'failed');
```

`original_unconfirmed` comes from migration `0004`; a test checks these
lines against the `quack-core` enums.

### 15.2 Servers, accounts, sessions

- `guilds`: every server ever joined, never deleted (`installed`).
- `case_counters`: per-server case number; its row lock serializes case
  creation.
- `discord_accounts`: signed-in accounts and their last 2FA reading (§2.2).
- `sessions`, `oauth_states`: hashed session IDs with the sealed Discord
  token (§14.4); one-time login states (10 minutes).

### 15.3 Rules and settings

- `rules`, `rule_versions`: identity and current version; immutable
  definitions (`reconstructed` for importer-rebuilt ones, §19.5).
- `guild_settings`, `ticket_settings`, `activity_log_settings` (+
  `activity_log_routes`), `honeypot_settings`: one row each per server,
  with `version` for optimistic checks. An enabled module has its channels
  (and the honeypot its rule).

### 15.4 Cases, actions, evidence, appeals

- `cases`: the stored decision (§4.4), context, operation key (§4.3),
  void and replacement fields, `imported` details. An imported case has
  no action and no DM; escalation counts `voided_at IS NULL AND source <>
  'imported'`.
- `actions`, `action_attempts`: originals and undos (one original per
  case; only undos are `skipped`, only originals `cancelled`); attempts
  opened before the call, at most one open per action.
- `evidence`, `evidence_files`: one snapshot per (case, message); its
  files with copy state and `copy_attempts` (§7.3).
- `appeals`, `appeal_events`: one appeal per case and its timeline.

### 15.5 Tickets

- `tickets`: state, thread, one timestamp per open and close step,
  `journal_gaps`; one live ticket per member per server.
- `ticket_messages`, `ticket_transcripts`: the journal; the saved
  transcript (≤ 8 MiB).

### 15.6 DMs, moderation log, tracked messages, jobs, operation keys, drafts, import

- `dms`: case, appeal-update, and ticket-closed DMs; `first_attempt_at`
  starts the nonce window (§10.4).
- `mod_log`: the moderation log (§9); append-only.
- `messages`: tracked messages (§10); one per owner, one ticket panel and
  honeypot warning per server.
- `jobs` (§16), `operation_keys` (§4.3; account 0 is Quack, guild 0 none),
  `case_drafts` (§11.6), `import_runs` and `import_guilds` (§19.11).

### 15.7 Honeypot hits

`honeypot_hits`: one row per honeypot post, with the settings `version`
and rule it was admitted under, its case or failure code, and cleanup
times; pruned after 30 days.

### 15.8 Ownership notes

Never deleted: cases, rule versions, the moderation log, appeals and
their events, actions and attempts, DMs, evidence. Housekeeping deletes
expired sessions, login states, operation keys, drafts, transcripts and
journal rows, dead jobs after 7 days, and honeypot hits after 30 days. A
job payload holds only IDs; the handler checks every row it loads against
the job's `guild_id`.

### 15.9 Hot-path queries

Each hot-path read has an index declared with its table (escalation:
`cases_escalation`; undo guard: `actions_member`; job claim:
`jobs_runnable`, `EXPLAIN`-tested; expiry sweeps: `*_expiry`).

### 15.10 Later migrations

- `0002`: `guild_settings.log_channel_posted_at` for §9.4's
  once-per-outage rule.
- `0003`: imported history may lack a sent DM's channel, a dismissal's
  `dismissed_by` ("A moderator"), a Discord or dashboard case's moderator
  (only with no or a `legacy:` operation key), and a ticket's entry
  channel (except while `opening`).
- `0004`: `action_failure` value `original_unconfirmed` (§5.6).
- `0005`: `evidence_files.copy_attempts` (§7.3).


## 16. Job queue

All background work is a row in `jobs`, inserted in the transaction that
needs it. The generic runner is `quack_db::jobs::Runner`; the engine and
each module supply one handler per kind.

### 16.1 Lifecycle

- **Claim** sets a fresh `claim_token`, `locked_until`, and `attempts + 1`
  from PostgreSQL's clock.
- **Fence.** Every handler transaction ends with `UPDATE jobs … WHERE id
  = $1 AND claim_token = $2 AND locked_until > clock_timestamp()`; no row
  rolls the whole transaction back and stops the handler. Domain writes
  are also compare-and-set on the state the handler read, so a handler
  that lost its lease never overwrites newer state.
- **Deadline.** Lease minus 15 s, anchored when the claim was sent.
  Before each Discord mutation the handler needs that call's deadline
  (10 s, 30 s for files) plus 5 s left, else it returns `Continue` with
  nothing opened or counted.
- **Lease sweep.** Each pass unclaims expired leases
  (`quack.job.lease_expired`); the next run resolves the half-done record
  by its own rule.
- **Budget.** Past `max_attempts`, or on `Retry` at the last attempt, the
  kind's database-only **finalize** settles the record terminally
  (§16.5). A failing finalize is tried `FINALIZE_TRIES` (3) times, then
  the job stays dead (`finalize_<error>`).

| Result | Effect |
| --- | --- |
| `Done` | Delete (or, with `rerun_at`, unclaim with `attempts = 0`). |
| `Retry { at, error }` | Unclaim, `run_at = at`, `last_error`. |
| `Defer { at }` | `Retry` that doesn't count (a busy member, a DM sending). |
| `Continue` | `Defer` at now. |
| `Dead { error }` | `dead_at = now()`: records missing or malformed. |
| `Reschedule { at }` | Periodic: `attempts = 0`, `run_at = at`. |

`rerun_at` moves any non-`Done` result earlier. A result applies only
while the claim holds (else `DbError::Stale`); any other handler error is
retried with the kind's backoff.

### 16.2 Claim

`SELECT … FOR UPDATE SKIP LOCKED` over due, live, unclaimed jobs of one
kind, `ORDER BY run_at, id LIMIT <permits>`, then the `UPDATE` above
(`crates/quack-db/src/jobs/queue.rs`, index `jobs_runnable`). A due job
skipped for a locked row is tried again after 250 ms.

### 16.3 Enqueue and coalescing

`key` is `<kind>:<owner IDs>` from `NewJob`'s constructors; payloads hold
only IDs. Enqueue upserts on `key` and sends `pg_notify('quack_jobs',
kind)`: a waiting job moves earlier, a claimed one gets `rerun_at`, a dead
one is revived with `attempts = 0`. `trace_context` keeps the first
enqueuer's; `quack.jobs.enqueued{quack.job.kind}` counts them.

### 16.4 Dispatch and wake-up

- One dispatcher per kind wakes on its `NOTIFY`, a finished job, its next
  due `run_at`, or a 2-second poll; a pass sweeps, then claims in batches
  of at most 8 while permits last.
- Permits are per kind (§16.5) and global (`QUACK_WORKER_CONCURRENCY`);
  kinds waiting for a global permit queue first in, first out.
- The listener's `Reconnected` wakes every dispatcher and reloads the
  caches (§17.6).
- Runs are `job {kind}` spans linked to `trace_context` (§18.4).
- Readiness needs every dispatcher's heartbeat within 15 s (§14.9).
  Actions are also serialized per member (§5.4).

### 16.5 Job kinds

From `JobKind::policy`; the last backoff repeats.

| Kind | Key | Limit | Lease | Backoff (after attempt n) | Max | Why repeating is safe | Finalize (database only) |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `run_action` | `run_action:<action>` | 8 | 60 s | 5 s, 15 s, 1 min, 5 min, 15 min, 1 h, 3 h | 8 | Every attempt reads Discord first (§5.1); start and settle are compare-and-set. | Open attempt → `interrupted`; action → `failed` (`unconfirmed` or `retries_exhausted`, §5.5). |
| `send_dm` | `send_dm:<dm>` | 4 | 120 s | 5 s, 30 s (only while `pending`) | 3 | One nonce per DM, reused only inside the window (§10.4). | `pending` → `failed` (`discord_unavailable`); `sending` → `unknown`; wake a waiting kick or ban (§6.2). |
| `sync_message` | `sync_message:<row>` | 4 | 60 s | 2 s, 10 s, 1 min, 5 min, 30 min | 6 | One writer per row; renders current state; write-back on the generation (§10.3). | `last_error = exhausted`; the next bump retries. |
| `post_mod_log` | `post_mod_log:<entry>` | 2 | 60 s | 10 s, 30 s, 2 min, 10 min, 1 h | 6 | Nonce = entry ID; channel pinned (§9.4). | `log_channel_failed` once per outage. |
| `capture_evidence` | `capture_evidence:<evidence>` | 4 | 120 s | 10 s, 1 min, 5 min | 4 | Snapshot stored once; each file has its state, try budget, and nonce (§7.2, §7.3). | Pending files → `copy_failed`; item → `saved` or `failed` (`discord_error`). |
| `honeypot_case` | `honeypot_case:<guild>:<member>` | 2 | 60 s | 5 s, 30 s, 2 min, 10 min | 5 | The batch's operation key; hits guarded on `pending` (§13.4.2). | Hits → `failed` (`discord_unavailable`); log `honeypot_case_failed`. |
| `honeypot_cleanup` | `honeypot_cleanup:<guild>:<message>` | 2 | 30 s | 1, 2, 4, 8, 16, 32, 64 s | 10 | Delete is idempotent (404 = done). | `cleanup_failed_at`. |
| `open_ticket` | `open_ticket:<ticket>` | 2 | 60 s | 2 s, 10 s, 30 s | 4 | Thread looked up before any create; steps check saved progress (§13.2.3). | The open fails (§13.2.2). |
| `close_ticket` | `close_ticket:<ticket>` | 2 | 300 s | 5 s, 30 s, 2 min, 10 min, 1 h | 6 | Each step records its column (§13.2.4). | §13.2.4 finalization. |
| `housekeeping` | `housekeeping` | 1 | 300 s | `Reschedule` | 1 | Deletes expired rows. | `Reschedule` in 10 min. |


Every handler first checks its server is installed; if not, an action
fails `quack_not_in_server`, a tracked message becomes `gone`, honeypot
hits fail `quack_not_in_server`, and a DM is still tried.

### 16.6 Periodic work

`housekeeping` (ensured at startup) deletes the expired rows of §15.8 in
batches of 1,000 for at most 60 s, plus journal rows over the cap
(§13.2.5), and marks private receipts with expired tokens `gone`; it
reschedules at once while rows remain, else in 10 minutes. A 60-second
task observes the database gauges (§18.2), such as `quack.jobs.ready`.

### 16.7 Shutdown

Dispatchers stop claiming (a job claimed during the switch is handed back
uncounted). Runs continue until the drain deadline (§17.5); then the
Discord client refuses new calls and the rest are dropped unsettled, to be
resolved after their leases expire.


## 17. Architecture

### 17.1 One binary

| Command | What it does |
| --- | --- |
| `quack serve [--migrate] [--discord-offline <url>]` | Everything. `--migrate` applies migrations first (else stale ones stop startup). `--discord-offline` (tests, smoke runs) skips the gateway and startup calls and sends REST to `<url>`. |
| `quack migrate` | Applies migrations. |
| `quack import-mysql [--dry-run] [--report <file>]` | One-time import (§19). |
| `quack commands sync` | Registers commands (§11.2). |

All take `--config <file>`. Exit codes: 0 success, 1 runtime failure, 2
usage or configuration error (all problems printed at once).

### 17.2 Crates

Workspace in `crates/` (edition 2024, Rust 1.100, pinned dependencies);
public surfaces in rustdoc (`cargo doc --workspace --no-deps`).

| Crate | Owns | May depend on |
| --- | --- | --- |
| `quack-core` | The pure domain: ids, rules and escalation, permissions, state machines, moderation log kinds, limits, domain errors, Discord call outcomes. No I/O. | — |
| `quack-text` | Every string a person reads, as a neutral message model; snapshot-tested. | core |
| `quack-telemetry` | OpenTelemetry setup and the typed instrument catalog (`METRICS.md`). | — |
| `quack-db` | PostgreSQL: migrations, every query (sqlx, offline data in `.sqlx/`), the job queue and runner, sessions, the singleton fence. | core, telemetry |
| `quack-discord` | Twilight: gateway, caches, REST with typed outcomes, rendering, custom ids, commands. | core, text, telemetry |
| `quack-engine` | Use cases and job handlers; defines the `Discord` port. | core, text, db, telemetry |
| `quack-tickets`, `quack-activity-log`, `quack-honeypot` | The optional modules, as service functions plus `ModuleJobs`/`ModuleHooks`. | core, text, db, engine, telemetry |
| `quack-api` | axum HTTP API, Discord sign-in, sessions, CSRF, rate limits, OpenAPI, dashboard files. | core, text, db, engine, modules, telemetry |
| `quack-bot` | Interactions and gateway events into engine calls and replies; the module adapters and `/setup`. | core, text, db, engine, discord, modules, telemetry |
| `quack-import` | The one-time MySQL importer (the only crate with sqlx's `mysql`). | core, db |
| `quack-testkit` | The fake `Discord` port, the PostgreSQL test harness, the queue driver. Dev-dependency only. | core, text, engine, db |
| `quack` | Config, CLI, the `DiscordPort` adapter, composition (`compose::App`), `serve` startup and shutdown. | everything |


Dependencies point toward `quack-core`; adapters never import each other;
the engine reaches Discord only through its `Discord` port. A trait needs
two real implementations.

- **REST**: a small `reqwest` transport over Twilight's rate limiter
  (`twilight-http-ratelimiting`), so nonces go with `enforce_nonce` and
  nothing retries on its own.
- **Gateway**: `Gateway::run` yields Quack's typed `Event`s with an
  `on_dropped` hook (§11.9); facts come from `quack_discord::facts::Facts`
  (caches under §2's rules, else REST), which `DiscordPort` wraps.
- **Modules**: no engine trait; each crate exports service functions,
  `jobs() -> ModuleJobs`, and `hooks() -> ModuleHooks`. The bot owns the
  seam (`quack_bot::Module`, one adapter per module) and `/setup`'s module
  areas.
- **Composition**: `quack::compose::App::compose` over a `Discord` port;
  `serve` passes the REST client, `cargo test -p quack --test system` the
  fake. `TestDb` needs `QUACK_TEST_DATABASE_URL` and copies a migrated
  template per test.

### 17.3 The `Discord` port

`quack_engine::Discord` (`crates/quack-engine/src/port/mod.rs`) covers
facts, actions (§5.1), messages, downloads, and channels and threads.
"Absent" is never cached; every mutation carries its
`X-Audit-Log-Reason`; creates send a nonce (§10.4). OAuth calls use
`quack-api`'s own client.

| `Outcome` | Meaning |
| --- | --- |
| `Done(T)` | Confirmed. |
| `Refused(Refusal)` | Nothing changed: a 4xx other than 429, or a 304. |
| `RetryAfter(delay)` | Rate limited before sending. |
| `NotSent(cause)` | Never sent: `Connection`, `ShuttingDown`, `Invalid`. |
| `Unknown(cause)` | May have applied: `Timeout`, `ServerError` (5xx or any other status), `Connection` after sending, `Malformed` success body. |

`RefusalKind` by JSON code: 50013 `MissingPermissions`, 50001
`MissingAccess`, 50007 `CannotDm`, 50278 `NoMutualGuilds`, 40005 or 413
`TooLarge`. The "absent" codes (10003, 10004, 10007, 10008, 10013, 10026)
count only with a 404, else `Other`. `last_error` stores
`http_<status>_<code>`.

- Deadline 10 s, 30 s for files; spans `{method} {route}` with
  `http.client.request.duration`.
- A route permit is awaited at most 10 s, else `RetryAfter` unsent.
  Buckets keep only major ids (channel, server, webhook). Interaction
  callbacks take no permit: each token is answered once, and with no
  major id the limiter would queue them behind any request without one
  (`GET /users/{id}`, opening a DM). A 429 waits its
  `retry_after` (else `Retry-After`, else 1 s). A global 429 holds all
  bot-authenticated requests (not interaction callbacks) until it ends.
- Modify Guild Member, Modify/Delete Channel, and interaction edits are
  `Done` only with a JSON object body.
- V2 channel-message edits send `"content": null, "embeds": []` so pre-V2
  messages convert.
- A closed client answers `NotSent(ShuttingDown)`, even mid-permit.

### 17.4 Dependency diagram

See the "May depend on" column of §17.2.

### 17.5 Startup and shutdown

**Startup (`quack serve`):**

1. Load and validate the configuration (exit 2 on any problem).
2. Start telemetry (§18.6).
3. Connect the pool (5 s acquire and statement timeouts, 60 s idle in
   transaction, UTC, `plan_cache_mode = force_custom_plan`); migrate with
   `--migrate`; verify (else exit 1).
4. **Singleton fence**: build the REST client, then take
   `pg_try_advisory_lock(0x517561636B)` on its own connection, retrying
   every 5 s while another instance holds it. It is checked every 5 s;
   losing it closes the Discord client at once, shuts down, and exits 1.
5. `GET /users/@me` (token, own account) and `GET /gateway/bot` (more
   than one shard: exit 1, §11.9).
6. Compose (§17.2); serve HTTP (`/livez` at once).
7. Sync commands (§11.2).
8. Start the listener and load the caches.
9. Connect the gateway; wait until every installed server loaded (≤ 30
   s); start the modules (`quack_honeypot::recover`, activity senders,
   ticket gauge, journal-gap recorder), the job runner (ensuring
   `housekeeping`), and the gauge task.
10. `/readyz` is ready with the fence, a `READY`/`RESUMED` session, and a
    pass from every dispatcher.

A signal during startup exits 0. A gateway that ends by itself (such as
close 4014) shuts down and exits 1.

**Server bootstrap** (`GUILD_CREATE`, one transaction):

1. Upsert `guilds` (`installed = true`).
2. Insert missing child rows; on a first join also create the starter
   rule (§3.7) and log `quack_joined`.
3. On a return, log `quack_joined` (`first_join = false`).
4. Reconcile channel settings (§13.1.5).

**Leaving** (`GUILD_DELETE`, or missing from `READY`): `installed =
false`, every enabled module off (`quack_left`), log `quack_left`, drop
caches; data stays, modules stay off after a re-add.

**Shutdown** (signal, lost fence, or ended gateway; one 30 s deadline, so
a grace period of at least 35 s):

1. Mark not ready.
2. Stop claiming jobs, gateway intake, and new HTTP connections.
3. Drain until 5 s before the deadline: interactions, requests, jobs, the
   activity log's batches and queue.
4. Abort the rest, close the Discord client (§16.7), close the pool and
   release the fence.
5. Flush telemetry (≤ 5 s); exit 0 (1 after a lost fence or gateway).

### 17.6 Caches and invalidation

- **Rules cache**: loaded at startup, reloaded per server on `quack_rules`;
  for pickers, autocomplete, modals, and early checks, never for a case's
  rule version (§3.1).
- **Config cache**: core and module settings, reloaded on
  `quack_settings` and after each `/setup` module change.
- A reload never replaces a newer version; `Reconnected` reloads all.
  Writes never trust a cache: they re-check facts (§2) in their
  transaction.

### 17.7 Configuration

`QUACK_*` variables, or a TOML file (`--config`) with the same names in
lowercase without the prefix; the environment wins, unknown keys are
errors, and a bad file is reported by path, line, and column only (never
the parser's message, which may quote a secret). Quack never reads `.env`.

| Variable | Type | Default | Purpose |
| --- | --- | --- | --- |
| `QUACK_DATABASE_URL` | Postgres URL | required except `commands sync` | Connection string. |
| `QUACK_DATABASE_MAX_CONNECTIONS` | integer 4–200 | 20 | Pool size; the listener and the fence use one dedicated connection each. |
| `QUACK_DISCORD_TOKEN` | secret | required for `serve`, `commands sync` | Bot token. |
| `QUACK_DISCORD_APPLICATION_ID` | snowflake | required for `serve`, `commands sync` | Application ID (also the OAuth client ID). |
| `QUACK_DISCORD_CLIENT_SECRET` | secret | required for `serve` | OAuth client secret. |
| `QUACK_PUBLIC_URL` | absolute `https` URL (`http` only for `localhost`, `127.0.0.1`, `[::1]`); no credentials, path, query, or fragment | required for `serve` | Dashboard links, OAuth redirect (`<url>/api/auth/callback`), CSRF origin, cookie names and `Secure`. |
| `QUACK_LISTEN_ADDR` | socket address | `0.0.0.0:8080` | HTTP listen address. |
| `QUACK_SESSION_SECRET` | base64, exactly 32 bytes | required for `serve` | Seals Discord tokens in sessions; rotating it signs everyone out (§14.4). |
| `QUACK_DASHBOARD_DIR` | path | `dashboard` | Built dashboard (`index.html`, `assets/`); checked at startup. |
| `QUACK_OTLP_ENDPOINT` | URL | unset | OTLP/HTTP base URL (`/v1/traces`, `/v1/metrics`, `/v1/logs` appended). Unset: no export; logs still go to stderr. |
| `QUACK_SERVICE_NAME` | string | `quack` | `service.name`. |
| `QUACK_ENVIRONMENT` | string | `development` | `deployment.environment.name`. |
| `QUACK_LOG_LEVEL` | `tracing` filter | `info` | Log filter (traces use a fixed `info` filter, §18.5). |
| `QUACK_WORKER_CONCURRENCY` | integer 1–64 | 16 | Jobs running at once across kinds. |
| `QUACK_DEV_GUILD_ID` | snowflake | unset | Register commands to this server only. |
| `QUACK_TRUSTED_PROXY_HOPS` | integer 0–5 | 0 | Proxies in front of Quack, for the client IP (§14.7). |
| `QUACK_IMPORT_SOURCE_URL` | secret MySQL URL | required for `import-mysql` | The Go database (§19). |


### 17.8 Engineering rules

See `AGENTS.md` and `[workspace.lints]` in the root `Cargo.toml`.

## 18. Telemetry

OTLP/HTTP traces, metrics, and logs (OpenTelemetry 0.33.1, `tracing`
0.1.44), all set up by `crates/quack-telemetry`.

### 18.1 Rules

- `crates/quack-telemetry/METRICS.md` (generated: `UPDATE_METRICS_DOC=1
  cargo test -p quack-telemetry metrics_doc`) is the source of truth for
  every instrument; a test checks each one named here exists there.
- Metric attributes are closed enums (`labels`); IDs only on spans and
  logs; "static names chosen in code" labels take only `&'static str`
  constants. Durations in seconds. Bounded export batches drop on overflow.

### 18.2 Where each instrument is recorded

| Instrument (METRICS.md) | Recorded in |
| --- | --- |
| `quack.interaction.ack.duration`, `quack.interaction.duration`, `quack.interaction.errors` | `quack-bot` router: on ack; after the final reply; on a refused or failed answer. |
| `quack.cases.created`, `quack.case.create.duration`, `quack.case.level`, `quack.cases.refused`, `quack.cases.voided` | engine `apply_rule` (after commit; refusals from every source) and `void_case`. |
| `quack.action.attempts`, `quack.action.retries`, `quack.actions.finished`, `quack.action.completion.duration`, `quack.action.undo.skipped` | engine `actions::run` and `actions::finalize`, in their settle. |
| `quack.dm.outcomes`, `quack.dm.send.duration`, `quack.appeal.transitions`, `quack.appeal.decision.duration`, `quack.evidence.captures`, `quack.evidence.files`, `quack.evidence.bytes`, `quack.modlog.posts`, `quack.message.syncs`, `quack.message.sync.lag` | the engine's `dms` (and void, `not_needed`), `appeals` (after commit), `evidence::capture`, `modlog::post`, `messages::sync`. |
| `quack.modlog.entries`, `quack.jobs.enqueued` | `quack-db`, after commit. |
| `quack.jobs.started`, `quack.job.queue.wait`, `quack.job.duration`, `quack.job.lease_expired` | the job runner. |
| `quack.jobs.ready`, `quack.jobs.oldest_ready.age`, `quack.jobs.dead`, `quack.actions.failed`, `quack.appeals.open`, `quack.sessions.active`, `quack.tickets.open` | gauge task (jobs with the claim's predicates, §16.6). |
| `http.client.request.duration`, `quack.discord.rate_limit.hits`, `quack.discord.rate_limit.wait` | `quack-discord` REST and `quack-api` OAuth clients; one observation per limiter wait; a request that never left is `not_sent`. |
| `quack.gateway.event.duration`, `quack.gateway.heartbeat.latency`, `quack.gateway.connection.events`, `quack.gateway.connected`, `quack.gateway.guilds` | gateway loop and `quack-bot`. |
| `quack.queue.depth`, `quack.queue.dropped`, `quack.cache.size`, `quack.cache.lookups`, `quack.cache.evictions` | every bounded queue (event and lifecycle queues, §11.9; interaction pool; activity posts) and cache (§11.10). |
| `http.server.request.duration`, `http.server.active_requests`, `quack.auth.sign_ins`, `quack.http.rate_limited` | `quack-api` (a drop guard counts cancelled requests). |
| `db.client.operation.duration`, `db.client.connection.*`, `quack.db.transaction.duration`, `quack.db.transaction.retries` | `quack-db` query wrapper and `Db::begin` guard; a CTE by its verb; unreadable row → `error.type` `decode`. |
| `quack.tickets.opened`, `quack.tickets.closed`, `quack.ticket.transcript.size`, `quack.activity_log.events`, `quack.honeypot.hits`, `quack.honeypot.cleanups` | the module crates. |
| `quack.runtime.*`, `process.*`, `quack.build.info` | telemetry crate, every 30 s (`quack.runtime.*` once `serve` starts). |

### 18.3 Label mapping

How domain values map onto the crate's generic labels:

| Domain value | Label value |
| --- | --- |
| `ActionFailure` | `quack.reason`: `quack_missing_permission` → `missing_permission`; `quack_role_too_low` → `role_hierarchy`; `member_is_owner` → `member_is_owner`; `member_not_in_server` → `member_not_found`; `quack_not_in_server`, `member_is_administrator` → `not_allowed`; `timeout_window_passed` → `invalid`; `discord_refused` → `discord`; `retries_exhausted` → `retries_exhausted`; `unconfirmed`, `original_unconfirmed` → `timeout` |
| `DmFailure` | `quack.reason`: `dms_closed` → `dms_closed`; `not_in_shared_server` → `no_shared_server`; `discord_unavailable` → `retries_exhausted`; `discord_refused` → `discord` |
| `MemberRefusal`, missing capability, 2FA | `quack.reason`: `role_hierarchy` (role checks), `member_not_found`, `member_is_owner`, `missing_permission` (Quack's permission, capability), `not_allowed` (self, bot, administrator, 2FA) |
| Appeal events `submitted` / `information_requested` / `information_provided` / `accepted` / `rejected` / `closed` (staff or by void) / `reopened` | `quack.appeal.transition`: `submit` / `request_info` / `reply` / `accept` / `reject` / `close` / `reopen` |
| Honeypot: case created or burst joined / ignored (staff, bot, exempt) / failed (refusal, policy changed, Discord) | `quack.honeypot.outcome`: `actioned` / `exempt` / `failed`; dropped events are `quack.queue.dropped` |
| Log channel post: posted / moved, no channel / refused, exhausted / unknown after the window | `quack.outcome`: `done` / `skipped` / `failed` / `unknown` |
| Tracked message sync: edited or posted / gone / exhausted or refused | `quack.outcome`: `done` / `skipped` / `failed` |

### 18.4 Spans

Each unit of work is a span (OTel semantic-convention names); DB queries and
Discord calls are child spans. Span status and events carry codes and error
classes, never Discord or SQL error text.

| Span | Kind | Attributes |
| --- | --- | --- |
| `interaction {kind} {command}` (root) | SERVER | `quack.interaction.kind`, `quack.command`, `quack.guild.id`, `quack.actor.id`, `quack.interaction.id`, `quack.interaction.outcome`; events `acked`, `replied`, `refused` |
| `gateway {event}` | CONSUMER | `quack.gateway.event`, `quack.guild.id` |
| `{method} {route}` (API; root, incoming `traceparent` ignored) | SERVER | `http.request.method`, `http.route`, `http.response.status_code` |
| `job {kind}` (root, **linked** to the enqueuer's trace) | CONSUMER | `quack.job.kind`, `quack.job.id`, `quack.job.attempt`, `quack.job.max_attempts`, `quack.job.queue_wait`, `quack.guild.id`, `quack.job.outcome` |
| `{operation} {table}` (DB) | CLIENT | `db.system.name`, `db.operation.name`, `db.collection.name`, `db.query.summary`, `error.type` |
| `{method} {url.template}` (Discord) | CLIENT | `url.template`, `http.response.status_code`, `quack.discord.outcome`, `quack.discord.rate_limit.wait`; event `rate_limited` |
| `use_case {name}` | INTERNAL | `quack.guild.id`; domain events `case.created`, `case.voided`, `action.attempt`, `dm.outcome`, `appeal.transition`, `evidence.captured`, `message.synced`, `mod_log.entry` |

**Propagation into jobs**: `enqueue` stores the W3C `traceparent` in
`jobs.trace_context` (a coalesced enqueue keeps the first); the job span
links to it, so a retry hours later is a short trace pointing at its cause.

### 18.5 Logs

- To **stderr** (JSON unless a terminal) and OTLP with trace IDs;
  `QUACK_LOG_LEVEL` filters logs, spans export at `info` regardless; the
  OTLP bridge drops `opentelemetry*`, `hyper`, `reqwest`, `h2`, `tonic`,
  `tower` events. `error` needs an operator; `warn` degraded but handled.
- **Never logged** (nor on spans): tokens, secrets, the import source URL,
  OAuth codes, session IDs, CSRF and interaction tokens, any member or
  staff text (messages, evidence, reasons, context, appeals, transcripts,
  DMs), IP addresses, Discord error bodies, config parser messages (§17.7).
  Secrets are `Secret<T>`; requests log the route pattern only.

### 18.6 Resource and export

`service.name` (`QUACK_SERVICE_NAME`), `service.version`,
`service.instance.id` (`<hostname>-<pid>`), `deployment.environment.name`
(`QUACK_ENVIRONMENT`). OTLP/HTTP protobuf to `QUACK_OTLP_ENDPOINT` (unset:
none), 5 s timeout, metrics every 30 s (cumulative), sampling always on.
Shutdown flushes with at most 5 s after the drain (§17.5).

### 18.7 Dashboards and alerts to build

Dashboards: moderation, reliability (jobs, failed actions, sync lag),
Discord (REST, rate limits, gateway, acks), API and database, runtime.
Alerts (counter increases; gauges report explicit zeros):

| Alert | Condition |
| --- | --- |
| Down | `/readyz` failing or `quack.gateway.connected` = 0 for 2 minutes; no `quack.build.info` series for 5 minutes. |
| Slow acknowledgements | p99 `quack.interaction.ack.duration` > 1.5 s for 10 minutes, or any observation above 3 s. |
| Job backlog | `quack.jobs.oldest_ready.age{quack.job.kind="run_action"}` > 120 s, or any kind > 600 s, for 5 minutes. |
| Jobs failing | `quack.job.lease_expired` increases on 3 consecutive 5-minute windows; `quack.jobs.dead` > 0 for `run_action`, `send_dm`, or `close_ticket`; `quack.actions.failed` grows by 10 in an hour. |
| Unconfirmed effects | `quack.dm.outcomes{quack.dm.outcome="unknown"}` grows by 5 or more in an hour, or `quack.action.attempts{quack.action.attempt.outcome="unknown"}` by 20 or more. |
| Limits and drops | `quack.discord.rate_limit.hits{quack.discord.rate_limit.scope="global"}` or `quack.queue.dropped` increases in 5 minutes. |
| API and database | 5xx share of `http.server.request.duration` > 2% for 10 minutes; p95 `db.client.connection.wait_time` > 100 ms, any `db.client.connection.timeouts`, or failed operations > 10/min, for 5 minutes. |


## 19. MySQL to PostgreSQL import

`quack import-mysql` (`quack-import`, the only crate with sqlx's `mysql`)
copies the Go MySQL database into PostgreSQL once, at cutover. **Nothing
uncertain is replayed** (it becomes "couldn't confirm"); **legacy data is
kept or reported**, never silently changed; **deterministic** (one
snapshot, `evaluated_at` = snapshot time, fixed ID mapping).

### 19.1 Preconditions

- Go stopped and fenced (§19.12 step 3); source read over
  `QUACK_IMPORT_SOURCE_URL` (read-only) in one consistent snapshot, UTC.
- Ledger `1–3`, `1–4`, or `1–5` (`baseline`, `audit_mirror_deliveries`,
  `launch_announcement`, `staff_roles`, `discord_user_mfa`). The source
  manifest (`quack_import::manifest`) names each column's fate; a missing
  or unexpected column aborts.
- PostgreSQL migrated; the importer holds the singleton fence (§17.5).

### 19.2 Command and modes

`QUACK_IMPORT_SOURCE_URL=mysql://… quack import-mysql [--dry-run] [--report
report.json]`. `--dry-run` writes each server in a rolled-back transaction
and keeps only its `import_runs` row. Otherwise one transaction per server
(with its `import_guilds` row and jobs) plus one for `discord_accounts`.

### 19.3 Identifier mapping

- **ULID → UUID** (`quack_core::ids::ulid_to_uuid`, order preserving):
  `T (48-bit ms) ‖ 0b0111 ‖ R[0..12) ‖ 0b10 ‖ R[12..74)`; unique per table
  or abort. The runtime uses it for legacy IDs in custom IDs and URLs
  (§11.5, §14.1), then checks ownership.
- **Snowflakes**: decimal in `1..=i64::MAX`; empty and `quack-system` →
  NULL; else `invalid_snowflake` (required → row and dependents skipped).
- Out-of-range numbers abort; `''` → NULL; over-limit text kept
  (`over_limit`); bad characters → U+FFFD (`normalized`); invalid JSON
  aborts the server. Ties: timestamp, then source ID.

### 19.4 Table mapping

| Go table | New table(s) | Mapping |
| --- | --- | --- |
| `guilds` | `guilds`, `case_counters` | `discord_guild_id` → `guild_id`; `name`; `icon_url` → `icon_hash` (the CDN URL's hash segment, else NULL); `owner_discord_user_id` → `owner_id`; `is_active` → `installed`; `created_at` → `first_joined_at`; `left_at` NULL; `mfa_required = false` until the next `GUILD_CREATE`. `case_counters.last_number = max(case_number)`. |
| `guild_settings` | `guild_settings` | Role lists (comma-separated) → `bigint[]` (deduplicated; invalid entries reported; over 25 aborts the server); `audit_mirror_channel_discord_id` → `log_channel_id`; `appeal_queue_channel_discord_id` → `appeal_channel_id`; `managed_evidence_channel_discord_id` → `evidence_channel_id`; `appeal_rejoin_url` → `appeal_invite_url` (re-validated; invalid → NULL, reported); `notification_introduction`/`_footer` → `dm_intro`/`dm_footer`; `starter_policy_template_id` → `starter_rule_id`; `starter_policy_notice_acknowledged_at` → `starter_notice_acknowledged_at`. Dropped: `appeal_review_reason_required`, `starter_policy_notice_pending`, `launch_announced_at`. `version = 1`. |
| `discord_user_mfa` | `discord_accounts` | `discord_user_id` → `account_id`; `mfa_enabled`, `mfa_checked_at` kept. |
| `case_templates` + context fields + levels + level actions | `rules`, `rule_versions` | §19.5. |
| `cases` | `cases` | §19.6. |
| `case_action_executions` | `actions` | §19.7. |
| `case_action_attempts` | `action_attempts` | `attempt_number` → `number`; `started_at`, `finished_at`. Outcome: `succeeded` → `done` (`no_change` when the response says `reversal_noop`); an attempt with no `finished_at` that did not succeed, or `running`, or `lease_expired` → `interrupted`; `failed` → `not_sent` or `refused` only for a Go error class on the allowlists in `quack-import`'s `actions::state` (`rate_limited`, `discord_unavailable`, … → `not_sent`; `permission_or_hierarchy_denied`, `validation_failed`, … → `refused`); a missing or unrecognized code → `unknown`. An attempt with an invalid number is not written but still counts for §19.7. HTTP status and Discord code NULL. |
| `case_notifications` | `dms` (kind `case`) | `pending`, `prepared`, `claimed` → `pending` (sent after cutover). `sending` → `unknown`. `sent` with its delivery message → `sent` (the channel may be unknown); `sent` without the message → `unknown` (`unconfirmed_dm`). `failed` + `case_voided` → `not_needed`; `delivery_outcome_unknown` → `unknown`; `delivery_deferred` → `pending`; other `failed` → `failed` (`…permission_or_hierarchy_denied` → `dms_closed`, `dm_prepare_failed` → `discord_unavailable`, else `discord_refused`). A missing notification row is never invented. |
| `case_evidence_snapshots` | `evidence` | `captured`, `uploaded` → `saved`; `deleted` → `failed`/`message_deleted`; `inaccessible` → `failed`/`quack_no_access`; `unavailable` → `failed`/`discord_error`. IDs and timestamps mapped; `embeds_json` → up to 10 summaries. Duplicate `(case, message)` rows: earliest kept, files merged, rest reported. A Go evidence copy becomes a jump link to the copy message; a Go-uploaded file becomes a file of that copy message (`uploaded_file_as_copy_message`). |
| `case_evidence_attachments` | `evidence_files` | `position` by creation order (1–10; extras reported); `url` → `original_url`. `preserved` → `copied` only when the copy channel, message, and attachment IDs all parse from `preserved_url` and agree; else `link_only`, reported. `metadata_only` → `link_only`. |
| `case_events` | — | Not imported; timelines are rebuilt from rows holding the same facts. Counts reported. |
| `case_publications` | `messages` (`case_receipt`) | The latest row with a message ID; `wanted_revision = posted_revision = 1` (old receipts are not re-rendered; old buttons resolve through §11.5). |
| `appeals` | `appeals` | Same states. The member must equal the case's (else skipped); `reviewed_by…` → `decided_by`; `reviewed_at` → `decided_at`; `first_decided_at` = earliest decision event. NULL `case_id` → skipped. |
| `appeal_events` | `appeal_events` | `information_submitted` → `information_provided`, other types the same; staff with empty actor → NULL; `body` → `text` (empty → "(no text)"); a close caused by a void → `closed_by_void`. |
| `appeal_notifications` | `dms` (`appeal_update`), `messages` (`appeal_post`) | `audience = member`: one DM per event, mapped like case notifications. `audience = staff`: the latest row with a message (or the appeal's `review_message_discord_id`) becomes the `appeal_post`, `wanted_revision = 2`/`posted_revision = 1` for `pending` or `needs_information` appeals (re-rendered at cutover), `1`/`1` otherwise. An appeal Go never posted for gets an `appeal_post` with no channel and no job, even while open (`appeal_post_unposted`); it is bound when the appeal channel is next saved (§8.3). |
| `audit_log_entries` | `mod_log` | §19.8. |
| `audit_mirror_deliveries` | — | Not imported; counts by state reported. |
| `module_configurations` (`tickets`) | `ticket_settings`, `messages` (`ticket_panel`) | `enabled`, channels, `transcript_retention_days`; the panel becomes a tracked message `2`/`1`. Enabled but missing a channel → disabled, reported. |
| `module_configurations` (`general_logging`) | `activity_log_settings`, `activity_log_routes` | Routes: `message_edit` → `message_edited`, `message_delete` → `message_deleted`, `message_bulk_delete` → `messages_bulk_deleted`, `member_join` → `member_joined`, `member_leave` → `member_left`, `discord_ban` → `member_banned`, `discord_unban` → `member_unbanned`, `guild_change` → `server_changed`, `channel_change` → `channel_changed` (invalid channel drops the route). `include_message_content`, `include_attachment_metadata` → `include_file_names`, `include_embed_metadata` → `include_embed_types`. Enabled with no route → disabled. |
| `module_configurations` (`honeypots`) | `honeypot_settings`, `messages` (`honeypot_warning`) | `channel_discord_id`, `template_id` → `rule_id`, `warning_text` (legacy default → NULL); a non-empty `disabled_reason` → off (`channel_deleted` when it says so). The warning keeps Go's warning channel and message, `2`/`1`. |
| `tickets` | `tickets` | `open` → `open`; `resolved`, `cancelled` → `closed`; owner → `member_id`; thread → `thread_id`; resolver → `closed_by`; `entry_channel_id` from the ticket settings, unknown without them (`ticket_entry_channel_unknown`). Two live tickets for a member: newest stays open. The log post → `ticket_post` `1`/`1`. A close notice showing a DM was attempted → a `ticket_closed` DM row in its mapped state. |
| `staff_members`, `ticket_events`, `ticket_member_states`, `honeypot_warning_refreshes`, `v4_import_batches`, `v4_import_sources` | — | Not imported (counts reported; v4 cases keep their source ID in `cases.imported`). |
| `ticket_message_journal` | `ticket_messages` | IDs and `body` → `text`; `author_name` NULL → "unknown"; thread must match. |
| `ticket_transcripts` | `ticket_transcripts` | As stored; `complete` from Go's flag, else `false`/`history`; rows expired at `evaluated_at` skipped. |
| `honeypot_triggers` | `honeypot_hits` | `settings_version = 1`; `created` → `case_created`; `failed`, `exempt` → `failed`; `pending` → §19.9. |
| `honeypot_message_cleanups` | `honeypot_hits.cleaned_up_at` | Incomplete cleanups for created hits enqueue `honeypot_cleanup`. |
| `quack_schema_migrations` | `import_runs.report` | Report only. |

Every source `id` (ULID) becomes the row's UUID (§19.3) and timestamps are
kept where a column exists; the manifest lists every other column.

### 19.5 Rules and their versions

1. `rules`: mapped ID, `name` (active-name clashes get " (2)" …, reported),
   `current_version`, `archived_at`. Dropped: `slug`, level names,
   `max_retries`.
2. The current version is validated as `Imported { reconstructed: false }`
   (§3.3): `reason_template` → `reason`, `case_decay_days` (0 → null) →
   `counting_window_days`, `discord_message_link` → `message_link`, the
   default level → `from_case_count = 1` (replaced, reported, by a level at
   1), `notify_user` → `send_dm`, `timeout_user`/`kick_user`/`ban_user` →
   `timeout`/`kick`/`ban`. Over-limit sizes kept; structural failure aborts.
3. Older versions used by cases are rebuilt from their snapshots
   (`reconstructed = true`, at most 32,767 levels; earliest case wins,
   `conflicting_snapshots`). A template that is gone or another server's
   becomes an archived rule rebuilt from cases (`rule_rebuilt_from_cases`).

### 19.6 Cases

| Column | Source |
| --- | --- |
| `id`, `number`, `member_id`, timestamps | mapped `id`, `case_number`, `target_discord_user_id` |
| `member_joined_at` | NULL (a kick then compares with `created_at`, §5.1) |
| `source` | same; `v4_import` → `imported` |
| `moderator_id` | `moderator_discord_user_id`; NULL for honeypot and `quack-system`. A Discord or dashboard case without one is imported without it (`case_moderator_unknown`); a non-snowflake value is dropped (`case_moderator_invalid`), the case still imported (migration `0003`, §15.10). |
| `rule_id`, `rule_version`, `operation_key` | `template_id`, `template_version` (v4: NULL); `legacy:<idempotency_key>` |
| `case_count`, `level_index`, `level_from_case_count`, `action_kind`, `timeout_seconds`, `ban_delete_message_seconds`, `sends_dm` | the snapshot's selected level and `actions[0]` |
| `context` | array → object `key → value`; numbers → canonical decimal strings |
| `replaces_case_id` | checked (same server, replaced case voided, not itself, not twice), else dropped and reported |
| void columns | `validity` decides (`unknown_validity` falls back to `voided_at`; a disagreement is reported); `void_cause = appeal_accepted` when an accepted appeal exists, else `staff`; empty reason → "(no reason recorded)" |
| `imported` | v4 cases: `{v4_action, v4_moderator, v4_reason, v4_source_id}`; no DM, no action |

A non-v4 case whose snapshot lacks `selected_level` or `template` aborts
its server.

### 19.7 Actions

| Column | Source |
| --- | --- |
| `kind` | `timeout_user`, `kick_user`, `ban_user`, `remove_timeout`, `unban_user` → `timeout`, `kick`, `ban`, `remove_timeout`, `unban`; `send_dm` executions skipped (`dm_action_row`) |
| `undo_of_action_id` | `reversal_of_execution_id` (same case) |
| `timeout_ends_at` | the last successful attempt's `timeout_until`, else `created_at + duration` (`derived_timeout_end`); a duration outside 1 s–28 days uses the case's creation (`timeout_without_duration`) |
| `delete_message_seconds` | bans: the config snapshot, else the case's, else 0 |
| `state` | `succeeded` → `done`; `cancelled` → `cancelled`; `failed` → below; `pending`, `retrying`, `running` → §19.9 |
| `attempts`, `done_at`, `no_change`, dismissal | 0 (budget starts at cutover); `finished_at`; `reversal_noop`; dismissal kept, unattributed when its staff member is unknown (`dismissed_by_unknown`, "A moderator") |

Whether an execution **may have applied** is decided from every source
attempt before any is filtered (§19.4's attempt outcomes, §5.2).

| Go `last_error_code` (suffix) | New state and reason |
| --- | --- |
| `permission_or_hierarchy_denied`, `reversal_authorization_unavailable`, `reversal_permission_denied` | `failed`, `discord_refused`, HTTP 403 |
| `unknown_member_or_resource` | timeout: `failed`, `member_not_in_server`; others: `failed`, `discord_refused`, HTTP 404 |
| `validation_failed`, `discord_failure`, `invalid_action_config`, `unsupported_action` | `failed`, `discord_refused`, HTTP 400 |
| `guild_not_found` | `failed`, `quack_not_in_server` |
| `rate_limited`, `discord_unavailable`, `guild_lookup_failed` (no attempt may have applied) | `failed`, `retries_exhausted` |
| `reversal_ownership_conflict`, `reversal_provenance_unavailable` | `skipped`, `changed_outside_case` |
| `case_voided` | `cancelled` |
| anything else, or any code after an attempt that may have applied | `failed`, `unconfirmed` |

### 19.8 Moderation log

Every `audit_log_entries` row becomes kind **`legacy`** (never re-posted):
redacted `LegacyDetail` (§9.5), `success`/`failure`/`denied` →
`ok`/`failed`/`refused`, source `imported`, links cross-checked (else NULL,
reported), `trace_id` NULL. The `kind` filter (§9.6) finds them as `legacy`.

### 19.9 In-flight work at cutover

| Go state | New state |
| --- | --- |
| Execution `pending`/`retrying` on a valid case, no attempt that may have applied | Action `pending` + `run_action` job (reads Discord first, §5.4). |
| Execution `running`, or `pending`/`retrying` after an attempt that may have applied | Action `failed` (`unconfirmed`), no job; an open attempt → `interrupted`. Staff decide. |
| Original on a voided case, `pending`/`retrying`, nothing may have applied | `cancelled`. Undo executions are never cancelled (§4.7). |
| Notification `pending`, `prepared`, `claimed` | DM `pending` + `send_dm` job; a pending kick or ban waits for it (§6.3). |
| Notification `sending` | DM `unknown`, never sent. |
| Appeal member notification `pending` or deferred / `sending` or unknown | DM `pending` + job / `unknown`. |
| Appeal staff post (known existing message) | Re-rendered at cutover (`wanted_revision = 2`); an unposted appeal gets no job (§19.4). |
| Undelivered audit mirror delivery; publication with `refresh_requested` | Dropped (reported); not re-rendered (§11.5). |
| Honeypot trigger `pending` | A case with Go operation key `honeypot:<guild ULID>:<message ID>` exists → `case_created` + cleanup job; otherwise `failed` (`imported_pending`). |
| Ticket slot without a ticket; ticket `open` whose thread is gone | Dropped; stays `open` until the first `GUILD_CREATE` reconciliation (§13.1.5) closes it. |

### 19.10 Validation report

| Section | Contents |
| --- | --- |
| `source` | Version, ledger, snapshot, `evaluated_at`, table digests, fingerprint, manifest. |
| `guilds` | Per server: `imported`, `checked`, `already_imported`, `failed`. |
| `written`, `skipped` | Rows per table; skips by reason (`dm_action_row`, `null_case_appeal`, `member_mismatch`, `invalid_snowflake`, `missing_parent`, …) with ≤ 20 example IDs. |
| `quarantined` | History rows a `CHECK`, foreign-key, or `NOT NULL` constraint refused (rolled back to a savepoint); a unique violation still stops the server. |
| `over_limit`, `normalized`, `derived` | Kept as-is, characters replaced, values derived or repaired (`derived_timeout_end`, `timeout_without_duration`, `unconfirmed_dm`, `case_moderator_unknown`, `case_moderator_invalid`, `dismissed_by_unknown`, `ticket_entry_channel_unknown`, `appeal_post_unposted`, `unknown_validity`, …). |
| `renamed_rules`, `reconstructed_versions`, `conflicting_snapshots`, `dropped_default_levels`, `legacy_mod_log*`, `in_flight`, `not_imported` | §19.5, §19.8, §19.9. |
| `checks`, `problems` | Relationship checks (versions, case/server/member agreement, undos, replacements, appeals, DMs, counters, UUID uniqueness); errors that aborted a server. |

A server whose checks fail is rolled back before its `import_guilds`
marker and reported `failed`. Exit 0 only when every server imported and
every check passed.

### 19.11 Re-running

The fingerprint is SHA-256 over ledger, manifest and transform versions,
and table digests. Servers in `import_guilds` with the same fingerprint are
skipped; a different fingerprint is refused before any write (re-create
the database). No `ON CONFLICT DO NOTHING`: unique collisions are errors
naming both keys; counters use `GREATEST(last_number, …)`.

### 19.12 Cutover runbook

1. **Rehearse**: restore a production backup into a uniquely named staging
   MySQL; import into empty staging Postgres; run `quack serve` with a
   staging application in **no** production server; walk §20.6; delete.
2. **Announce** a window (under 15 minutes expected).
3. **Fence Go**: scale bot and dashboard to zero; `SHOW PROCESSLIST` shows
   none of their connections; make the MySQL account read-only.
4. **Back up MySQL** (`mysqldump --single-transaction`), verify a restore.
5. **Prepare Postgres**: create the database, `quack migrate`.
6. **Dry run** (`--dry-run --report dry.json`); review `problems`,
   `checks`, `quarantined`, `in_flight`, `over_limit`.
7. **Import** (`--report import.json`); it must exit 0; review.
8. **Decision**: nothing has reached Discord yet; rollback = start Go
   against the untouched MySQL. Record it.
9. **Point of no return: start `quack serve`** (same application). It syncs
   commands, re-renders appeal posts, ticket panels, honeypot warnings, and
   runs imported pending work. Record the time.
10. **Verify**: `/readyz`; `quack.jobs.ready` drains; in three servers
    `/case view`, `/case history`, a dashboard case, an old DM appeal link
    and appeal button, failed actions; `quack.dm.outcomes` normal.
11. **Roll forward only** after step 9; never restart Go. Keep MySQL
    read-only 30 days, then archive the dump.


## 20. Testing, development, operations, and decisions

### 20.1 Testing

CI (`.github/workflows/ci.yml`, on Namespace runner profiles), jobs in
parallel: fmt, clippy and rustdoc with `-D warnings`; `cargo test
--workspace --locked` against PostgreSQL 18 and MySQL 8.4 services (this
includes OpenAPI freshness); the dashboard's client freshness, `check`,
`test`, `build`; `cargo deny`; `docker build`. A pull request runs only the
jobs its files touch; pushes to `main` and a nightly run do all of them,
and only `main` writes the build cache pull requests restore. `ok` is the
one required check: it fails when any job failed or when change detection
itself failed. The hooks in `.githooks/` (`git config core.hooksPath
.githooks`) run fmt on commit, check the commit message, and run fmt,
clippy, and the dashboard's `check` on push for the files the push changes.
`TestDb` copies a migrated
template database per test on the server named by `QUACK_TEST_DATABASE_URL`
(required). Importer end-to-end tests need `QUACK_IMPORT_TEST_*` URLs, else
print `SKIPPED` (and fail under `CI`).

| Layer | Covers |
| --- | --- |
| `quack-core` | Rule validation, escalation and state machines (proptest), capabilities, member checks, `permissions::in_channel`, `may_have_applied`, `controls`, parsing, `ulid_to_uuid`, the §15.1 enums. |
| `quack-text` | Every message and variant (`insta`); every §12 id and §14.6 code has a sample. |
| `quack-engine` | §20.1.1 (`FakeDiscord`, queue driver). |
| `quack-db` | Migrations, constraints, append-only triggers, the queue (concurrent claims, leases, fences, budgets, coalescing). |
| `quack-discord` | Components V2 JSON and budgets, custom IDs, REST outcome classification (`httpmock`), command definitions and sync diff. |
| `quack-api` | OAuth, sessions, CSRF, rate limits, error codes, idempotency, member isolation, capabilities, static files, `openapi_is_current`. |
| Modules, `quack-import` | Tickets, activity log, honeypot; every import mapping, dry run, rerun, changed source, quarantine, rollback on failed checks. |
| `quack` | `tests/serve.rs` (Discord off) and `tests/system` (the composed `App`). |

#### 20.1.1 Engine scenarios (each one test)

`FakeDiscord` keeps Discord's state (members, timeouts, bans, messages by
nonce) so a call can succeed while its answer is lost; each scenario runs at
every crash boundary it names.

| # | Area | Scenario |
| --- | --- | --- |
| 1 | Numbers | Concurrent `apply_rule` for one member and rule → consecutive numbers and counts. |
| 2 | Operation keys | Same key twice (sequential, concurrent) → one case; another request with the key → refused; two staff, one key → two cases; replay after losing `moderate` → refused; field order doesn't matter, `5` ≠ `"5"`. |
| 3 | Refusals | Each member refusal; only Quack-caused ones write `case_create_refused`. |
| 4 | Rule races | Archived / edited between cache read and transaction → `rule_archived` / the new version (`rule_changed` when pinned). |
| 5 | Kick with DM | The action waits for the DM; a failed DM releases it; a DM stuck `sending` delays it at most 120 s. |
| 6 | Void vs start | Both orders; a lost ban answer → the retry reads the marker, `done`; the void's undo reads it and unbans. |
| 7 | Crashes | Crash after an attempt opens → `interrupted`, read first; 8 unknown attempts → `unconfirmed`; a claim over budget finalizes without Discord. |
| 8 | Lost lease | The settle rolls back without touching newer state. |
| 9 | Timeout | End identical on every attempt; a longer timeout left alone (`no_change`); after the end → `timeout_window_passed`. |
| 10 | Kick | Member left and rejoined → `no_change`, no `DELETE`. |
| 11 | Undo provenance | Other case's ban → `skipped`; timeout end differs → `skipped`; ban without marker → `skipped`; voided marker → unbanned; nothing in place → `no_change`; unresolved original → reads again, then `original_unconfirmed`. |
| 12 | Staff controls | Retry of an original on a voided case refused; undo retry works; dismiss; concurrent retry and dismiss → one wins; undo offered ⇔ the undo lifts it (property test). |
| 13 | DM | Crash in `sending` inside the window resumes with the same nonce (one message); past it → `unknown`; refusal after an unknown create → `unknown`; rate limit before any unknown create → `pending`; void before start → `not_needed`. |
| 14 | Appeals | Every legal and illegal transition; submit racing a void; stale reply; accept voids; reopen on a voided case refused; two decisions → two DMs. |
| 15 | Tracked messages | Bump during sync; move during create; expired private receipt `gone`; lost create not duplicated inside the window. |
| 16 | Evidence | Caps under concurrency; re-add retries a failed item; crash between copies; every capture failure; channel stops being staff-only; upload budget across runs. |
| 17 | Drafts | Double submit, expiry, another moderator's draft, rule edited mid-draft; Try again creates exactly one case. |
| 18 | Settings | Stale `expected_version` → 409; moderator roles without `manage_staff_roles` → 403; channel checks use the locked roles. |
| 19 | Context edits | Second editor → `context_changed`; links become evidence. |
| 20 | Leave and rejoin | Modules turn off; queued work for an uninstalled server settles by §16.5. |

### 20.2 Development

Needs Rust 1.100, Docker, Bun, `sqlx-cli 0.9.0`, and a development Discord
application (both privileged intents, redirect
`<QUACK_PUBLIC_URL>/api/auth/callback`). Settings are in `.env.example`
(§17.7); export `.env` yourself for `cargo run`. `compose.yaml` (project
`quack`): `postgres` on `54418` by default, `quack` (profile `app`, `serve
--migrate` on `8080`), `telemetry` (profile `telemetry`, `grafana/otel-lgtm`,
OTLP `4318`, Grafana `3001`). Commands: `quack migrate`, `quack serve
[--migrate] [--discord-offline <url>]` (no gateway, REST to `<url>`),
`quack commands sync`, `bun run dev` in `apps/dashboard` (`:3000`, proxying
`/api` to `:8080`). Verification commands are in `AGENTS.md`.

### 20.3 Operations

- **Image**: Bun-built dashboard plus `cargo-chef` release build on
  `gcr.io/distroless/cc-debian13:nonroot` (about 138 MB),
  `/usr/local/bin/quack`, dashboard in `/usr/share/quack/dashboard`, port
  8080, command `serve`.
- **Release**: `quack migrate` (or `serve --migrate`), then roll the single
  instance; the fence (§17.5) holds the new one until the old exits. Probes
  `/livez` and `/readyz`; grace ≥ 35 s. `serve` ideally has only `SELECT,
  INSERT` on `mod_log` and `rule_versions`.
- **Discord application**: intents Server Members and Message Content;
  scopes `bot applications.commands`; permissions View Channel, Send
  Messages (and in Threads), Embed Links, Attach Files, Read Message
  History, Moderate/Kick/Ban Members, Manage Messages, Manage Channels,
  Manage Threads, Create Private Threads, View Audit Log (health shows
  gaps).
- **Backups**: continuous archiving, daily `pg_dump`, quarterly drill.
  Never hand-edit `mod_log`, `rule_versions`, `actions`, `dms`, `appeals`,
  or `cases`; use staff controls.

| Incident | What to do |
| --- | --- |
| Postgres down (`/readyz` fails, jobs stop) | Restore it; Quack resumes, nothing committed is lost. |
| Discord down or rate limiting | Afterwards review failed actions; check `unconfirmed` ones in Discord first. |
| Dead jobs (`quack.jobs.dead`) | Fix the cause; `UPDATE jobs SET dead_at = NULL, attempts = 0, run_at = now() WHERE id = …` is the only sanctioned edit. |
| Stuck "in progress" | Impossible past §16.5's budgets; check `quack.jobs.oldest_ready.age` and `quack.job.lease_expired`. |
| Fence lost or terminal gateway close (4004, 4014) | Exits 1 (§17.5); fix intents or token, let the supervisor restart. A second instance waits on the fence. |
| Leaked token, client secret, or session secret | Rotate; for the session secret also `DELETE FROM sessions`. |
| Bad migration | Never edit an applied one; write a new one. |

### 20.4 Known gaps

Numbers are stable (code cites them); merged items keep the lower number.

| # | Gap |
| --- | --- |
| 1 | One gateway shard (about 2,500 servers; `serve` refuses more) and one acting instance. |
| 2 | 2FA is only as fresh as the last dashboard sign-in; staff channel overwrites are not re-synced when staff roles change. |
| 4 | Posts already in a channel stay visible to whoever can see it later. |
| 5 | Opening a private ticket thread from the staff post needs Manage Threads. |
| 6 | The activity log is best effort (in memory; drops counted). |
| 7 | A create whose answer was lost and retried after the 60-second nonce window posts twice (staff post, log post, evidence copy, public receipt, ticket welcome); never a DM. |
| 8 | Kick and ban DMs go before the action, which may then fail. |
| 9 | An identical manual timeout or a copied ban marker can't be told from Quack's own; voiding the later of two overlapping timeouts leaves it until it ends. |
| 11 | A honeypot ban that deletes messages can delete the post before it's saved; reconstructed rule versions know only the levels cases used. |
| 13 | A worker outliving its lease (§16.1) can land an older tracked-message edit; the next bump fixes it. |
| 14 | A log-channel switch committing after the post job's second read can still race the create (§9.4). |
| 15 | An activity-log post can race a permission change during the request (§13.3.2). |
| 16 | Shared-scope or body-only `retry_after` is not published to queued peers (§17.3). |
| 17 | An interaction acknowledged after Discord's 3 s is still answered and only logged; the member may already see "interaction failed" (§11.3). |
| 18 | No dashboard directory cache (§11.10). |
| 19 | Not yet rehearsed against real Discord (§20.6). |

### 20.5 Decisions for the owner

Choices made without the owner; overrule by editing this document first.

| # | Decision |
| --- | --- |
| 1 | Server key is the Discord server ID (`bigint`), not a UUID. |
| 2 | Fixed retry budget (8 attempts over about 4.6 hours) replaces per-level `max_retries`. |
| 3 | Level names dropped: "level N (starts at case M)". |
| 4 | Rule slugs dropped; active names unique case-insensitively (the importer renames clashes). |
| 5 | New limits for new writes (imports keep and report): 10 levels, 100 active rules, names 100, description 500, void reason 1,000, DM intro 1,000 and footer 500, file copies 10 MiB, case numbers 2^53 − 1. |
| 6 | Decision reasons always required; modals prefilled. |
| 7 | Appeal reversal offers dropped: accepting voids, which queues guarded undos; a skipped undo is final. |
| 8 | Queued actions check only Quack's access when they run; staff authority is checked when staff ask. |
| 9 | Timeouts on Administrator members are refused before the case exists. |
| 10 | A timeout past its fixed end fails (`timeout_window_passed`); a longer timeout is never shortened. |
| 11 | Every case DM describes the decision ("The moderators decided to …"). |
| 12 | DM failures: `discord_unavailable` for an unopenable channel or persistent rate limit; 50007 is "Discord didn't let Quack message them"; only 50278 is "no shared server". |
| 13 | All staff reads are ephemeral. |
| 14 | Guided `/setup` replaces the `/setup` subcommands. |
| 15 | `/case add` has no `message_link` or `file` options; evidence comes from the message command, link fields, links in text, and Add evidence. |
| 16 | Edit context in Discord only for rules with at most 5 fields; the staff card doesn't page. |
| 17 | Prefilled-modal exception (Edit context, Try again, setup modals, the first `/case add` page): one authorized read or write within 300 ms before the modal (§11.3). |
| 18 | A case started from a message fills empty `message_link` fields. |
| 19 | Modals open only for rules with a required field; a rule with no fields takes free-text `details`. |
| 20 | Activity log defaults: message text, file names, embed types on; per-event routing and redaction kept; cache and delivery settings are constants. |
| 21 | Activity log posts are not durable jobs. |
| 22 | The honeypot's rule can't be archived or given a required field; Quack never adopts a rule by name. |
| 23 | The honeypot ignores every bot; refused hits keep their message; hits admitted under older settings fail. |
| 24 | Honeypot bait deletions appear in the activity log as ordinary deletions. |
| 25 | Tickets: one live ticket per member (partial unique index); open and close are durable jobs; close freezes then deletes the thread; transcripts read at most 1,000 history messages; "Fix access" kept, not logged. |
| 26 | Moderation log: reads and exports not recorded; `dm_sent` and `evidence_added` recorded, not posted; imported entries are `legacy`. |
| 27 | Log channel posts retry for about an hour (6 attempts). |
| 28 | Not imported: `case_events`, `ticket_events`, `staff_members`, `audit_mirror_deliveries`, `ticket_member_states`, `honeypot_warning_refreshes`, `v4_import_*`; uncertain in-flight effects become `unconfirmed`/`unknown`; pending honeypot hits fail. |
| 29 | Sessions last as long as the Discord token (≤ 7 days), no sliding renewal, no logout-all. |
| 30 | Sessions and idempotency are hand-written on Postgres. |
| 31 | Direct dependencies beyond the original stack list: `aws-lc-rs`, `tower-http` (`fs`, `compression-gzip`), `futures-util`, `bytes`, `url`. |
| 32 | Extra configuration keys: `QUACK_DATABASE_MAX_CONNECTIONS`, `QUACK_DASHBOARD_DIR`, `QUACK_ENVIRONMENT`, `QUACK_TRUSTED_PROXY_HOPS`, `QUACK_IMPORT_SOURCE_URL`. |
| 33 | The `Discord` port's implementation lives in the binary (`DiscordPort`). |
| 34 | Module HTTP routes live in `quack-api`. |
| 35 | Span names follow the OTel semantic conventions (§18.4). |
| 36 | Operation-key replays return the current authorized representation with the original status. |
| 37 | Settings PATCH needs `expected_version` only when role lists change. |
| 38 | Legacy buttons: old appeal, review, and ticket buttons work; old case buttons open the case; others answer `interaction.legacy_button`. Appeal posts, ticket panels, honeypot warnings are re-rendered; old receipts are not. |
| 39 | Dashboard paths `/audit` → `/mod-log` and `/modules/logging` → `/modules/activity-log`, with redirects. |
| 40 | Jobs FIFO per kind, round-robin across kinds, no per-server fairness. |
| 41 | Server health is available to `manage_settings`. |
| 42 | Members see evidence on their own case page (text and files, never authors, links, or message-link context values). |
| 43 | Members see the void reason. |
| 44 | Receipts: the public one has fixed fields and one Open case button; a private one lives 15 minutes as the ephemeral reply; the member command posts a public receipt, the message command a private one. |
| 45 | Ticket transcripts capped at 8 MiB (partial when cut). |
| 46 | Statistics default to the calendar month before `to`, read in one snapshot. |
| 47 | Leaving a server turns its modules off, and they stay off on return. |
| 48 | A kick acts only on the membership the case was about. |
| 49 | Context numbers are exact decimals (strings on the API). |
| 50 | Setup adopts its own fresh channel (same name, under 10 minutes old, Quack's overwrite) instead of creating a second. |
| 51 | The dashboard CSP is the Go server's, including the visitors.now analytics origins. |

### 20.6 Live rehearsal checklist

Tests use `FakeDiscord` and `httpmock`; nothing here has run against real
Discord. Run it with a staging application before release and at cutover
(§19.12 step 1).

| Area | Check |
| --- | --- |
| Install | [ ] Starter rule and notice; global commands and the `QUACK_DEV_GUILD_ID` overwrite. |
| Install | [ ] Discord accepts the per-scope command shapes (§11.2): no `contexts`/`integration_types` in guild scope, context commands without `description`, modal selects without `disabled`. |
| Install | [ ] `/setup` creates channels with overwrites, adopts its own fresh channel, re-renders after module changes; deleting a configured channel; leave and rejoin. |
| Permissions | [ ] Owner, Administrator, Manage Server, Moderate Members before and after moderator roles, a role without moderation permissions, rules manager, former moderator, every member check. |
| Permissions | [ ] Server 2FA with and without confirmation; `permissions::in_channel` matches Discord in threads and for timed-out members. |
| Cases | [ ] Warning, timeout, kick, ban from Discord and dashboard; public and private receipts (also after 15 minutes); multi-page `/case add`. |
| Cases | [ ] The 300 ms prefilled-modal deadline (§11.3) holds at production database latency. |
| Cases | [ ] Evidence with and without an evidence channel; DMs open and closed; void, undo, the undo guard with two cases, retry, dismiss. |
| Cases | [ ] A hand-applied ban and timeout survive voiding; a re-ban of a banned member: does Discord replace the stored reason (unban provenance reads the marker)? |
| Cases | [ ] A ban retried after a lost answer sends `delete_message_seconds` 0. |
| DMs | [ ] Discord answers 50278 for a DM with no mutual server. |
| Appeals | [ ] From a DM after a ban; every decision; the appeal post; appeal DMs. |
| Legacy | [ ] Go button ids decode on real old messages (§11.5); a V2 edit with `"content": null, "embeds": []` converts a pre-V2 appeal post. |
| Tickets | [ ] Open, freeze, delete; lookup by `owner_id` over archived private threads within the 10-page cap; an archived thread's 400 is recovered by unarchive-and-lock. |
| Modules | [ ] Activity log routes and privacy flags; honeypot case, burst, cleanup; the log channel. |
| Protocol | [ ] The 60-second nonce window: a lost response (network blocked after the request) for a DM, a log post, and an evidence copy returns the first message; a DM found `sending` after a crash is resent with its nonce and not duplicated. |
| Protocol | [ ] Classification per §17.3: 304 and 4xx (not 429) refusals; `Unknown …` codes absent only with 404; JSON-object success bodies. |
| Protocol | [ ] A global 429 holds bot requests but not interaction callbacks or webhooks; route buckets keyed by major IDs match Discord's. |
| Protocol | [ ] A malformed interaction is answered from its envelope; acks stay under 3 s with 64 working and 256 in flight; an unreadable gateway frame distrusts the cache until `READY`. |
| Operations | [ ] `SIGTERM` with jobs in flight, nothing runs twice; a second instance waits; fence loss exits 1; the 4014 disallowed-intents close stops `serve` with exit 1. |
| Operations | [ ] Probes, the 35-second grace, telemetry in `grafana/otel-lgtm`. |
| Import | [ ] The full runbook (§19.12) on a production copy, staging application in no production server. |
