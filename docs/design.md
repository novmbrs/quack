# Quack design

This document is the single source of truth for Quack: what it does, how the
Rust code does it, how it is configured, deployed, observed, and migrated from
the Go version. Implementers code from this document without reading the Go
code. If code and this document disagree, one of them is wrong: fix the code,
or change this document first when the change is intentional, in the same
commit.

Normative words: **must** and **never** are requirements; **may** is a
permitted option. Every limit is a number in [Limits](#39-limits). Every
user-facing sentence is in the [Message catalog](#12-message-catalog).

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

> Admins define the server's moderation rules. Moderators apply those rules.
> Quack chooses and carries out the configured result.

A server's rules managers build **rules**, one per kind of problem ("Spam",
"Harassment"), never one per punishment. Each rule has **levels** keyed on
**case count**. A moderator applies a rule to a member. Quack counts that
member's counted cases for the rule, picks the level, records the **case**,
runs the level's single **action** (timeout, kick, or ban, if any), sends at
most one **DM** about the case, and writes the **moderation log**. Members can
**appeal** from the dashboard or from the DM. Discord and the dashboard share
the same behavior; Discord stays the authority for identity, roles, and
permissions.

### 1.1 Principles

1. **The server owns its rules.** Rules managers control rule names, reasons,
   levels, actions, DM settings, appealability, and case context fields.
   Quack ships one starter rule that can be edited, exported, or archived.
2. **Moderators apply rules; they do not invent them.** A moderator never
   chooses the level, rewrites the reason, or changes the action.
3. **Discord is the authority for access.** Quack never creates or assigns
   roles and keeps no staff accounts. Staff roles are Discord roles the
   server picks.
4. **History stays understandable.** A case references the immutable rule
   version it used and stores its decision. Later rule edits never change
   old cases.
5. **Server data stays inside the server.** Nothing about one server affects
   another, including escalation.
6. **Records are kept.** Cases are voided, never deleted. Moderation log
   entries are append-only, enforced by the database.
7. **Every external effect has a durable, terminal outcome.** Each action,
   DM, and tracked message ends in a stored state; a crash never leaves work
   stuck or done twice.

### 1.2 Firm boundaries

Not part of Quack: cross-server history; public automation APIs; levels
chosen by moderators; direct punishment commands; reason overrides; more than
one action per level; severity points or weights; escalation across rules;
private staff notes on cases; hard deletion of cases or moderation log
entries; roles managed by Quack; rule editing in Discord (rules are edited
on the dashboard only). Changing a boundary starts with changing this
document.

### 1.3 Glossary

These are the only words allowed in user-facing text and public APIs. Code
identifiers follow them too.

| Term | Meaning | Never say |
| --- | --- | --- |
| **Rule** | A server-defined kind of problem with escalation levels. | template, policy |
| **Level** | One rung of a rule: starts at a **case count**, has an optional action and a DM setting. | step, tier |
| **Case count** | Number of counted cases for one member and one rule, including the new one. | offense count |
| **Case**, **case #N** | The record of a rule applied to a member. A case with no action is a warning. | infraction, record, incident |
| **Action** | What Quack does on Discord for a case: **timeout**, **kick**, **ban**; and the undo actions **remove timeout**, **unban**. | punishment, enforcement, execution |
| **Outcome** | What actually happened to an action or DM: in progress, done, failed, couldn't confirm. | status code names |
| **Attempt** | One try at carrying out an action. | |
| **Void** | Keep the case but stop counting it; undo its timeout or ban. | delete, remove, revoke |
| **Archive** (a rule) | Keep it but stop new use. | disable, delete |
| **DM** | The one direct message a member gets about a case, or about an appeal update. | notification, notice |
| **Case context** | Answers to a rule's extra fields; visible to the member. | metadata |
| **Evidence** | Saved message snapshots and files; staff-only, except that the member sees it in their own case view on the dashboard. | attachment (for snapshots) |
| **Appeal** | A member's request to reconsider a case: **accept**, **reject**, **request more information**, **close**, **reopen**. | decline, deny |
| **Moderation log** | Quack's append-only record of who did what. | audit log (UI), audit mirror |
| **Log channel** | The Discord channel moderation log entries are posted to. | mirror |
| **Server activity log** | The optional module that posts Discord events (edits, deletes, joins). | general logging |
| **Member** | The person a case is about. "You" in member-facing text. | user, target, offender |
| **Moderator**, **rules manager**, **administrator** | The three staff roles. **Staff** only when all are meant. | mod, admin (alone) |
| **Server** | A Discord guild, in user-facing text. Code and API say `guild`. | |

Code identifiers: `Rule`, `RuleVersion`, `Level`, `Case`, `Action`,
`ActionKind::{Timeout, Kick, Ban, RemoveTimeout, Unban}`, `Outcome`,
`Appeal`, `Evidence`, `Dm`, `ModLog`. A Discord account that is not being
discussed as a member of a server is an **account** (`account_id`), for
example whoever is signed in to the dashboard.

### 1.4 Message style

Every string a human reads follows these rules. All of them live in
`quack-text` (see [§12](#12-message-catalog)).

- Plain sentences. Say what happened, then what the reader can do. No codes,
  enum names, or the words "request", "resource", "execution", "delivery
  updated".
- Name the actor honestly: "Quack couldn't …", "Discord didn't let Quack …",
  "You need the Moderate Members permission …".
- A failure never hides a success: "Case #12 is saved. Quack couldn't send
  @Sam a DM because their DMs are closed. They can still see the case by
  signing in to the dashboard."
- **couldn't** means Discord said no. **couldn't confirm** means the result
  is unknown.
- The same fact uses the same sentence on every surface: Discord reply, log
  channel, dashboard, API error `message`.
- Staff-only information is always ephemeral in Discord. Only the case
  receipt may be public, and only when the moderator did not ask for it to
  be private.

### 1.5 Surfaces

| Surface | Used by | What it does |
| --- | --- | --- |
| Discord commands and buttons | Moderators, administrators, members | Apply rules, view and void cases, review appeals, guided setup, appeal from a DM. |
| Dashboard (`/`, served by `quack`) | Staff and members | Everything in Discord plus rule editing, settings, statistics, the moderation log, module settings, and the member's own cases and appeals. |
| HTTP API (`/api`) | The dashboard only | Not a public automation API. |
| Log channel | Staff | Posts selected moderation log entries. |
| DMs | Members | One DM per case (when the level says so), one DM per appeal decision, one DM when a ticket closes. |

---

## 2. People and permissions

Quack re-reads Discord state for every staff operation. Cached data never
grants lasting access: losing a role takes effect on the next operation.
Earlier records stay attributed to whoever acted.

### 2.1 Capabilities

`quack-core::permissions` derives a `Capabilities` set from facts; nothing
else grants access. Inputs: the actor's Discord permission bits in the
server, whether they own the server, their role IDs, the server's
`moderator_role_ids` and `rules_manager_role_ids` (after dropping roles the
server no longer has), the server's MFA requirement, and the actor's
recorded 2FA confirmation.

| Capability | Who has it | Grants |
| --- | --- | --- |
| `moderate` | Owner; Administrator; holders of a configured moderator role; when no configured moderator role still exists in the server, holders of Moderate Members. | Apply rules, view cases and member history, void cases, edit case context, add evidence, retry/dismiss/undo actions, review appeals, read the moderation log and statistics, work tickets as staff, list rules. |
| `manage_rules` | Owner; Administrator; Manage Server; holders of a configured rules manager role (no fallback). | Create, edit, archive, restore, import, and export rules; list rules. |
| `manage_settings` | Owner; Administrator; Manage Server. | Read and change settings except moderator roles; guided setup; module settings and enable/disable; server health; view moderator roles. |
| `manage_staff_roles` | Owner; Administrator. | Change `moderator_role_ids`. Resending the same set is not a change. |
| (member self) | The member a case is about, signed in to the dashboard or clicking their own DM. | See their own cases (valid and voided) and evidence; submit and reply to their own appeal; open and close their own ticket. Works after they leave or are banned. |

Rules:

1. **Moderator fallback.** If `moderator_role_ids` contains at least one role
   that still exists in the server, only those roles grant `moderate`
   (besides owner and Administrator). Otherwise Moderate Members grants it.
   Deleting roles therefore never locks a server out.
2. **Capabilities combine.** A member with a moderator role and Manage Server
   has `moderate`, `manage_rules`, and `manage_settings`.
3. **Staff destination predicate.** Staff-only channels, ticket threads, and
   the honeypot exemption use a broader predicate, "is staff for Discord
   access": owner, Administrator, Moderate Members, or a configured moderator
   role. It has no 2FA gate and does not include Manage Server or rules
   managers alone.
4. **Every member sees staff commands.** Staff commands set no default member
   permission because staff may hold a role rather than a Discord
   permission. Quack checks capabilities when they run. `/setup` defaults to
   Manage Server.

### 2.2 The 2FA rule

When the server's Discord MFA level is elevated (`guilds.mfa_required`),
every staff capability (owner included) also requires that Quack has
recorded the actor's account as having 2FA enabled. Quack reads `mfa_enabled`
from Discord's `GET /users/@me` at each dashboard sign-in and stores it in
`discord_accounts` with `mfa_checked_at`. An account never seen counts as not
having 2FA. There is no freshness limit: the last sign-in's answer holds
until the next sign-in. Member self operations are not staff operations and
ignore the rule. A refused actor gets message `error.mfa_required`.

### 2.3 Member checks

`quack_core::permissions::check_member(facts) -> Result<(), MemberRefusal>`
is pure. The caller gathers `ApplyFacts` from the `Discord` port (gateway
cache first, REST fallback). Checks run in this order; the first failure
wins.

| # | Check | Refusal (`MemberRefusal`) | Caused by Quack's access? |
| --- | --- | --- | --- |
| 1 | Quack is in the server. | `QuackNotInServer` | yes |
| 2 | The actor is in the server (skipped for Quack-initiated work). | `ActorNotInServer` | no |
| 3 | The member is in the server (only where the operation requires it, see §2.4). | `MemberNotInServer` | no |
| 4 | The member is not the actor. | `MemberIsSelf` | no |
| 5 | The member is not a bot account (this includes Quack). | `MemberIsBot` | no |
| 6 | The member is not the server owner. | `MemberIsOwner` | no |
| 7 | The member's highest role position is strictly below the actor's highest role position, unless the actor is the owner. Equal positions refuse. | `MemberRoleTooHigh` | no |
| 8 | The member's highest role position is strictly below Quack's highest role position. | `QuackRoleTooLow` | yes |
| 9 | Quack holds the Discord permission for the action kind: timeout and remove timeout need Moderate Members; kick needs Kick Members; ban and unban need Ban Members. Administrator on Quack satisfies all. Checked after the level is chosen (§4.2). | `QuackMissingPermission(Permission)` | yes |
| 10 | For a timeout, the member does not have the Administrator permission (Discord never times out administrators). Checked after the level is chosen. | `MemberIsAdministrator` | no |

Checks 7 and 8 also apply to warnings (cases with no action), because a
case is a moderation decision about the member. When the member is not in the
server, checks 4 to 8 use the account facts that are still known (ID, bot
flag) and skip role positions.

Refusals caused by Quack's own access (`QuackNotInServer`,
`QuackRoleTooLow`, `QuackMissingPermission`) are recorded in the moderation
log as `case_create_refused` because an administrator must fix them. All
other refusals, missing capabilities, and the 2FA rule are answered but
never logged.

### 2.4 Operations and their checks

| Operation | Capability | Extra checks |
| --- | --- | --- |
| Apply a rule (`/case add`, "Add case", dashboard) | `moderate` + 2FA | Rule active. Member checks 1–10; check 3 required. |
| Apply the honeypot rule (Quack) | none (Quack acts) | Member checks 1, 3, 5, 6, 8, 9, 10 with Quack as the actor. |
| View a case, list cases, member history, evidence | `moderate` + 2FA | none |
| Edit case context, add evidence | `moderate` + 2FA | none (allowed on valid and voided cases) |
| Void a case | `moderate` + 2FA | Case is valid. |
| Retry a failed original action | `moderate` + 2FA | Action `failed`, not dismissed; case valid; failure is not `timeout_window_passed`; member checks 2, 4, 6, 7 with the retrying moderator as actor; check 3 for timeout. Quack's own checks run in the action job (§5.4). |
| Retry a failed undo action | `moderate` + 2FA | Action `failed`, not dismissed. |
| Dismiss a failed action | `moderate` + 2FA | Action `failed`, not dismissed. |
| Undo a done timeout or ban by hand | `moderate` + 2FA | Original action `done`, kind timeout or ban, no undo exists; for timeout, `timeout_ends_at` in the future and check 7 when the member is in the server. |
| Accept, reject, request information, close, reopen an appeal | `moderate` + 2FA | Appeal state allows the transition (§8.2); `expected_version` matches. |
| Submit an appeal, reply with information | member self | §8.1 eligibility. |
| Rule create, edit, archive, restore, import, export | `manage_rules` + 2FA | §3 validation. |
| Read settings | `manage_settings` + 2FA | none |
| Change settings | `manage_settings` + 2FA | `manage_staff_roles` when `moderator_role_ids` changes; channel checks (§13.1.3). |
| Guided `/setup`, module settings, enable/disable | `manage_settings` + 2FA | Module enable checks (§13). |
| Server health | `manage_settings` + 2FA | none |
| Read the moderation log, statistics | `moderate` + 2FA | none |
| Open own ticket | member of the server | Tickets enabled; no ticket of theirs is opening, open, or closing. |
| Close a ticket | ticket's member, or `moderate` + 2FA | Ticket `open`. |
| Fix ticket access | `moderate` + 2FA | Ticket `open`. |

---

## 3. Rules and escalation

### 3.1 Storage model

- `rules` holds a rule's identity: ID, server, `current_version`, the current
  name (denormalized for listing and uniqueness), `archived_at`, and who
  created and last changed it.
- `rule_versions` holds every version as one validated JSON document
  (`definition`), immutable once written (a trigger rejects UPDATE and
  DELETE).
- A case references `(rule_id, rule_version)` and stores only its decision
  (§4.4).

Editing a rule inserts version `current_version + 1` and updates
`rules.current_version` and `rules.name` in one transaction. The client
sends `expected_version`; a mismatch is `409 rule_changed`. Archive and
restore change `archived_at` only, never the version. There is no delete,
draft, or enable switch: a saved rule that is not archived is active
immediately.

### 3.2 The rule definition document

`quack_core::rule::RuleDefinition` serializes to and from this JSON (the
`definition` column, the API body, and the export file all use it):

```json
{
  "name": "Spam",
  "description": "Unsolicited ads, link spam, and repeated messages.",
  "reason": "Posting spam or unsolicited advertising.",
  "appealable": true,
  "counting_window_days": 30,
  "fields": [
    { "key": "what_happened", "label": "What happened?", "kind": "long_text", "required": true },
    { "key": "message", "label": "Message link", "kind": "message_link", "required": false }
  ],
  "levels": [
    { "from_case_count": 1, "action": null, "send_dm": true },
    { "from_case_count": 3, "action": { "kind": "timeout", "duration_seconds": 3600 }, "send_dm": true },
    { "from_case_count": 5, "action": { "kind": "ban", "delete_message_seconds": 86400 }, "send_dm": true }
  ]
}
```

| Field | Type | Meaning |
| --- | --- | --- |
| `name` | string | Shown to staff and members. Unique (case-insensitive, trimmed) among the server's non-archived rules. |
| `description` | string | Staff-facing explanation; may be empty. Shown in pickers (truncated to 100 characters there). |
| `reason` | string | The official reason shown to the member in the DM, receipt, and dashboard. No placeholders. |
| `appealable` | bool | Whether cases under this version can be appealed. |
| `counting_window_days` | integer or null | null counts all past cases; otherwise only cases created within the last N days count. |
| `fields` | array | Case context fields, in display order. |
| `fields[].key` | string | Stable machine key; unique within the rule. |
| `fields[].label` | string | The question shown to the moderator and the label shown to the member. |
| `fields[].kind` | enum | `short_text`, `long_text`, `boolean`, `number`, `message_link`. |
| `fields[].required` | bool | Must be answered when the rule is applied. |
| `levels` | array | Ascending by `from_case_count`. |
| `levels[].from_case_count` | integer | The case count at which this level starts. The first level must be 1. |
| `levels[].action` | object or null | null means the case is a warning. `{"kind":"timeout","duration_seconds":N}`, `{"kind":"kick"}`, or `{"kind":"ban","delete_message_seconds":N}`. |
| `levels[].send_dm` | bool | Whether the member gets a case DM. |

Rust shape (in `quack-core`):

```rust
pub struct RuleDefinition {
    pub name: String,
    pub description: String,
    pub reason: String,
    pub appealable: bool,
    pub counting_window_days: Option<u16>,
    pub fields: Vec<Field>,
    pub levels: Vec<Level>,
}
pub struct Field { pub key: FieldKey, pub label: String, pub kind: FieldKind, pub required: bool }
pub enum FieldKind { ShortText, LongText, Boolean, Number, MessageLink }
pub struct Level { pub from_case_count: u16, pub action: Option<LevelAction>, pub send_dm: bool }
pub enum LevelAction {
    Timeout { duration_seconds: u32 },
    Kick,
    Ban { delete_message_seconds: u32 },
}
```

### 3.3 Validation

`RuleDefinition::validate(&self, Strictness) -> Result<(), Vec<RuleProblem>>`
runs before every write and reports every problem at once. Text is trimmed
first; "characters" means Unicode scalar values.

| Field | Rule | Problem code |
| --- | --- | --- |
| `name` | 1–100 characters. | `name_length` |
| `name` | Not used by another non-archived rule in the server (case-insensitive). Checked by the database. | `rule_name_taken` (409) |
| `description` | 0–500 characters. | `description_length` |
| `reason` | 1–1,000 characters. | `reason_length` |
| `counting_window_days` | null or 1–36,500. | `counting_window` |
| `fields` | 0–10 entries. | `too_many_fields` |
| `fields[].key` | Matches `^[a-z0-9][a-z0-9_-]{1,63}$`; unique within the rule. | `field_key`, `field_key_duplicate` |
| `fields[].label` | 1–100 characters. | `field_label_length` |
| `levels` | 1–10 entries. | `level_count` |
| `levels[0].from_case_count` | Exactly 1 (`Strictness::New`). | `first_level_not_one` |
| `levels[i].from_case_count` | 2–1,000 for i > 0; strictly increasing. | `level_case_count`, `level_order` |
| timeout `duration_seconds` | 1–2,419,200 (28 days). | `timeout_duration` |
| ban `delete_message_seconds` | 0–604,800 (7 days). | `ban_delete_window` |
| honeypot rule | When the server's honeypot uses this rule, no field may be required. | `required_field_on_honeypot_rule` (409) |
| active rule count | At most 100 non-archived rules per server (create, import, restore). | `too_many_rules` (409) |

`Strictness::Historical` is used only for rule versions reconstructed by the
importer (§19.5): it allows a first level other than 1 and does not check
name uniqueness. Quack never applies a historical version to a new case,
because new cases always use `current_version`, which is always validated
with `Strictness::New`.

### 3.4 Choosing the level

`quack_core::escalation::select_level(def: &RuleDefinition, case_count: u32) -> LevelChoice`
is pure:

1. `case_count` is the number of **counted** cases plus one for the new case.
   A case is counted when it is in the same server, for the same member, for
   the same `rule_id` (any version), not voided, not `source = 'imported'`,
   and, when the current version has a counting window, created at or after
   `now - counting_window_days × 86,400 s`. The window always comes from the
   version being applied, so changing the window affects counting only.
2. The chosen level is the level with the highest `from_case_count` that is
   less than or equal to `case_count`. Because the first level starts at 1,
   there is always one.
3. `LevelChoice { index: u8 /* 1-based */, from_case_count, action, send_dm }`.

Escalation never uses other rules, other servers, weights, or a moderator's
choice. Proptest covers: monotonic level choice in `case_count`; the first
level for `case_count = 1`; the last level for very large counts.

### 3.5 Archive and restore

- Archive sets `archived_at = now()`; the rule disappears from pickers and
  autocomplete; its cases and versions stay visible. Archive is refused
  with `409 rule_used_by_honeypot` while the honeypot module's settings point
  at the rule.
- Restore clears `archived_at`. It is refused with `409 rule_name_taken` when
  another active rule now has the same name, and with `409 too_many_rules`
  at the limit.
- Both write a moderation log entry and notify the rules cache (§17.6).

### 3.6 Import and export

Export returns a portable file:

```json
{
  "format": "quack-rule",
  "format_version": 1,
  "exported_at": "2026-11-17T18:00:00Z",
  "rule": { "...": "a RuleDefinition document" }
}
```

It never contains server IDs, channel IDs, case data, member history, or
settings. Import accepts the same file with `"confirm": true` in the request
body, validates it as a new rule (`Strictness::New`), and creates a new rule
at version 1. A name clash is `409 rule_name_taken`; the dashboard asks for a
new name. Export is a read and is not logged. Import is logged as
`rule_imported`.

### 3.7 The starter rule

When Quack joins a server for the first time (no `guilds` row), it creates,
in the bootstrap transaction (§17.5), this rule and stores its ID in
`guild_settings.starter_rule_id`:

```json
{
  "name": "General rule violation",
  "description": "A starter rule. Review it and change it to fit your server.",
  "reason": "Breaking the server rules.",
  "appealable": true,
  "counting_window_days": null,
  "fields": [],
  "levels": [
    { "from_case_count": 1, "action": null, "send_dm": true },
    { "from_case_count": 3, "action": { "kind": "timeout", "duration_seconds": 86400 }, "send_dm": true },
    { "from_case_count": 5, "action": { "kind": "ban", "delete_message_seconds": 86400 }, "send_dm": true }
  ]
}
```

`created_by` is NULL (Quack). The dashboard shows a notice asking
administrators to review it until someone with `manage_settings`
acknowledges it (`starter_notice_acknowledged_at`). Rejoining a server never
creates a second starter rule.

### 3.8 The honeypot rule

The honeypot module (§13.4) applies one rule chosen in its settings. When an
administrator turns on the honeypot from `/setup` without choosing a rule,
Quack uses the active rule named "Honeypot" if one exists, otherwise creates
it:

```json
{
  "name": "Honeypot",
  "description": "Applied when a member posts in the honeypot channel.",
  "reason": "Posting in the honeypot channel.",
  "appealable": true,
  "counting_window_days": null,
  "fields": [],
  "levels": [
    { "from_case_count": 1, "action": { "kind": "ban", "delete_message_seconds": 0 }, "send_dm": true }
  ]
}
```

The rule the honeypot uses must have no required fields (validated at setup
and on every edit of that rule) and cannot be archived while in use.

### 3.9 Limits

All limits live in `quack_core::limits` as constants, are validated before
any write, and are mirrored by database CHECK constraints where a column
holds the value. "Characters" means Unicode scalar values after trimming.

| Limit | Value | Where enforced |
| --- | --- | --- |
| Rule name | 1–100 characters | §3.3, `rules.name`, `rule_versions.definition` |
| Rule description | 0–500 characters | §3.3 |
| Official reason | 1–1,000 characters | §3.3 |
| Context fields per rule | 0–10 | §3.3 |
| Field key | `^[a-z0-9][a-z0-9_-]{1,63}$` | §3.3 |
| Field label | 1–100 characters | §3.3 |
| Levels per rule | 1–10 | §3.3 |
| Level `from_case_count` | first = 1; others 2–1,000, strictly increasing | §3.3 |
| Timeout duration | 1–2,419,200 s | §3.3 |
| Ban message deletion window | 0–604,800 s | §3.3 |
| Counting window | null or 1–36,500 days | §3.3 |
| Active rules per server | 100 | §3.3 |
| Context `short_text` | 1–500 characters | §4.6 |
| Context `long_text` | 1–4,000 characters | §4.6 |
| Context `message_link` | a Discord message link, at most 200 characters | §4.6 |
| Context `number` | finite, absolute value at most 1,000,000,000,000,000, at most 6 decimal places | §4.6 |
| Void reason | 1–1,000 characters | §4.7, `cases.void_reason` |
| Appeal statement, information reply | 1–4,000 characters | §8.1, `appeal_events.text` |
| Appeal decision reason | 1–2,000 characters | §8.2 |
| Evidence items per case | 50 | §7.1 |
| Evidence files per case | 100 | §7.1 |
| Message links per add-evidence call | 10 | §7.1 |
| Saved message text per evidence item | 4,000 characters | §7.2 |
| Saved embeds per evidence item | 10 | §7.2 |
| Saved files per evidence item | 10 | §7.2 |
| Largest file Quack copies | 10,485,760 bytes (10 MiB) | §7.3 |
| Moderator roles, rules manager roles | 0–25 each | §13.1 |
| DM introduction | 0–1,000 characters | §13.1 |
| DM footer | 0–500 characters | §13.1 |
| Rejoin invite | `https://discord.gg/<code>`, code 2–32 of `[A-Za-z0-9-]` | §13.1 |
| Honeypot warning text | 0–1,500 characters | §13.4 |
| Ticket transcript retention | 1–365 days (default 90) | §13.2 |
| Ticket transcript file | 8,388,608 bytes (8 MiB) | §13.2 |
| `Idempotency-Key` header | 8–200 printable ASCII characters | §14.1 |
| API page size | default 50, maximum 100 | §14.1 |
| Discord list page size | 10 rows | §11 |
| Case draft lifetime | 15 minutes | §11.6 |
| HTTP request body | 262,144 bytes (256 KiB) | §14.8 |
| Statistics range | 1 second to 366 days | §14.3 |

---

## 4. Cases

A case records that one rule was applied to one member. It is itself the
moderation outcome (a warning when the level has no action). Its action,
DM, evidence, and appeal are separate records with their own outcomes; none
of them changes whether the case is valid.

### 4.1 Case sources

| `case_source` | Created by | Notes |
| --- | --- | --- |
| `discord` | `/case add`, the "Add case" message command, the "Add case" member command | Operation key: interaction ID, or the draft ID for multi-page forms (§11.6). |
| `dashboard` | `POST /api/guilds/{guild_id}/cases` | Operation key: `Idempotency-Key` header. |
| `honeypot` | The honeypot module | Operation key `honeypot:<guild_id>:<first message id>`. `moderator_id` is NULL. |
| `imported` | The MySQL importer, for cases the Go system imported from Quack v4 | Never counted for escalation; no rule, no action, no DM, no appeal. |

### 4.2 Creating a case, end to end

`quack_engine::cases::apply_rule(&self, req: ApplyRule) -> Result<Applied, ApplyError>`
is the only way to create a case. `ApplyRule` carries: guild, actor
(`Principal` or `Quack`), member ID, rule ID, context values, evidence
sources (message links and, for the message command, the source message),
`private` (receipt visibility), optional `replaces_case_id`, source, the
operation key and its scope, and an optional receipt destination
(interaction token and channel). The caller has already acknowledged the
interaction or accepted the HTTP request.

**Before the transaction** (no locks held; may call Discord):

1. **Replay check.** Look up `(scope, key)` in `operation_keys`. If found
   with the same request hash, return `Applied::Replayed(case_id)`; the
   caller renders the existing case. If found with a different request hash,
   return `ApplyError::OperationKeyReused`.
2. **Capability.** For staff sources, the `Principal` must hold `moderate`
   and pass the 2FA rule (§2.2). Otherwise `Forbidden`.
3. **Rule.** Load the rule and its current version from the rules cache
   (§17.6; Postgres on a miss). Missing → `RuleNotFound`; archived →
   `RuleArchived`.
4. **Context.** Validate the submitted values against the version's fields
   (§4.6). When the case starts from a message, every empty `message_link`
   field is filled with that message's link before validation.
5. **Evidence sources.** Collect, deduplicated by message ID: the source
   message (message command), every `message_link` context value, and any
   explicit links (dashboard). Each must be a link into this server
   (§7.1); more than 50 → `TooMuchEvidence`.
6. **Facts.** Ask the `Discord` port for `ApplyFacts`: server owner, MFA
   level, Quack's member and permissions, the actor's member (staff
   sources), and the member (or `None` when absent). Discord errors here →
   `DiscordUnavailable`; nothing is written.
7. **Member checks 1–8** (§2.3). A refusal caused by Quack's own access is
   written to the moderation log in its own short transaction
   (`case_create_refused`, result `refused`) and returned; for honeypot
   cases the module logs `honeypot_case_failed` instead (§13.4.2). Other
   refusals are returned without a log entry.

**Inside one transaction** (READ COMMITTED; all statements in order):

8. `UPDATE case_counters SET last_number = last_number + 1 WHERE guild_id = $1 RETURNING last_number`.
   This row lock serializes case creation per server until commit, so the
   count in step 10 sees every earlier committed case.
9. `INSERT INTO operation_keys (...) VALUES (...)` with the response that
   this request will produce (`{"case_id": "<new id>"}`; the case ID is a
   UUIDv7 generated before the transaction). A unique violation means a
   concurrent request with the same key committed first: roll back and go
   to step 1.
10. Count counted cases (§3.4) with the `cases_escalation` index; add one.
11. `select_level` (§3.4).
12. If the level has an action, run member checks 9 and 10 against the
    facts from step 6. A refusal rolls back and is returned; a check 9
    refusal (Quack's access) is also logged as in step 7.
13. If `replaces_case_id` is set: the referenced case must be in the same
    server, voided, and not already replaced (unique index). Otherwise
    `ReplacementInvalid`.
14. Insert the `cases` row with the decision (§4.4), `number` from step 8,
    `created_at = now()` captured once in Rust and reused below.
15. If the level has an action, insert an `actions` row (`purpose`
    original, state `pending`). For a timeout,
    `timeout_ends_at = created_at + duration_seconds`.
16. If the level sends a DM, insert a `dms` row (kind `case`, state
    `pending`, fresh nonce).
17. Insert one `evidence` row per evidence source (state `saving`).
18. If a receipt destination is given, insert a `messages` row (kind
    `case_receipt`, `message_id` NULL, `wanted_revision` 1,
    `posted_revision` 0, the interaction token and its expiry).
19. Insert the moderation log entry `case_created` (§9) and, when the log
    channel is set, its `post_mod_log` job.
20. Enqueue jobs (§16): `send_dm` for the DM; `run_action` for the action,
    except a kick or ban with a DM, whose `run_action` job is enqueued by
    the DM job when the DM settles (§6.3); one `capture_evidence` per
    evidence row; `sync_message` for the receipt.
21. `NOTIFY quack_jobs` (sent on commit).
22. Commit. The transaction must finish within 2 seconds
    (`SET LOCAL statement_timeout = '2s'`).

**After the transaction:**

23. Return `Applied::Created(CaseCreated { case, action, dm })`. The bot
    calls `sync_message` inline for the receipt (§10.3) so the moderator
    sees it without waiting for a worker; the API returns the case.
24. Metrics: `quack.case.created`, `quack.case.create.duration`,
    `quack.case.level`. Span event `case.created`.

A failure in steps 8–22 (other than the handled ones) rolls back everything:
no case number is consumed, no job exists, nothing reaches Discord.

### 4.3 Operation keys

Every state-changing command carries an operation key, stored in
`operation_keys` in the same transaction as the write it protects.

| Origin | Scope | Key |
| --- | --- | --- |
| Slash command, button, select, modal | `discord:<guild_id or dm>:<account_id>` | The interaction ID (decimal). |
| Multi-page `/case add` | `discord:<guild_id>:<account_id>` | `draft:<draft_id>` (stable across pages and double clicks). |
| Dashboard write | `http:<account_id>:<METHOD> <route pattern>:<guild_id or ->` | The `Idempotency-Key` header. |
| Honeypot | `quack:<guild_id>` | `honeypot:<guild_id>:<message_id>`. |

- `request_hash` is SHA-256 of a canonical encoding of the request: for HTTP
  the raw query string, media type, and body; for Discord the command name
  and decoded options.
- A replay with the same hash returns the stored `response_status` and
  `response` (HTTP replays add `Idempotency-Replayed: true`). A different
  hash returns `409 idempotency_key_reused`.
- Two concurrent requests with one key: the second blocks on the primary
  key until the first commits, then replays its result.
- Rows expire after 24 hours (housekeeping deletes them). Case creation is
  also protected forever by `cases.operation_key` (unique per server), so a
  replay after expiry still finds the case.
- Reads never use operation keys.

### 4.4 Case numbers and the decision

- `case_counters(guild_id, last_number)` is created with the server and
  only ever increments. Numbers start at 1, never repeat, and are not
  reused after voids. The importer sets `last_number` to the highest
  imported number.
- The decision stored on the case (its "snapshot") is exactly:

| Column | Meaning |
| --- | --- |
| `rule_id`, `rule_version` | The immutable version applied. Name, reason, appealability, and field labels are read from it. |
| `case_count` | The case count including this case. |
| `level_index` | 1-based position of the chosen level in that version. |
| `level_from_case_count` | That level's `from_case_count`. |
| `action_kind` | `timeout`, `kick`, `ban`, or NULL (warning). |
| `timeout_seconds` | For timeouts. |
| `ban_delete_message_seconds` | For bans. |
| `sends_dm` | Whether the level sends a DM. |

The decision never changes. Corrections are a void plus a replacement case.

### 4.5 Case states

A case is **valid** or **voided** (`voided_at IS NULL` means valid).

| From | Event | To | Effects |
| --- | --- | --- | --- |
| (none) | Commit of §4.2 | valid | §4.2 steps 14–20. |
| valid | Staff void (§4.7) | voided | §4.7. |
| valid | Appeal accepted (§8.2) | voided | §4.7 with cause `appeal_accepted`. |
| voided | Any void | voided | Refused: `409 case_already_voided`. A replay of the same operation key returns the original result instead. |

There is no un-void.

### 4.6 Case context validation

`quack_core::case::validate_context(fields, submitted) -> Result<ContextValues, Vec<ContextProblem>>`.
Submitted values arrive as `key → JSON value` (API) or `key → string` (Discord
modal). Unknown keys → `unknown_field`. Duplicate keys → `duplicate_field`.
An omitted, empty, or whitespace-only optional value is stored as absent.

| Kind | Accepted input | Stored as | Problems |
| --- | --- | --- | --- |
| `short_text` | String, 1–500 characters after trimming. | JSON string | `required`, `too_long` |
| `long_text` | String, 1–4,000 characters after trimming. | JSON string | `required`, `too_long` |
| `boolean` | API: JSON `true`/`false`. Discord: the select values `yes`/`no`. | JSON bool | `required`, `not_yes_no` |
| `number` | API: JSON number. Discord: text parsed as a decimal (`-` allowed, `.` decimal point, no exponent). Finite, absolute value ≤ 10^15, ≤ 6 decimal places. | JSON number | `required`, `not_a_number`, `number_out_of_range` |
| `message_link` | `https://discord.com/channels/<guild>/<channel>/<message>` (also `ptb.`, `canary.`, `www.` hosts and `discordapp.com`), IDs of 17–20 digits, `guild` equal to this server, at most 200 characters. Query and fragment are dropped. | Canonical `https://discord.com/channels/…` string | `required`, `not_a_message_link`, `other_server` |

`false` and `0` are answers, not absence. Validation reports every problem
at once; the message for each is in §12.3.

### 4.7 Voiding

`quack_engine::cases::void_case(principal, case_ref, reason, op_key)`:

1. Capability `moderate` + 2FA. Reason 1–1,000 characters.
2. One transaction:
   1. Insert the operation key.
   2. `SELECT … FROM cases WHERE guild_id = $1 AND id = $2 FOR UPDATE`.
      Voided already → `409 case_already_voided`.
   3. Set `voided_at = now()`, `voided_by`, `void_reason`, `void_cause`
      (`staff` or `appeal_accepted`).
   4. Original actions in state `pending` → `cancelled` (their jobs find the
      state terminal and finish). Actions in `running` are left alone: the
      job checks validity when it settles (§5.4).
   5. For each original timeout or ban in state `done` with no undo yet:
      insert an undo action (`remove_timeout` or `unban`, state `pending`,
      `requested_by` = the voider, `undo_of_action_id`) and its `run_action`
      job. A timeout whose `timeout_ends_at` has passed gets no undo.
   6. The case DM in state `pending` → `not_needed`. A DM in `sending` or a
      terminal state is left alone.
   7. An appeal in `pending` or `needs_information` → `closed` with
      appeal event `closed` (actor NULL, text "Closed because the case was
      voided.") and an appeal DM (§6) — unless the void is the appeal's own
      acceptance.
   8. Bump the case receipt and the appeal post (§10).
   9. Moderation log `case_voided` (detail: cause, cancelled action IDs,
      undo action IDs).
3. Metric `quack.case.voided{cause}`.

Undo actions are never cancelled by a later void: once queued, they run.

### 4.8 Replacement links

A wrong case is fixed by voiding it and creating a new case with
`replaces_case_id` pointing at it (§4.2 step 13). One voided case has at most
one replacement (`UNIQUE (replaces_case_id)`). The replacement may use a
different rule or member. Both views show the link in both directions
("Replaced by case #13", "Replaces case #12").

### 4.9 Edits

The decision never changes. Two edits exist, both on valid or voided cases:

- **Edit context.** Replaces the case context with new values validated
  against the case's own rule version (not the current one). New
  `message_link` values that are not yet evidence get evidence rows and
  `capture_evidence` jobs (subject to the per-case cap). Existing evidence
  is never removed. Moderation log `case_context_edited` (detail: changed
  field keys). The receipt is bumped.
- **Add evidence.** Adds 1–10 message links (§7.1). Moderation log
  `evidence_added` (detail: count). The receipt is bumped.

### 4.10 Reading cases

Staff read by number (`/case view`, `GET …/cases/{case_ref}` where
`case_ref` is a number, a UUID, or a legacy ULID mapped as in §19.3).
Members read their own case by UUID or legacy ULID.

| Field | Staff view | Member view |
| --- | --- | --- |
| Server name and icon | yes | yes |
| Case number, created time | yes | yes |
| Valid/voided, void reason, voided time | yes | yes |
| Who voided it | yes | no |
| Rule name, version, official reason | yes | yes (name and reason) |
| Case count, level index, level start | yes | no |
| Decided action and its outcome sentence | yes | yes |
| Timeout end time | yes | yes |
| Action attempts, failure reasons, HTTP and Discord codes | yes | no (outcome sentence only) |
| Undo actions and their outcomes | yes | yes (outcome sentence only) |
| DM outcome | yes | yes ("Quack sent you a DM" / "couldn't" / "couldn't confirm") |
| Case context (all fields, including message links) | yes | yes |
| Evidence text, embeds summary, files | yes | yes, through the member file route |
| Evidence author, original message link, channel | yes | no |
| Moderator | yes | no |
| Source (`discord`, `dashboard`, `honeypot`, `imported`) | yes | no |
| Replacement links | yes | yes (case numbers only) |
| Appeal (state, statement, member replies, decision reasons) | yes | yes |
| Who decided the appeal | yes | no |
| Staff timeline (moderation log entries linked to the case) | yes | no |
| Member timeline (§4.11) | yes | yes |

Member reads never reveal whether a case exists in another member's name:
a wrong owner and a missing case are both `404 not_found`.

### 4.11 Member timeline

Built from rows, never from the moderation log, so it carries no staff
identity. Items, oldest first: case added (`created_at`); each action
reaching a terminal state (its outcome sentence, `done_at` or `updated_at`);
the case DM settling; each appeal event (kind and text, actor removed);
case voided (`voided_at`, reason).

---

## 5. Actions

An action is one Discord change Quack makes for a case. A case has at most
one **original** action (timeout, kick, or ban) and at most one **undo**
action per original (remove timeout for a timeout, unban for a ban). A kick
has no undo.

### 5.1 Kinds

| `action_kind` | Purpose | Discord call | Permission | Done when |
| --- | --- | --- | --- | --- |
| `timeout` | original | `PATCH /guilds/{g}/members/{m}` `communication_disabled_until = timeout_ends_at` | Moderate Members | 2xx. |
| `kick` | original | `DELETE /guilds/{g}/members/{m}` | Kick Members | 2xx, or 404 Unknown Member (10007): the member already left. |
| `ban` | original | `PUT /guilds/{g}/bans/{m}` `delete_message_seconds` | Ban Members | 2xx (also when already banned). |
| `remove_timeout` | undo of `timeout` | `PATCH /guilds/{g}/members/{m}` `communication_disabled_until = null` | Moderate Members | 2xx, or 404 Unknown Member: nothing to remove. |
| `unban` | undo of `ban` | `DELETE /guilds/{g}/bans/{m}` | Ban Members | 2xx, or 404 Unknown Ban (10026): nothing to undo. |

Every call sends `X-Audit-Log-Reason: Quack case #<n>: <rule name>` truncated
to 512 characters (undo: `Quack case #<n> voided`).

### 5.2 Fixed timeout end and idempotency

- `timeout_ends_at = cases.created_at + timeout_seconds`, fixed at creation
  and sent unchanged on every attempt. Repeating the call converges on the
  same state.
- A ban repeated is still one ban. An unban of a member who is not banned is
  done. A kick of a member who already left is done. A timeout removal for a
  member who is not timed out is done.
- Therefore an attempt whose result is unknown (crash, timeout, 5xx,
  connection reset) is retried. No action is ever parked for review because
  its outcome is uncertain.

### 5.3 States

| `action_state` | Outcome shown | Meaning |
| --- | --- | --- |
| `pending` | in progress | Waiting for its first attempt or for the next retry (`next_attempt_at`). |
| `running` | in progress | An attempt has started and has not settled. |
| `done` | done | Discord confirmed the change, or the desired state already held. |
| `failed` | failed | A definite refusal, or the retry budget ran out. `failure_reason` says which. |
| `cancelled` | cancelled | An original action whose case was voided before it started. |
| `skipped` | left in place | An undo that did not run because another valid case still applies (`skipped_because_case_id`). |

Transition table (`quack_core::action::transition(state, event)`; any pair
not listed is rejected as `InvalidTransition`):

| From | Event | To | Side effects (same transaction) |
| --- | --- | --- | --- |
| (none) | Case created with an action / undo requested | `pending` | Insert `run_action` job (except kick/ban waiting for the DM, §6.3). |
| `pending` | Claimed; checks pass; attempt recorded | `running` | Insert `action_attempts` row (`finished_at` NULL). |
| `pending` | Claimed; original action and case voided | `cancelled` | Bump receipt. |
| `pending` | Claimed; undo guard finds another valid case (§5.6) | `skipped` | `skipped_because_case_id`; log `action_undo_skipped`; bump receipt. |
| `pending` | Claimed; Quack-side pre-check refuses (§5.4 step 6) | `failed` | `failure_reason`; log `action_failed`; bump receipt. |
| `pending` | Claimed; timeout window passed | `failed` | `failure_reason = timeout_window_passed`; log; bump. |
| `pending` | Case voided (§4.7) | `cancelled` | Original actions only. |
| `running` | Attempt `done` or `no_change` | `done` | `done_at`; log `action_done`; bump; if original and case now voided, queue the undo (§4.7 step 5). |
| `running` | Attempt `refused` | `failed` | `failure_reason` from §5.5; log `action_failed`; bump. |
| `running` | Attempt `unknown`, `rate_limited`, or `interrupted`, attempts left | `pending` | `next_attempt_at`; job rescheduled (§5.7). |
| `running` | Same, no attempts left | `failed` | `failure_reason = retries_exhausted`; log; bump. |
| `running` | Attempt not `done`, original action, case now voided | `cancelled` | Bump. |
| `failed` | Staff retry (§5.8) | `pending` | Clear `failure_reason`; new job with a fresh budget; log `action_retry_requested`; bump. |
| `failed` | Staff dismiss | `failed` | `dismissed_at`, `dismissed_by`; log `action_dismissed`; bump. |

`done`, `cancelled`, and `skipped` are terminal. `failed` is terminal unless
staff retry it.

### 5.4 Running an attempt

The `run_action` job handler (`quack_engine::actions::run`):

1. **Serialize per member.** Take a session advisory lock on a dedicated
   pooled connection: `SELECT pg_try_advisory_lock(hashtextextended('member:' || $guild || ':' || $member, 0))`.
   Not acquired → return `Defer(1 s)` (no attempt consumed). All actions for
   one (server, member) therefore run one at a time across workers. The
   lock is released after step 10 or when the connection drops.
2. Load the action and its case. A terminal state → `Done` (idempotent
   re-delivery).
3. An attempt row with `finished_at IS NULL` belongs to an attempt that
   crashed: set its outcome to `interrupted` and `finished_at = now()`.
4. Original action on a voided case → `cancelled`.
5. Undo action → run the guard (§5.6); another case applies → `skipped`.
6. **Quack-side pre-check** from `ApplyFacts` (cache, REST fallback):
   Quack in the server (`quack_not_in_server`), Quack holds the permission
   (`quack_missing_permission`), the member is not the owner
   (`member_is_owner`), for timeout the member is in the server
   (`member_not_in_server`) and does not have Administrator
   (`member_is_administrator`), and when the member is present, the member's
   highest role is below Quack's (`quack_role_too_low`; not checked for
   `unban`). A refusal fails the action without calling Discord.
7. Timeout: `now() >= timeout_ends_at - 5 s` → `failed`
   (`timeout_window_passed`).
8. Record the attempt (short transaction): state `running`,
   `attempts = attempts + 1`, `last_attempt_at`, insert `action_attempts`
   with `started_at`.
9. Call Discord through the `Discord` port with a 10-second deadline
   (§17.3). The port returns `Outcome<()>`.
10. Settle (one transaction, guarded by `state = 'running'`): finish the
    attempt row, apply the transition from §5.3, write the log entry, bump
    the receipt, and return the job result.

Metrics: `quack.action.attempts{kind,outcome,reason}`; on `done`,
`quack.action.time_to_done{kind}` (from case creation); on retry,
`quack.action.retries{kind}`; on `skipped`, `quack.action.undo.skipped{kind}`.

### 5.5 Attempt outcomes and failure reasons

| Port outcome | `attempt_outcome` | Next |
| --- | --- | --- |
| `Done` | `done` | `done` |
| `Refused` with Unknown Member (kick, remove timeout) or Unknown Ban (unban): the desired state already holds (§5.1) | `no_change` | `done` |
| `Refused(r)` | `refused` | `failed` with the reason below |
| `RetryAfter(d)` | `rate_limited` | retry at `max(d, backoff)` |
| `Unknown(c)` | `unknown` | retry with backoff |
| crash detected in step 3 | `interrupted` | retry with backoff |

`action_failure` (typed enum `ActionFailure`; message ids in §12.6):

| Value | When |
| --- | --- |
| `quack_not_in_server` | Pre-check, or Discord 404 Unknown Guild (10004). |
| `quack_missing_permission` | Pre-check. Carries the permission name. |
| `quack_role_too_low` | Pre-check. |
| `member_is_owner` | Pre-check. |
| `member_not_in_server` | Timeout pre-check, or 404 Unknown Member on a timeout. |
| `member_is_administrator` | Timeout pre-check: Discord never times out a member with Administrator. |
| `timeout_window_passed` | §5.4 step 7. |
| `discord_refused` | Any other 4xx except 429, including 403 Missing Permissions (50013) after the pre-check passed (a stale cache). Carries HTTP status and Discord error code. |
| `retries_exhausted` | The budget ran out on `unknown`, `rate_limited`, or `interrupted` attempts. |

### 5.6 The undo guard

Inside the member-serialized section, before an undo action calls Discord:

```sql
SELECT c.id, c.number
FROM actions a JOIN cases c ON c.guild_id = a.guild_id AND c.id = a.case_id
WHERE a.guild_id = $1 AND a.member_id = $2
  AND a.kind = $3                        -- 'timeout' for remove_timeout, 'ban' for unban
  AND a.case_id <> $4                    -- the undo's own case
  AND c.voided_at IS NULL
  AND a.state IN ('pending', 'running', 'done')
  AND (a.kind <> 'timeout' OR a.timeout_ends_at > now())
  AND NOT EXISTS (SELECT 1 FROM actions u WHERE u.undo_of_action_id = a.id AND u.state = 'done')
ORDER BY c.number DESC LIMIT 1;
```

A row means another valid case still applies: the undo becomes `skipped`
with `skipped_because_case_id`, logged as `action_undo_skipped`, shown as
"left in place because case #N still applies". Uses index `actions_member`.

### 5.7 Retry policy

- Budget: 8 attempts per job (one first try plus seven retries).
- Backoff after attempt n (n = 1…7): 5 s, 15 s, 1 min, 5 min, 15 min,
  1 h, 3 h. Total ≈ 4 h 36 min.
- `RetryAfter(d)`: the next attempt waits `max(d, backoff)`.
- A `Defer` (lock busy) does not consume an attempt.
- A staff retry creates a new job with a fresh budget; `actions.attempts`
  keeps counting across budgets.

### 5.8 Staff controls

| Control | Allowed when | Effect |
| --- | --- | --- |
| Retry | `failed`, not dismissed. Original: case valid, reason not `timeout_window_passed`, retrying moderator passes member checks 2, 4, 6, 7 (and 3 for timeout). | `pending` + fresh job. |
| Dismiss | `failed`, not dismissed. | Sets `dismissed_at`; the action leaves the "Actions that need review" list; history kept. |
| Undo (remove timeout / unban) | Original `done`, kind timeout or ban, no undo yet. Timeout: end time in the future. | New undo action `pending` + job, `requested_by` = staff. The guard still applies. |

Refusals: `409 action_not_retryable` (wrong state or dismissed), `409
case_voided` (retry of an original on a voided case),
`422 timeout_window_passed`, `409 action_not_undoable`.

### 5.9 What a void does at each action state

| Original action state | On void |
| --- | --- |
| `pending` | `cancelled`. Never starts. |
| `running` | Left to finish. If it ends `done`, the undo is queued in the settle transaction; otherwise it ends `cancelled`. |
| `done` (timeout, end in the future; or ban) | Undo action queued. |
| `done` (timeout already ended; or kick) | Nothing. |
| `failed` | Stays `failed`; retry is refused because the case is voided. |
| `cancelled`, `skipped` | Nothing. |

| Undo action state | On void of its own case | On void of another case |
| --- | --- | --- |
| any | Unchanged: undo actions are never cancelled. | Unchanged; the guard re-evaluates when it runs. |

---

## 6. DMs

A DM is one direct message from Quack to a member. Each DM is a `dms` row
with its own outcome and is sent **at most once**.

### 6.1 Kinds

| `dm_kind` | Created when | At most | Content |
| --- | --- | --- | --- |
| `case` | A case is created at a level with `send_dm` | one per case | `dm.case` (§12.8) |
| `appeal_update` | A staff decision on an appeal (request information, accept, reject, close, reopen), or Quack closing the appeal because its case was voided | one per appeal event | `dm.appeal.*` (§12.8) |
| `ticket_closed` | A ticket finishes closing | one per ticket | `dm.ticket_closed` (§12.13) |

A case whose level has no DM has no `dms` row; every view reports its DM as
"not needed" (`dm.state.not_needed.level`).

### 6.2 States

| `dm_state` | Outcome shown | Meaning |
| --- | --- | --- |
| `pending` | in progress | Not yet sent. |
| `sending` | in progress | Recorded immediately before the create-message call. |
| `sent` | done | Discord returned the message. `message_id` set. |
| `failed` | failed | Discord definitely refused. `failure` set. |
| `unknown` | couldn't confirm | The send started but Quack never learned the result (crash, deadline, repeated network error). Terminal: never sent again. |
| `not_needed` | not needed | The case was voided before the DM was sent. |

| From | Event | To | Side effects (same transaction) |
| --- | --- | --- | --- |
| (none) | Created (§6.1) | `pending` | `send_dm` job. |
| `pending` | Case voided (case DMs only) | `not_needed` | Bump receipt. If a kick/ban waits on this DM, its action is already `cancelled` by the void. |
| `pending` | Job: DM channel opened, about to send | `sending` | `attempted_at`, `channel_id`. |
| `pending` | Job: opening the DM channel refused, or not opened after 3 attempts | `failed` | `failure`; log `dm_failed`; bump; release a waiting kick/ban. |
| `sending` | Create-message `Done` | `sent` | `message_id`, `settled_at`; log `dm_sent`; bump; release a waiting kick/ban. |
| `sending` | Create-message `Refused` | `failed` | `failure`; log `dm_failed`; bump; release. |
| `sending` | Create-message `RetryAfter` longer than 10 s (Discord did nothing) | `pending` | Job retried after the delay. |
| `sending` | Still `Unknown` after the in-job retry, or the job finds the row already `sending` (a crash) | `unknown` | log `dm_failed` (detail `unknown`); bump; release. |

"Release a waiting kick/ban" means: if this is a case DM and the case's
original action is a kick or ban in state `pending` with no job, insert its
`run_action` job in the same transaction.

### 6.3 Ordering with kick and ban

For kick and ban the DM goes **before** the action so the member can still
receive it (after a kick or ban they often share no server with Quack). The
case transaction enqueues only `send_dm`; the DM job enqueues `run_action`
when the DM reaches any terminal state. A DM failure never blocks the
action. For timeouts and warnings both jobs are enqueued at once and run
independently. The DM job makes at most three Discord calls with a
10-second deadline each, so a kick or ban normally waits a second or two and
at most about 35 seconds plus queue delay.

The DM describes the case's decision ("What happens: you're banned from the
server"), not the action's later outcome, so it is correct whichever order
the two finish in.

### 6.4 Sending

The `send_dm` handler (`quack_engine::dms::send`):

1. Load the row. Terminal state → `Done`.
2. Row is `sending` → a previous attempt crashed after it started: set
   `unknown` (§6.2) and finish. Quack never resends.
3. Case DMs: case voided → `not_needed`.
4. `open_dm(member_id)` (`POST /users/@me/channels`, idempotent). `Refused` →
   `failed` with the reason from §6.5. `Unknown` or `RetryAfter` → retry the
   job (budget 3 attempts, backoff 5 s, 30 s). Nothing was sent yet, so on
   exhaustion the DM becomes `failed` with `dm_channel_unavailable`.
5. Mark `sending` (short transaction).
6. Render the DM from current data (§12.8) and call `send(channel, msg,
   files, nonce)` (§17.3) with `enforce_nonce = true`. The nonce is the DM ID as 22
   characters of base64url, so a repeat within Discord's nonce window returns
   the first message instead of creating a second one.
7. `Unknown` → wait 2 s and repeat step 6 once with the same nonce.
   `RetryAfter(d)` with `d ≤ 10 s` → wait `d` and repeat step 6 (at most
   twice); with `d > 10 s` → back to `pending` and retry the job after `d`
   (nothing was sent). Anything else settles.
8. Settle per §6.2.

Metrics: `quack.dm.outcomes{kind,state,reason}`,
`quack.dm.send.duration{kind}`. Span event `dm.outcome`.

### 6.5 Failure reasons

`dm_failure` (`DmFailure`):

| Value | Discord answer | Sentence fragment (§12.7) |
| --- | --- | --- |
| `dms_closed` | 403 Cannot send messages to this user (50007) while the member is in the server | "because their DMs are closed or they blocked Quack" |
| `not_in_shared_server` | 50007 while the member is not in the server, or 50278 (no mutual servers) | "because they no longer share a server with Quack" |
| `dm_channel_unavailable` | Opening the DM channel failed three times without a definite answer | "because Discord didn't open a DM channel" |
| `discord_refused` | Any other 4xx except 429 | "because Discord refused the message (error {code})" |

### 6.6 What a case DM contains

In order (exact text in §12.8): the server name; the rule name; the official
reason as a quote; the server's DM introduction (if set); every case context
field except `message_link` fields (booleans as Yes/No); "What happens" with
the decided action (timeout end time as a Discord timestamp); the case
number and time; appeal instructions with an **Appeal** button and an
**Open in dashboard** link when the case is appealable, or "This case can't
be appealed."; the server's DM footer (if set). It never contains evidence,
the moderator's identity, or internal details.

---

## 7. Evidence

Evidence is saved message snapshots plus their files, attached to a case.
Capture happens after the case is committed, as jobs, so creating a case
never waits on Discord fetches or file copies.

### 7.1 Sources and limits

| Source | Added by |
| --- | --- |
| The message an "Add case" message command was used on | The case transaction |
| `message_link` context values | The case transaction, and context edits |
| Links given when creating a case on the dashboard | The case transaction |
| "Add evidence" (Discord button, dashboard) | Its own transaction, 1–10 links per call |

- A link must point into this server (§4.6 `message_link` rules); otherwise
  `other_server`.
- One evidence row per `(case_id, message_id)`; adding the same message
  again is a no-op that reports "already saved".
- At most 50 evidence rows and 100 files per case. Links beyond the cap are
  refused before any write (`422 evidence_limit_reached`).
- Each row starts `saving` with a `capture_evidence` job keyed
  `evidence:<evidence_id>`. Views show "Saving evidence…" until it settles.

### 7.2 The capture job

`quack_engine::evidence::capture`:

1. Load the row; terminal state → `Done`.
2. If `added_by` is a staff member (not Quack), check from the gateway cache
   that they can read the channel (View Channel and Read Message History,
   computed with overwrites; threads use the parent channel and private
   threads also need thread membership or Manage Threads). No access →
   `failed` (`moderator_no_access`).
3. `fetch_message(channel_id, message_id)`. `Refused` 404 Unknown Message or
   Unknown Channel → `failed` (`message_deleted`); 403 → `failed`
   (`quack_no_access`); the message's server differs → `failed` (`other_server`);
   `Unknown`/`RetryAfter` → retry (budget 4, backoff 10 s, 1 min, 5 min).
   Budget exhausted → `failed` (`discord_error`).
4. Save the snapshot: author ID, content (first 4,000 characters), created
   and edited times, up to 10 embeds as `{type, title, description, url}`
   summaries (title ≤ 256, description ≤ 1,000 characters), and up to 10
   files as `evidence_files` rows (filename, content type, size, original
   URL). More than 10 files → the rest are recorded with copy state
   `limit_reached`. The per-case file cap (100) also yields `limit_reached`.
5. Copy files (§7.3).
6. Set `saved` and `saved_at`; bump the receipt. Metric
   `quack.evidence.captures{outcome}`.

`evidence_state`: `saving` → `saved` | `failed`. A `saved` row may still have
individual files that could not be copied; each file has its own state.

`evidence_failure`: `message_deleted`, `moderator_no_access`,
`quack_no_access`, `other_server`, `discord_error`.

### 7.3 Copying files

When `guild_settings.evidence_channel_id` is set, the job copies each file:

- Eligible: size ≤ 10,485,760 bytes and content type (lowercased, without
  parameters) starting with `image/`, `video/`, `audio/`, or equal to
  `text/plain` or `application/pdf`.
- Before uploading, re-check the evidence channel is staff-only (§13.1.3).
  Not staff-only → copy state `copy_failed` for every file of this item and
  `copy_skipped_not_staff_only = true` on the item (shown as
  `evidence.channel_not_staff_only`).
- Download only from `cdn.discordapp.com` or `media.discordapp.net` over
  HTTPS, at most 5 redirects within those hosts, 30-second deadline, stop at
  10,485,760 bytes.
- Upload as `SPOILER_<filename>` with no message text, nonce = file ID
  (22 characters). Store `copy_message_id` and `copy_attachment_id`.
- Metrics: `quack.evidence.files_copied{outcome}`,
  `quack.evidence.bytes_copied`.

`file_copy_state`:

| Value | Meaning |
| --- | --- |
| `pending` | Not yet processed. |
| `copied` | A copy exists in the evidence channel. |
| `link_only` | No evidence channel is set; only the original link is kept. |
| `too_large` | Larger than 10 MiB. |
| `unsupported_type` | Not an eligible content type. |
| `copy_failed` | Download or upload failed, or the channel is not staff-only. |
| `limit_reached` | Over 10 files on the message or 100 on the case. |

### 7.4 Who sees evidence

- Staff see everything, including authors and original message links.
- The member sees their own case's evidence on the dashboard: message text,
  embed summaries, file names, and files through
  `GET /api/me/cases/{case_id}/evidence/{file_id}`. They never see authors,
  original message links, or channel names.
- File routes redirect (302) to a freshly signed Discord URL: a `copied`
  file uses its copy (Quack fetches the copy message for a fresh attachment
  URL); any other file uses its original URL refreshed through `POST
  /attachments/refresh-urls`. A file whose copy and original are both gone is
  `404 file_unavailable`.
- Evidence never appears in DMs, receipts, or the log channel.

---

## 8. Appeals

### 8.1 Eligibility

A member may appeal a case when all hold:

- The signed-in account (dashboard) or the clicking account (DM button) is
  the case's member. Server membership is not required.
- The case is valid.
- The case's rule version has `appealable = true` (the version the case
  used, not the current one).
- The case has no appeal (`UNIQUE (case_id)`).
- The case is not `imported`.

The statement answers one fixed question (§12.9 `appeal.form.*`) and is
1–4,000 characters. Ineligible → `409 appeal_exists`, `409 case_voided`, or
`422 appeal_not_allowed`.

### 8.2 States

| `appeal_state` | Meaning |
| --- | --- |
| `pending` | Waiting for staff. |
| `needs_information` | Staff asked the member for more information. |
| `accepted` | Staff accepted; the case is voided. Terminal. |
| `rejected` | Staff rejected; the case stands. |
| `closed` | Staff closed it, or Quack closed it because the case was voided. |

| From | Event (`appeal_event_kind`) | Actor | To | Effects (same transaction) |
| --- | --- | --- | --- | --- |
| (none) | `submitted` | member | `pending` | Appeal row (version 1), event; appeal post created (when the appeal channel is set); log `appeal_submitted`; bump receipt. |
| `pending` | `information_requested` | staff, reason | `needs_information` | Event; `appeal_update` DM; bump post; log. |
| `needs_information` | `information_provided` | member, text 1–4,000 | `pending` | Event; bump post; log. |
| `pending` | `accepted` | staff, reason | `accepted` | Event; void the case (§4.7, cause `appeal_accepted`); DM; bump post and receipt; log `appeal_accepted` and `case_voided`. |
| `pending` | `rejected` | staff, reason | `rejected` | Event; DM; bump post; log. |
| `pending`, `needs_information` | `closed` | staff, reason | `closed` | Event; DM; bump post; log. |
| `pending`, `needs_information` | `closed` | Quack (case voided directly) | `closed` | Event text "Closed because the case was voided."; DM; bump post; log `appeal_closed` (actor NULL). |
| `rejected`, `closed` | `reopened` | staff, reason | `needs_information` | Event; DM (asks for information); bump post; log. |
| any other | any | | unchanged | `409 appeal_changed`. |

- Every staff decision requires a reason of 1–2,000 characters. In Discord
  the Accept and Reject buttons open a modal prefilled with default text
  (`appeal.reason.default.accept`, `appeal.reason.default.reject`).
- Every staff transition carries `expected_version`; the row's `version`
  increments on each transition. A mismatch is `409 appeal_changed`.
- `decided_by`, `decided_at`, `decision_reason` hold the latest staff
  decision.
- Metrics: `quack.appeal.submitted`, `quack.appeal.transitions{decision}`,
  `quack.appeal.time_to_decision{decision}` (submission to first
  accept/reject/close), `quack.appeal.open` (gauge). Span event
  `appeal.transition`.

### 8.3 The appeal post

Each appeal has at most one tracked staff message (kind `appeal_post`,
§10) in the server's appeal channel. It is created when the appeal is
submitted, if `appeal_channel_id` is set, and re-rendered on every
transition. Content: `appeal.post.*` (§12.9) — member, case number and rule,
state, statement (first 1,500 characters; the rest on the dashboard), latest
decision and reason, and buttons **Accept**, **Reject** (only while
`pending`), and **Open appeal** (dashboard link). The post never names the
reviewer in text visible to the member (the member never sees it).

### 8.4 Appeal DMs

Each staff decision (and Quack's automatic close) sends exactly one
`appeal_update` DM (§6). Content per event in §12.8: the decision, the
reason as a quote, what happens next, a link to the member's case page
(`/guilds/{guild_id}/cases/{case_id}/appeal`), and for accepted appeals of
cases with a kick or ban, the server's rejoin invite when set. DMs never name
the staff member.

---

## 9. Moderation log

The moderation log (`mod_log`) is Quack's append-only record of who did what.
Reads and refusals about the person acting are not recorded.

### 9.1 Entries

Columns: `id`, `guild_id`, `kind` (`mod_log_kind`), `result`
(`mod_log_result`: `ok`, `failed`, `refused`), `actor_id` (NULL = Quack),
`source` (`mod_log_source`: `discord`, `dashboard`, `quack`, `imported`),
typed links `case_id`, `member_id`, `appeal_id`, `action_id`, `rule_id`,
`ticket_id`, `detail` (`jsonb`, typed per kind, below), `trace_id`,
`created_at`.

`quack_core::modlog::ModLogEntry` is a Rust enum with one variant per kind;
its serde form is `detail`. No variant has a free-text field except where
listed, so tokens and message content cannot enter by construction.

### 9.2 Kinds

"Posted" means the entry is posted to the log channel when one is set
(§9.4). Text ids are in §12.10.

| `mod_log_kind` | Result(s) | Links | `detail` | Posted |
| --- | --- | --- | --- | --- |
| `case_created` | ok | case, member, rule | `{source, case_number, level_index, level_from_case_count, case_count, action_kind\|null, timeout_seconds\|null, ban_delete_message_seconds\|null, sends_dm, replaces_case_number\|null}` | yes |
| `case_create_refused` | refused | member, rule | `{refusal: "quack_not_in_server"\|"quack_role_too_low"\|"quack_missing_permission", permission\|null}` | yes |
| `case_voided` | ok | case, member, appeal (when cause is appeal) | `{case_number, cause: "staff"\|"appeal_accepted", cancelled_action_ids: [uuid], undo_action_ids: [uuid]}` | yes |
| `case_context_edited` | ok | case, member | `{case_number, changed_keys: [string]}` | yes |
| `evidence_added` | ok | case, member | `{case_number, count}` | no |
| `action_done` | ok | case, member, action | `{case_number, kind, attempts, no_change}` | yes |
| `action_failed` | failed | case, member, action | `{case_number, kind, reason, permission\|null, http_status\|null, discord_code\|null, attempts}` | yes |
| `action_retry_requested` | ok | case, member, action | `{case_number, kind}` | yes |
| `action_dismissed` | ok | case, member, action | `{case_number, kind}` | yes |
| `action_undo_requested` | ok | case, member, action | `{case_number, kind}` (the undo's kind) | yes |
| `action_undo_skipped` | ok | case, member, action | `{case_number, kind, still_applies_case_number}` | yes |
| `dm_sent` | ok | case\|null, member, appeal\|null, ticket\|null | `{dm_kind}` | no |
| `dm_failed` | failed | case\|null, member, appeal\|null, ticket\|null | `{dm_kind, state: "failed"\|"unknown", failure\|null, discord_code\|null}` | yes |
| `appeal_submitted` | ok | case, member, appeal | `{case_number}` | yes |
| `appeal_information_requested` | ok | case, member, appeal | `{case_number}` | yes |
| `appeal_information_provided` | ok | case, member, appeal | `{case_number}` | yes |
| `appeal_accepted` | ok | case, member, appeal | `{case_number}` | yes |
| `appeal_rejected` | ok | case, member, appeal | `{case_number}` | yes |
| `appeal_closed` | ok | case, member, appeal | `{case_number, because_case_voided}` | yes |
| `appeal_reopened` | ok | case, member, appeal | `{case_number}` | yes |
| `rule_created` | ok | rule | `{name, version}` | yes |
| `rule_edited` | ok | rule | `{name, version}` | yes |
| `rule_archived` | ok | rule | `{name}` | yes |
| `rule_restored` | ok | rule | `{name}` | yes |
| `rule_imported` | ok | rule | `{name}` | yes |
| `settings_changed` | ok | — | `{fields: [string]}` (setting names, never values) | yes |
| `module_enabled` | ok | — | `{module}` | yes |
| `module_disabled` | ok | — | `{module, reason: null\|"channel_deleted"}` | yes |
| `module_settings_changed` | ok | — | `{module, fields: [string]}` | yes |
| `quack_joined` | ok | rule (starter, first join only) | `{first_join}` | yes |
| `quack_left` | ok | — | `{}` | no |
| `channel_setting_cleared` | ok | — | `{setting: "log_channel"\|"appeal_channel"\|"evidence_channel"\|"ticket_entry_channel"\|"ticket_queue_channel"\|"honeypot_channel"\|"activity_log_route"}` | yes |
| `log_channel_failed` | failed | — | `{entry_id, reason: "not_staff_only"\|"discord_refused"\|"retries_exhausted", discord_code\|null}` | no |
| `ticket_opened` | ok | member, ticket | `{}` | yes |
| `ticket_closed` | ok | member, ticket | `{transcript_complete}` | yes |
| `honeypot_case_failed` | failed | member, rule | `{refusal}` (a `MemberRefusal` or `"rule_unavailable"`) | yes |
| `legacy` | as imported | as imported | `{action, resource_type, resource_id, failure_reason}` (imported Go rows only, §19.4) | no |

`actor_id` is the staff member for staff operations, the member for
`appeal_submitted`, `appeal_information_provided`, `ticket_opened` (and
`ticket_closed` when the member closed it), and NULL for Quack.

### 9.3 Append-only enforcement

```sql
CREATE FUNCTION mod_log_reject_change() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  RAISE EXCEPTION 'mod_log is append-only' USING ERRCODE = 'insufficient_privilege';
END $$;
CREATE TRIGGER mod_log_no_update BEFORE UPDATE OR DELETE ON mod_log
  FOR EACH ROW EXECUTE FUNCTION mod_log_reject_change();
CREATE TRIGGER mod_log_no_truncate BEFORE TRUNCATE ON mod_log
  FOR EACH STATEMENT EXECUTE FUNCTION mod_log_reject_change();
```

Entries are written in the same transaction as the change they describe.
Bot-caused refusals (`case_create_refused`) are written in their own
transaction because the change rolled back.

### 9.4 Posting to the log channel

- When an entry of a posted kind commits and `guild_settings.log_channel_id`
  is set, the same transaction inserts a `post_mod_log` job keyed
  `mod_log_post:<entry_id>`. Changing the log channel later does not
  re-post old entries.
- The job renders the entry (§12.10) from current data, re-checks the
  channel is staff-only (§13.1.3), and creates the message with nonce = the
  entry ID in base64url (22 characters).
- Outcomes: `Done` → finished. Channel missing (404 Unknown Channel, or
  `log_channel_id` now NULL) → finished without posting; when the channel is
  missing, clear `log_channel_id` (guarded on the old value) and log
  `channel_setting_cleared`. Not staff-only or 403 → finished; log
  `log_channel_failed` once per outage (no other `log_channel_failed` for
  this server since the last successful post). `Unknown`/`RetryAfter`/5xx →
  retry with backoff 10 s, 30 s, 1 min, 2 min (5 attempts, all inside
  Discord's nonce window so a retry cannot post twice); exhausted →
  `log_channel_failed` (`retries_exhausted`).
- An `action_failed` post includes a **Retry** button (`1:ar:<action>`) while
  the action is `failed`, not dismissed, and its case is valid. It is not
  edited later; clicking it re-checks everything.
- Metric `quack.mod_log.posts{outcome}`.

### 9.5 Redaction

`detail` is typed (§9.2) and holds no message content, tokens, or Discord
error bodies. Free text that staff wrote (void reasons, decision reasons)
stays on its own row (`cases`, `appeal_events`) and is joined at render
time. Imported `legacy` rows pass through `quack_core::modlog::redact`:
JSON keys containing `token`, `secret`, `password`, `cookie`,
`authorization`, `webhook`, `session`, `access_key`, `private_key`,
`payload`, `message_content`, `member_content`, or `transcript` (after
lowercasing and replacing `-` with `_`) have their values replaced by
`"[REDACTED]"`, recursively; `failure_reason` strings containing `token=`,
`authorization:`, `bearer `, `password=`, `secret=`, or `cookie=`
(case-insensitive) become `"[REDACTED]"`, others are cut to 240 characters.

### 9.6 Reading

`GET /api/guilds/{guild_id}/mod-log` (capability `moderate`):

- Filters (all optional, combined with AND): `kind` (repeatable), `result`,
  `actor_id`, `member_id`, `case_number`, `rule_id`, `appeal_id`,
  `created_after` (inclusive), `created_before` (exclusive).
- Order: `created_at DESC, id DESC`. Keyset pagination: `limit` (1–100,
  default 50) and `cursor` (opaque base64url of the last row's
  `(created_at, id)`); the response has `next_cursor` (null on the last
  page). No offsets, no totals.
- Each item has the stored columns plus `text` (the rendered sentence from
  §12.10 for the dashboard) and `links` (dashboard paths).
- Indexes: `mod_log_guild_time`, and one partial index per link column
  (§15).

---

## 10. Tracked messages

A tracked message is one Discord message Quack keeps up to date: case
receipts, the staff post for each appeal, ticket panels, ticket staff
posts, and the honeypot warning. Each is a `messages` row.

### 10.1 The model

| Column | Meaning |
| --- | --- |
| `kind` | `case_receipt`, `appeal_post`, `ticket_panel`, `ticket_post`, `honeypot_warning`. |
| `owner_id` | The case, appeal, or ticket ID; NULL for one-per-server kinds (`ticket_panel`, `honeypot_warning`). |
| `channel_id`, `message_id` | Where it lives; `message_id` NULL until first posted. |
| `wanted_revision` | Incremented in the same transaction as any change the message shows. |
| `posted_revision` | The highest revision whose rendering is on Discord. |
| `interaction_token`, `token_expires_at` | For receipts posted as interaction responses: edits use the token while it is valid (15 minutes minus 60 seconds). |
| `private` | An ephemeral receipt: editable only through its token. |
| `state` | `active` or `gone`. |
| `stale_channel_id`, `stale_message_id` | The previous location after a move (below). |
| `last_error` | Machine code of the last failed sync, e.g. `http_403_50001`; never shown in Discord or on the dashboard. |

Uniqueness: one row per `(kind, owner_id)` when `owner_id` is set; one row
per `(guild_id, kind)` when it is NULL.

### 10.2 Bumping

`quack_db::messages::bump(tx, kind, owner)`:

```sql
UPDATE messages SET wanted_revision = wanted_revision + 1, updated_at = now()
WHERE kind = $1 AND owner_id = $2 AND state = 'active'
RETURNING id, wanted_revision;
```

followed by enqueueing `sync_message` keyed `message:<id>` (§16.3: if that
job exists it is coalesced; if it is running, it is marked to run again).
What bumps what:

| Change | Bumps |
| --- | --- |
| Action state change, DM settles, evidence settles, context edit, void, appeal submitted or decided, replacement created | the case's `case_receipt` |
| Appeal transition | the appeal's `appeal_post` |
| Ticket opened, closed, access fixed | the ticket's `ticket_post` |
| Ticket settings changed, tickets enabled or disabled | the server's `ticket_panel` |
| Honeypot case created, honeypot settings changed, honeypot enabled or disabled | the server's `honeypot_warning` |

### 10.3 The sync job

`quack_engine::messages::sync` (also called inline by the bot right after
creating a receipt):

1. Load the row; `gone` → `Done`. Note `rev = wanted_revision`.
2. Render from current data (§12) — never from the revision.
3. Write to Discord:
   - `message_id` NULL and a valid interaction token → edit the original
     interaction response through the token; store the returned message ID.
   - `message_id` NULL, no token → create the message with nonce
     `<row id base64url><rev mod 1000, 3 digits>` (25 characters).
   - `message_id` set, valid token (ephemeral receipts and receipts younger
     than the token) → edit through the token.
   - `message_id` set, no valid token → edit with bot credentials.
   - Ephemeral receipt (`private`) whose token expired → mark `gone`.
4. Record it:

   ```sql
   UPDATE messages
   SET posted_revision = GREATEST(posted_revision, $rev),
       message_id = COALESCE(message_id, $id), last_error = NULL, updated_at = now()
   WHERE id = $1;
   ```

5. If `wanted_revision > rev` now, repeat from step 1 (at most 3 loops per
   run; then return `Done` with the job marked to run again).

When the message or channel is gone (404 Unknown Message 10008, Unknown
Channel 10003, 403 Missing Access 50001):

| Kind | Action |
| --- | --- |
| `case_receipt` | `gone`. The case is unaffected. |
| `appeal_post` | Re-post in the current `appeal_channel_id` (new `message_id`). If none is set or that channel is also gone → `gone`. |
| `ticket_panel` | Re-post in the current entry channel. Entry channel gone → `gone` and tickets are disabled (§13.2). |
| `ticket_post` | Re-post in the current queue channel; none → `gone`. |
| `honeypot_warning` | Re-post in the honeypot channel; channel gone → `gone` and the honeypot is disabled (§13.4). |

**Moved messages.** When settings move a tracked message to another
channel (the ticket entry channel or the honeypot channel changes), the
settings transaction sets `stale_channel_id` and `stale_message_id` to the
old location, `channel_id` to the new one, `message_id = NULL`, and bumps.
The sync job posts in the new channel, then edits the stale message to
`ticket.panel.moved` (ticket panel) or deletes it (honeypot warning),
treating 404 as done, and clears the stale columns.

Other outcomes: `Unknown`, `RetryAfter`, 5xx → retry with backoff 2 s,
10 s, 1 min, 5 min, 30 min (6 attempts); exhausted → `last_error` set and
the job finishes; the next bump tries again. A definite 4xx not listed →
`last_error`, finish.

Metrics: `quack.message.syncs{kind,outcome}`; on success
`quack.message.sync.lag{kind}` (seconds from the bump's `updated_at` to
now).

---

## 11. Discord surface

### 11.1 Commands

All commands are guild-install only (`integration_types = [0]`,
`contexts = [0]`). Descriptions and option descriptions are catalog
messages (§12.1). No command has a default member permission except
`/setup` (Manage Server, `0x20`).

| Command | Type | Options (name, type, required, constraints) | Capability | Reply |
| --- | --- | --- | --- | --- |
| `/case add` | slash subcommand | `member` USER, required; `rule` STRING, required, autocomplete, max 100; `private` BOOLEAN, optional | `moderate` | Case receipt: public, or ephemeral when `private` is true. |
| `/case view` | slash subcommand | `number` INTEGER, required, min 1 | `moderate` | Ephemeral case card (§11.7). |
| `/case list` | slash subcommand | `member` USER, optional; `rule` STRING, optional, autocomplete (includes archived rules) | `moderate` | Ephemeral list, 10 per page, newest first. |
| `/case void` | slash subcommand | `number` INTEGER, required, min 1; `reason` STRING, required, 1–1,000 | `moderate` | Ephemeral result (§4.7). |
| `/case history` | slash subcommand | `member` USER, required | `moderate` | Ephemeral member history: totals and 10 cases per page. |
| `/appeals` | slash | none | `moderate` | Ephemeral list of `pending` appeals, oldest first, 10 per page, each with an **Open** button. |
| `/rules` | slash | none | `moderate` or `manage_rules` | Ephemeral list of active rules (name, levels summary) with a dashboard link per rule; 10 per page. |
| `/setup` | slash | none | `manage_settings` | Ephemeral guided setup panel (§11.8). |
| `/help` | slash | `topic` STRING, optional, choices `cases`, `rules`, `appeals`, `setup` | none | Ephemeral help text (§12.12). |
| Add case | message command | (the message) | `moderate` | Ephemeral rule picker, then an ephemeral receipt. The source message becomes evidence. |
| Add case | member command | (the member) | `moderate` | Ephemeral rule picker, then a public receipt posted by Quack in the channel. |

Autocomplete for `rule` options answers from the rules cache only
(§17.6): case-insensitive substring match on the name, at most 25 choices,
name as label, rule UUID as value. It returns no choices when the
interaction member lacks the capability (computed from the interaction's
member roles and the cached settings; 2FA is checked on submit).

### 11.2 Command registration

`quack commands sync` and `quack serve` (before connecting the gateway)
run the same function:

1. Build the definitions from `quack_discord::commands::definitions()`.
2. Scope: `QUACK_DEV_GUILD_ID` set → that server's commands; otherwise
   global commands.
3. `GET` the remote list. Canonicalize both sides (drop IDs, versions, and
   default fields; sort keys; keep option order).
4. Equal → do nothing. Different → one bulk overwrite
   (`PUT /applications/{app}/commands` or the guild variant) with every
   definition. This replaces removed Go-era commands (`/template …`, `/case
   failures|retry|dismiss|reverse|evidence|user`, "Add case for member",
   `/ui-preview`) and keeps IDs for unchanged names.
5. Store `name → command ID` in memory for command mentions in text
   (`</case view:ID>`).

A sync failure stops `serve` with a clear log line; `commands sync` exits 1.

### 11.3 Acknowledgement rules

Every interaction is acknowledged before any database or Discord REST call,
with one documented exception. Work happens after the ack.

| Interaction | Initial response | Visibility of the final answer |
| --- | --- | --- |
| `/case add` when the rule has a required field | Modal page 1, built from the rules cache. | Receipt follows the modal submit. |
| `/case add` otherwise | Deferred channel message (type 5). | Public, or ephemeral when `private`. |
| `/case view`, `/case list`, `/case void`, `/case history`, `/appeals`, `/rules`, `/setup` | Deferred channel message, ephemeral. | Ephemeral. |
| `/help` | Immediate message (type 4), ephemeral. Static text. | Ephemeral. |
| Message or member command "Add case" | Deferred channel message, ephemeral. | Picker and private receipt ephemeral; member-command receipt public (posted by Quack). |
| Autocomplete | Type 8 result from memory. | — |
| Button that opens a modal (Void, Add evidence, Accept, Reject, Appeal, Continue, setup modals) | Modal (no I/O: from custom ID, caches, or static text). | — |
| **Exception:** Edit context, and re-opening a draft page after a validation error | Modal prefilled after one primary-key read (`cases` or `case_drafts`) with a 300 ms deadline. Deadline missed → immediate ephemeral `interaction.busy` message. | — |
| Button or select on an ephemeral message | Deferred update (type 6). | Edits that ephemeral message. |
| Button on a shared message (receipt, log channel post, appeal post, ticket post, ticket panel) | Deferred channel message, ephemeral (type 5, flag 64). | New ephemeral message; the shared message changes only through tracked-message sync. |
| Modal submit | Deferred update when the modal came from an ephemeral message; otherwise deferred channel message (ephemeral, except the final `/case add` page, which follows `private`). | As stated. |

- The ack must go out within 2.5 seconds of the interaction's snowflake
  timestamp. `quack.interaction.ack.duration` measures receive-to-ack.
- After the ack, the handler answers by editing the original response
  (`PATCH /webhooks/{app}/{token}/messages/@original`) or with a follow-up.
- In DMs the ephemeral flag is omitted (Discord rejects it there).
- Duplicate deliveries (same interaction ID within 15 minutes) are dropped
  by an in-memory set before the ack. Writes are protected again by
  operation keys (§4.3).
- Unknown or legacy custom IDs: see §11.5.

### 11.4 What is ephemeral

Everything staff-facing is ephemeral, including errors. The only public
messages Quack sends in server channels are: the case receipt from `/case
add` without `private` and from the member command "Add case"; tracked staff
messages in staff-only channels (appeal posts, ticket posts, log channel
posts, evidence copies); the ticket panel; the ticket thread welcome; the
honeypot warning; server activity log posts. Every button on a public
message re-checks the clicker's capability and answers privately.

### 11.5 Custom IDs

Format: `1:<code>[:<arg>]…`, at most 100 characters. `1` is the scheme
version. UUIDs are 22 characters of unpadded base64url (`u22`). Snowflakes
and integers are decimal. `quack_discord::custom_id::CustomId` is an enum
with one variant per row; `encode` and `decode` are property-tested
(round trip, ≤ 100 characters for maximal arguments).

| Code | Component | Arguments | Behaviour |
| --- | --- | --- | --- |
| `cv` | Button "Open case" | case u22 | Show the case card. |
| `cvs` | Select "Open a case" (lists and history) | none; value: case u22 | Show the chosen case card. |
| `ce` | Button "Edit context" | case u22 | Open the edit-context modal (exception read). Rules with more than 5 fields: answer `case.context.too_many_fields` with a dashboard link instead. |
| `cem` | Modal | case u22 | Save context (§4.9). |
| `ea` | Button "Add evidence" | case u22 | Open the add-evidence modal (one paragraph input, one link per line). |
| `eam` | Modal | case u22 | Add evidence (§7.1). |
| `ev` | Button "View evidence", pager | case u22, item number (1-based) | Show one evidence item. |
| `vd` | Button "Void case" | case u22 | Open the void modal (reason). |
| `vdm` | Modal | case u22 | Void (§4.7). |
| `hi` | Button "History", pager | member snowflake, page | Member history page. |
| `cl` | Pager | page, member snowflake or `0`, rule u22 or `0` | Case list page. |
| `ar` | Button "Retry" | action u22 | Retry (§5.8). |
| `ad` | Button "Dismiss" | action u22 | Dismiss (§5.8). |
| `au` | Button "Remove timeout" / "Unban" | action u22 (the original) | Ask to confirm. |
| `auc` | Button "Yes, remove timeout" / "Yes, unban" | action u22 | Undo (§5.8). |
| `rp` | Select (rule picker) | draft u22 | Value: rule u22. Opens modal page 1 when the rule needs input, otherwise creates the case. |
| `rpp` | Pager | draft u22, page | Rule picker page (25 rules per page). |
| `cd` | Modal (case form page) | draft u22, rule u22, rule version, page | Save the page; finish or offer Continue. |
| `cn` | Button "Continue" | draft u22, rule u22, rule version, page | Open the next page's modal. |
| `cr` | Button "Try again" | draft u22, rule u22, rule version, page | Re-open the same page prefilled (exception read). |
| `ap` | Button "Appeal" (DM) | case u22 | Open the appeal modal. |
| `apm` | Modal | case u22 | Submit the appeal (§8.1). |
| `aa` | Button "Accept" | appeal u22, version | Open the accept modal (prefilled default reason). |
| `aam` | Modal | appeal u22, version | Accept. |
| `aj` | Button "Reject" | appeal u22, version | Open the reject modal. |
| `ajm` | Modal | appeal u22, version | Reject. |
| `ao` | Button "Open" | appeal u22 | Show the appeal card. |
| `al` | Pager | page | `/appeals` page. |
| `rl` | Pager | page | `/rules` page. |
| `su` | Setup controls | area, operation | §11.8. |
| `to` | Button "Open a ticket" | none | Open a ticket (§13.2). |
| `tc` | Button "Close ticket" | ticket u22 | Close the ticket. |
| `tf` | Button "Fix access" | ticket u22 | Fix ticket access. |

Longest form: `1:cd:<22>:<22>:<10>:<2>` = 64 characters.

**Legacy IDs** (messages posted by the Go version stay in Discord):

| Legacy ID | Handled as |
| --- | --- |
| `appeal:submit:v1:<ULID>` | `ap` with the case ID mapped from the ULID (§19.3). |
| `ticket:open:v1:<anything>` | `to`. |
| `case:<action>:v<n>:<payload containing a 26-character ULID>` | `cv` for the mapped case. |
| anything else | Ephemeral `interaction.legacy_button`. |

An undecodable `1:` ID answers `interaction.expired_button`.

### 11.6 Multi-page `/case add` drafts

A Discord modal holds at most 5 inputs. Rules with up to 10 fields use up to
2 pages. Drafts live in `case_drafts` so restarts and double clicks are
harmless.

**From `/case add`:**

1. The rule (from the rules cache) has a required field → respond with
   modal page 1 at once. The draft ID is generated in memory (UUIDv7) and
   carried in the modal's custom ID (`cd`). Right after the ack, insert the
   `case_drafts` row (actor, member, rule, version, `private`, `page = 1`,
   empty values, `expires_at = now + 15 min`). No required field → create
   the case directly with operation key = interaction ID.
2. Page modal contents: the fields of that page (5 per page, in rule order)
   — `short_text` and `number` as short text inputs, `long_text` and
   `message_link` as paragraph inputs, `boolean` as a Yes/No string select.
   Each is wrapped in a Label (label ≤ 45 characters, cut with "…"; the full
   label goes in the description, ≤ 100). Required flags and max lengths
   come from §4.6.
3. On a page submit (`cd`): defer, then

   ```sql
   UPDATE case_drafts SET values = values || $page_values, page = page + 1, updated_at = now()
   WHERE id = $1 AND page = $2 AND actor_id = $3 AND expires_at > now()
   RETURNING *;
   ```

   No row → the page was already submitted (double click), the draft
   expired, or (page 1 only) the row from step 1 has not been inserted yet.
   When the draft ID's UUIDv7 timestamp is less than 2 seconds old, wait
   500 ms and try once more. Then: if a case exists for operation key
   `draft:<id>`, show it; otherwise answer `case.draft.expired`.
4. Page values that fail §4.6 → answer `case.draft.page_invalid` listing the
   problems with a **Try again** button (`cr`); the draft page is not
   advanced.
5. More pages → answer `case.draft.page_saved` with a **Continue** button
   (`cn`) that opens the next modal (no I/O).
6. Last page → claim the draft
   (`DELETE FROM case_drafts WHERE id = $1 AND page = $2 RETURNING *` inside
   the case transaction) and apply the rule with operation key
   `draft:<id>`. The receipt's visibility follows the draft's `private`.
7. At every page submit, the rule's current version (rules cache) must
   still equal the draft's; otherwise answer `case.draft.rule_changed` and
   delete the draft.

**From the "Add case" message or member command:**

1. Defer ephemerally; check `moderate` + 2FA; insert a draft (member, source
   message for the message command, `private = true` for the message
   command and `false` for the member command, no rule yet).
2. Show the rule picker: a string select of active rules (25 per page,
   pager `rpp`), or `case.picker.no_rules` when there are none. With exactly
   one active rule, skip the picker and continue at step 3 with it.
3. On select (`rp`): set the draft's rule with
   `UPDATE … SET rule_id = $r, rule_version = $v WHERE id = $1 AND rule_id IS NULL`.
   When the rule has required fields left after auto-filling
   `message_link` fields from the source message, respond with modal page 1
   (as above). Otherwise defer and apply the rule with operation key
   `draft:<id>`.
4. Message command: the private receipt replaces the picker. Member
   command: the picker message becomes `case.receipt.posted` and Quack
   creates the public receipt in the channel as a tracked message (§10.3).

Expired drafts are deleted by housekeeping (§16.6).

### 11.7 The case card and receipt

Both render `case.card.*` (§12.4) from the same data; the receipt is the
card at creation time, kept current by tracked-message sync.

Buttons (2 rows maximum; disabled or hidden as stated):

| Button | Shown when |
| --- | --- |
| Edit context | always |
| Add evidence | always |
| View evidence | the case has evidence |
| History | always |
| Void case | case valid |
| Retry | the original or undo action is `failed`, not dismissed, and (for originals) the case is valid |
| Dismiss | an action is `failed` and not dismissed |
| Remove timeout | original timeout `done`, end in the future, no undo |
| Unban | original ban `done`, no undo |
| Open in dashboard | always (link) |

The public receipt shows the moderator and case context but never evidence
(§1.4).

### 11.8 Guided `/setup`

One ephemeral panel with one button per area. Each area shows its current
state and controls; every change is saved immediately through the same
engine calls as the dashboard and re-renders the panel with a result line.
Every click re-checks `manage_settings` + 2FA.

| Area (`su` area code) | State shown | Controls (`su` operation) |
| --- | --- | --- |
| Home (`home`) | One line per area below. | One button per area; "Open settings" dashboard link. |
| Log channel (`log`) | Channel or "Not set". | Channel select (text channels) `ch`; "Create #moderation-log" `new`; "Stop posting" `clr`. |
| Appeals (`ap`) | Appeal channel; rejoin invite set or not. | Channel select `ch`; "Create #appeals" `new`; "Set rejoin invite" `inv` (modal `invm`, prefilled from the settings cache); "Remove invite" `clr`. |
| Evidence copies (`ev`) | Channel or "Off". | Channel select `ch`; "Create #evidence" `new`; "Stop copying files" `clr`. |
| Tickets (`tk`) | On/off; entry and queue channels. | Entry channel select `entry`; queue channel select `queue`; "Create channels" `new` (creates `#support` and `#ticket-log`); "Turn on" `on`; "Turn off" `off`. |
| Honeypot (`hp`) | On/off; channel; rule; warning. | Channel select `ch`; "Create #honeypot" `new`; rule select `rule` (active rules without required fields, from the rules cache); "Edit warning" `warn` (modal `warnm`, prefilled from the settings cache); "Turn on" `on`; "Turn off" `off`. |
| Server activity log (`al`) | On/off; channel (or "Several channels" when routes differ). | Channel select `ch` (routes every event to it); "Create #server-log" `new`; "Turn on" `on`; "Turn off" `off`. |

Channel selects offer text channels only. A chosen staff destination must
pass §13.1.3; created channels get staff-only overwrites (§13.1.4).

### 11.9 Gateway intents and events

Intents (fixed at connect): `GUILDS`, `GUILD_MEMBERS` (privileged),
`GUILD_MODERATION`, `GUILD_MESSAGES`, `MESSAGE_CONTENT` (privileged). Both
privileged intents must be enabled in the developer portal. One shard
(`ShardId::ONE`); Discord requires sharding at 2,500 servers.

| Event | Handling |
| --- | --- |
| `READY` | Mark the gateway connected; record Quack's own account ID; clear the member cache. |
| `RESUMED` | Mark connected. |
| `GUILD_CREATE` | Update the guild cache; seed the member cache from the payload's members; bootstrap the server (§17.5); reconcile channel settings (§13.1.5); warm the rules and config caches. |
| `GUILD_UPDATE` | Update `guilds` (name, icon, owner, MFA level) when changed; server activity `server_changed`. |
| `GUILD_DELETE` | `unavailable = true` → ignore (outage). Otherwise `guilds.installed = false`, `left_at`; log `quack_left`; drop caches for the server. |
| `GUILD_ROLE_CREATE/UPDATE/DELETE` | Guild cache only. Deleted roles are ignored when reading staff roles. |
| `CHANNEL_CREATE/UPDATE` | Guild cache; server activity `channel_changed`. |
| `CHANNEL_DELETE` | Guild cache; clear settings that point at the channel (§13.1.5); server activity `channel_changed`. |
| `THREAD_DELETE` | A ticket's thread → close the ticket from its journal (§13.2.4). |
| `GUILD_MEMBER_ADD` | Member cache; server activity `member_joined`. |
| `GUILD_MEMBER_UPDATE` | Member cache. |
| `GUILD_MEMBER_REMOVE` | Member cache removal; server activity `member_left`. |
| `MESSAGE_CREATE` | Server activity message cache; ticket journal; honeypot trigger. |
| `MESSAGE_UPDATE` | Server activity `message_edited`. |
| `MESSAGE_DELETE`, `MESSAGE_DELETE_BULK` | Server activity `message_deleted` / `messages_bulk_deleted`; a deleted honeypot warning or ticket panel bumps its tracked message (re-post). |
| `GUILD_AUDIT_LOG_ENTRY_CREATE` | Ban (22) and unban (23) by someone other than Quack → server activity `member_banned` / `member_unbanned`. |
| `INTERACTION_CREATE` | Interaction router (§11.3). |

Dispatch: the shard loop only updates caches and hands events off.
Interactions go to a task pool (at most 64 concurrent handlers).
Other events go to 4 queues of 1,024 events, chosen by server ID, so one
server's events stay in order; a full queue drops the event and counts
`quack.gateway.queue.dropped{queue}`.

### 11.10 Caches

| Cache | Contents | Bound | Freshness | Size metric |
| --- | --- | --- | --- | --- |
| Discord guild cache (`twilight-cache-inmemory`, resource types `GUILD`, `CHANNEL`, `ROLE`, `USER_CURRENT`) | Server owner, MFA level, name, icon, channels with overwrites and parents, roles with positions and permissions | The servers Quack is in (at most 2,500 on one shard) | Gateway events | `quack.cache.size{cache="guilds"}` |
| Member cache (`quack-discord`) | (server, account) → roles, joined time, timeout end, bot flag, username, global name, avatar | 200,000 entries, LRU | Event-fed entries expire after 30 min, REST-fed after 5 min; removed on `GUILD_MEMBER_REMOVE`; "not a member" is never cached | `quack.cache.size{cache="members"}` |
| Rules cache (`quack-engine`) | Server → active rules with current definition | 100 per server (the limit) | Reloaded for a server on `NOTIFY quack_rules` | `quack.cache.size{cache="rules"}` |
| Config cache (`quack-engine`) | Server → `guild_settings` and module settings | One per installed server | Reloaded on `NOTIFY quack_settings` | `quack.cache.size{cache="config"}` |
| Server activity message cache (`quack-activity-log`) | (server, channel, message) → author, text, file names and sizes, embed types | 50,000 entries and 64 MiB estimated | 24 h; LRU | `quack.cache.size{cache="activity_messages"}` |
| Interaction dedupe set (`quack-bot`) | Interaction IDs | 100,000 | 15 min | `quack.cache.size{cache="interactions"}` |
| Directory cache (`quack-api`) | (server, account) → display data for the dashboard | 10,000 | 10 min | `quack.cache.size{cache="directory"}` |
| Account servers cache (`quack-api`) | Session → the account's Discord server list | 1,000 | 60 s | `quack.cache.size{cache="account_guilds"}` |

Every cache counts `quack.cache.lookups{cache,result}` and
`quack.cache.evictions{cache,cause}` (`capacity`, `expired`, `invalidated`).
Twilight's message, member, presence, voice, emoji, sticker, and
integration caches are off.

### 11.11 Rendering

`quack-text` returns a neutral `Msg`; `quack_discord::render` turns it into
a Components V2 message (flag `IS_COMPONENTS_V2`, no `content`, no embeds):

| `Msg` part | Components V2 |
| --- | --- |
| `Block::Heading(text)` | Text Display `### text` |
| `Block::Text(text)` | Text Display |
| `Block::Quote(text)` | Text Display, every line prefixed `> ` |
| `Block::Field { label, value }` | Text Display `**label:** value` |
| `Block::Subtext(text)` | Text Display `-# text` |
| `Block::Divider` | Separator |
| `Row::Buttons(Vec<Button>)` | Action Row, at most 5 buttons |
| `Row::Select(Select)` | Action Row with one select |
| `Msg::tone` | Container accent: none (neutral), green `0x3BA55C` (done), amber `0xFAA61A` (in progress or warning), red `0xED4245` (failed) |

- Everything sits in one Container. At most 40 components and 4,000
  characters of text per message. When text is longer, the renderer cuts
  Subtext first, then the longest Quote or Text block (with "…"), and adds
  `render.trimmed` ("Some details are only on the dashboard.").
- Dynamic text (names, reasons, context, statements, evidence text) is
  Markdown-escaped. Mentions render but never ping (`allowed_mentions`
  empty), except the ticket welcome, which pings its member.
- Button labels ≤ 80 characters, select option labels and descriptions
  ≤ 100, modal titles and Label labels ≤ 45.
- Discord timestamps use `<t:unix:f>` (full) and `<t:unix:R>` (relative).

---

## 12. Message catalog

Every user-facing string, keyed by a stable message id. `quack-text`
implements one function per id (`quack_text::case::card_heading(...)` for
`case.card.heading`), returning a `Msg` or a `String`, and has one `insta`
snapshot test per id and per listed variant. The dashboard renders API
fields that carry these texts (`message`, `text`, `outcome_text`) as given.

### 12.0 Notation

| Placeholder | Renders as (Discord) | Renders as (dashboard / API text) |
| --- | --- | --- |
| `{member}`, `{actor}`, `{author}` | Mention `<@id>` (never pings) | Display name (server nickname, else global name, else username), else the ID |
| `{actor}` when NULL | "Quack" | "Quack" |
| `{case}` | `**[case #N](<dashboard case URL>)**` in log posts; `case #N` elsewhere | "case #N" |
| `{rule}` | Rule name in bold | Rule name |
| `{channel}`, `{thread}` | Channel mention `<#id>` | `#name` |
| `{server}` | Server name in bold | Server name |
| `{ends_at}` | `<t:unix:f> (<t:unix:R>)` | Localized date and time |
| `{time}` | `<t:unix:R>` | Relative time |
| `{duration}` | `quack_text::duration`: the two largest whole units, e.g. "1 h 30 min", "2 d", "45 s" | same |
| `{permission}` | Discord's UI name: Moderate Members, Kick Members, Ban Members, Manage Messages, Manage Channels, Manage Threads, View Channel, Send Messages, Send Messages in Threads, Read Message History, Attach Files, Embed Links, Create Private Threads, View Audit Log | same |
| `{code}` | Discord JSON error code, or the HTTP status when there is none | same |

Plural variants are listed as "one: … / other: …". A line starting with
`>` is a quote block; `###` a heading; `-#` subtext.

### 12.1 Commands

| Id | Text |
| --- | --- |
| `command.case` | Add, view, and void cases. |
| `command.case.add` | Apply a rule to a member. |
| `command.case.add.member` | The member this case is about. |
| `command.case.add.rule` | The rule to apply. |
| `command.case.add.private` | Show the receipt only to you. |
| `command.case.view` | Show a case. |
| `command.case.number` | The case number. |
| `command.case.list` | List recent cases. |
| `command.case.list.member` | Only show cases about this member. |
| `command.case.list.rule` | Only show cases under this rule. |
| `command.case.void` | Void a case so it stops counting. |
| `command.case.void.reason` | Why the case is wrong. The member can see this reason. |
| `command.case.history` | Show a member's case history. |
| `command.case.history.member` | The member whose cases to show. |
| `command.appeals` | Review appeals waiting for a decision. |
| `command.rules` | List this server's rules. |
| `command.setup` | Set up Quack for this server. |
| `command.help` | Learn how Quack works. |
| `command.help.topic` | What you want to know about. |
| `command.help.topic.cases` | Cases |
| `command.help.topic.rules` | Rules |
| `command.help.topic.appeals` | Appeals |
| `command.help.topic.setup` | Setup |
| `command.add_case` | Add case |

### 12.2 Shared words

| Id | Text |
| --- | --- |
| `word.action.timeout` | timeout |
| `word.action.kick` | kick |
| `word.action.ban` | ban |
| `word.action.remove_timeout` | timeout removal |
| `word.action.unban` | unban |
| `word.outcome.in_progress` | In progress |
| `word.outcome.done` | Done |
| `word.outcome.failed` | Failed |
| `word.outcome.couldnt_confirm` | Couldn't confirm |
| `word.outcome.not_needed` | Not needed |
| `word.outcome.cancelled` | Cancelled |
| `word.outcome.left_in_place` | Left in place |
| `word.yes` | Yes |
| `word.no` | No |
| `word.voided` | Voided |
| `word.imported` | Imported |
| `word.page` | Page {page} of {pages} |
| `button.previous` | Previous |
| `button.next` | Next |
| `button.open_case` | Open case |
| `button.open_in_dashboard` | Open in dashboard |
| `select.open_case` | Open a case |
| `render.trimmed` | Some details are only on the dashboard. |
| `level.summary.item` | from case {from}: {outcome} |
| `level.outcome.warning` | warning |
| `level.outcome.timeout` | timeout for {duration} |
| `level.outcome.kick` | kick |
| `level.outcome.ban` | ban |
| `level.outcome.ban_delete` | ban and delete the last {duration} of messages |

### 12.3 Errors and refusals

Shown ephemerally in Discord; the same texts are the API `message` for the
matching error code (§14.6).

| Id | Text |
| --- | --- |
| `error.missing_capability.moderate` | Only moderators can do this. Ask a server administrator if you should be a moderator. |
| `error.missing_capability.manage_rules` | You need the Manage Server permission or a rules manager role to manage rules. |
| `error.missing_capability.manage_settings` | You need the Manage Server permission to change Quack's settings. |
| `error.missing_capability.manage_staff_roles` | Only the server owner and administrators can change the moderator roles. |
| `error.mfa_required` | This server requires two-factor authentication for moderation. Turn on 2FA in your Discord settings, then sign in to the Quack dashboard once so Quack can confirm it. |
| `error.actor_not_in_server` | You're not in this server anymore. |
| `error.quack_not_in_server` | Quack isn't in this server. Ask an administrator to add Quack again. |
| `error.discord_unavailable` | Quack couldn't reach Discord, so nothing changed. Try again in a moment. |
| `error.internal` | Something went wrong inside Quack, so nothing was saved. Try again in a moment. |
| `error.unconfirmed` | Quack couldn't confirm whether that was saved. Check before you try again. |
| `error.render_failed` | Case #{number} is saved, but Quack couldn't show it here. Use /case view number:{number} to see it. |
| `error.case_not_found` | There's no case #{number} in this server. |
| `error.rule_not_found` | That rule doesn't exist anymore. Choose another rule. |
| `error.rule_archived` | The rule {rule} is archived, so it can't be applied. Choose an active rule. |
| `error.case_already_voided` | Case #{number} is already voided. |
| `error.case_voided` | Case #{number} is voided, so Quack won't run its {action}. Add a new case if it still applies. |
| `error.action_not_retryable` | This {action} isn't failed anymore, so there's nothing to retry. |
| `error.action_not_dismissable` | This {action} isn't failed anymore, so there's nothing to dismiss. |
| `error.action_not_undoable` | There's nothing to undo: the {action} isn't in place, or Quack is already undoing it. |
| `error.timeout_window_passed` | The timeout's end time has passed. Add a new case if {member} still needs a timeout. |
| `error.appeal_not_found` | That appeal doesn't exist in this server. |
| `error.appeal_changed` | This appeal changed since you opened it. Open it again to see where it stands. |
| `error.evidence_limit_reached` | Case #{number} already has {count} evidence items. A case can have at most 50. |
| `error.too_many_links` | Add at most 10 message links at a time. |
| `error.replacement_invalid` | Case #{number} can't be replaced: only a voided case in this server that has no replacement yet can be. |
| `interaction.busy` | Quack is busy right now. Try again in a moment. |
| `interaction.expired_button` | This button has expired. Run the command again. |
| `interaction.legacy_button` | This button is from an older version of Quack and no longer works. Use the commands or the dashboard instead. |

Member refusals when applying a rule (`refusal.*`; the first sentence is
always "No case was added."):

| Id | Text |
| --- | --- |
| `refusal.quack_not_in_server` | No case was added. Quack isn't in this server. |
| `refusal.actor_not_in_server` | No case was added. You're not in this server anymore. |
| `refusal.member_not_in_server` | No case was added. {member} isn't in this server. Choose a current member. |
| `refusal.member_is_self` | No case was added. You can't add a case about yourself. Ask another moderator. |
| `refusal.member_is_bot` | No case was added. {member} is a bot, and cases are only for people. |
| `refusal.member_is_owner` | No case was added. {member} owns the server. |
| `refusal.member_role_too_high` | No case was added. {member}'s highest role is at or above yours. Ask a moderator with a higher role. |
| `refusal.quack_role_too_low` | No case was added. {member}'s highest role is at or above Quack's. Ask an administrator to move Quack's role above it. |
| `refusal.quack_missing_permission` | No case was added. This rule would {action_verb} {member}, but Quack doesn't have the {permission} permission. Ask an administrator to give it to Quack's role. |
| `refusal.member_is_administrator` | No case was added. This rule would time out {member}, but Discord doesn't allow timeouts for members with the Administrator permission. |

`{action_verb}`: `time out`, `kick`, `ban`.

### 12.4 Case card and receipt

The card (staff, ephemeral) and the receipt (public or private) use the same
lines, in this order; lines whose condition is false are omitted.

| Id | Text | Condition |
| --- | --- | --- |
| `case.card.heading` | ### Case #{number} · {rule} | valid |
| `case.card.heading_voided` | ### Case #{number} · {rule} · Voided | voided |
| `case.card.member` | {member} · level {level_index} (starts at case {from}) · case count {case_count} | not imported |
| `case.card.member_imported` | {member} · imported from Quack v4 | imported |
| `case.card.reason` | > {reason} | always |
| `case.card.action` | **Action:** {action sentence, §12.6} | always |
| `case.card.undo` | **Undo:** {undo sentence, §12.6} | an undo exists |
| `case.card.dm` | **DM:** {DM sentence, §12.7} | not imported |
| `case.card.evidence.saving` | **Evidence:** Saving evidence… | any item `saving` |
| `case.card.evidence.saved` | **Evidence:** one: 1 item saved. / other: {count} items saved. | all items settled, none failed |
| `case.card.evidence.partial` | **Evidence:** {saved} saved, {failed} couldn't be saved. Open View evidence to see why. | some failed |
| `case.card.context_heading` | **Case context** | the case has context |
| `case.card.context_line` | **{label}:** {value} (text cut to 300 characters with "…"; booleans Yes/No) | per answered field |
| `case.card.voided` | Voided by {actor} {time}. | voided |
| `case.card.void_reason` | > {void_reason} | voided |
| `case.card.replaces` | Replaces case #{other}. | replaces another |
| `case.card.replaced_by` | Replaced by case #{other}. | has a replacement |
| `case.card.appeal.pending` | **Appeal:** waiting for a decision. | appeal pending |
| `case.card.appeal.needs_information` | **Appeal:** waiting for {member} to add information. | |
| `case.card.appeal.accepted` | **Appeal:** accepted. | |
| `case.card.appeal.rejected` | **Appeal:** rejected. | |
| `case.card.appeal.closed` | **Appeal:** closed. | |
| `case.card.imported_action` | -# The original Quack v4 action was {v4_action}. Quack took no action for this case. | imported |
| `case.card.footer` | -# Added by {moderator} {time} | moderator set |
| `case.card.footer_honeypot` | -# Added by Quack {time} after a post in the honeypot channel | honeypot |
| `case.card.footer_imported` | -# Imported {time}. Original moderator: {v4_moderator} | imported |

Buttons (labels): `case.button.edit_context` "Edit context",
`case.button.add_evidence` "Add evidence", `case.button.view_evidence`
"View evidence", `case.button.history` "History", `case.button.void` "Void
case", `case.button.retry` "Retry", `case.button.dismiss` "Dismiss",
`case.button.remove_timeout` "Remove timeout", `case.button.unban` "Unban",
`button.open_in_dashboard`.

| Id | Text |
| --- | --- |
| `case.receipt.posted` | Case #{number} is saved. Quack posted the receipt in this channel. |
| `case.receipt.post_failed` | Case #{number} is saved. Quack couldn't post the receipt in this channel, so here it is privately. |
| `case.receipt.replayed` | Case #{number} was already saved by this command. |

### 12.5 Adding a case: picker, form, drafts, context problems

| Id | Text |
| --- | --- |
| `case.picker.prompt` | Choose a rule for the case about {member}. |
| `case.picker.placeholder` | Choose a rule |
| `case.picker.no_rules` | This server has no active rules yet. Ask a rules manager to create one on the dashboard. |
| `case.form.title` | {rule} ({page}/{pages}) — cut to 45 characters |
| `case.form.boolean_placeholder` | Choose Yes or No |
| `case.form.number_placeholder` | A number, like 3 or 2.5 |
| `case.form.link_placeholder` | https://discord.com/channels/… |
| `case.draft.page_saved` | Page {page} of {pages} is saved in your draft. Continue to finish the case. |
| `case.draft.button.continue` | Continue |
| `case.draft.button.try_again` | Try again |
| `case.draft.page_invalid` | Some answers need fixing. Nothing on this page was saved:\n{problems, one per line} |
| `case.draft.expired` | This form expired after 15 minutes. Run /case add again. |
| `case.draft.rule_changed` | The rule {rule} changed while you were filling in the form. Run /case add again to use the new version. |
| `context.required` | {label}: an answer is required. |
| `context.too_long` | {label}: use at most {max} characters. |
| `context.not_yes_no` | {label}: choose Yes or No. |
| `context.not_a_number` | {label}: enter a number, like 3 or 2.5. |
| `context.number_out_of_range` | {label}: use a number no bigger than 1,000,000,000,000,000 with at most 6 decimal places. |
| `context.not_a_message_link` | {label}: paste a Discord message link. Right-click the message and choose Copy Message Link. |
| `context.other_server` | {label}: that message is in another server. Use a message from this server. |
| `context.unknown_field` | {key} isn't a field of this rule. |
| `context.duplicate_field` | {key} is answered more than once. |

### 12.6 Actions

Action sentences for staff views, receipts, the dashboard, and the log
channel. `{retry}` appends `action.retry_scheduled` when the action is
`pending` after at least one attempt.

| Id | Text |
| --- | --- |
| `action.none` | No action. This case is a warning. |
| `action.retry_scheduled` | Discord didn't confirm the last try, so Quack will try again {time}. |
| `action.timeout.in_progress` | Quack is timing out {member} until {ends_at}.{retry} |
| `action.timeout.done` | {member} is timed out until {ends_at}. |
| `action.timeout.failed` | Quack couldn't time out {member}. {reason} |
| `action.timeout.cancelled` | Quack didn't time out {member} because the case was voided first. |
| `action.kick.in_progress` | Quack is removing {member} from the server.{retry} |
| `action.kick.done` | Quack removed {member} from the server. |
| `action.kick.done_no_change` | {member} had already left the server. |
| `action.kick.failed` | Quack couldn't remove {member} from the server. {reason} |
| `action.kick.cancelled` | Quack didn't remove {member} because the case was voided first. |
| `action.ban.in_progress` | Quack is banning {member}.{retry} |
| `action.ban.done` | Quack banned {member}. |
| `action.ban.done_delete` | Quack banned {member} and deleted their messages from the last {duration}. |
| `action.ban.failed` | Quack couldn't ban {member}. {reason} |
| `action.ban.cancelled` | Quack didn't ban {member} because the case was voided first. |
| `action.remove_timeout.in_progress` | Quack is removing {member}'s timeout.{retry} |
| `action.remove_timeout.done` | Quack removed {member}'s timeout. |
| `action.remove_timeout.done_no_change` | {member}'s timeout had already ended. |
| `action.remove_timeout.failed` | Quack couldn't remove {member}'s timeout. {reason} |
| `action.remove_timeout.skipped` | Quack left {member}'s timeout in place because case #{other} still applies. |
| `action.unban.in_progress` | Quack is unbanning {member}.{retry} |
| `action.unban.done` | Quack unbanned {member}. |
| `action.unban.done_no_change` | {member} wasn't banned anymore, so there was nothing to undo. |
| `action.unban.failed` | Quack couldn't unban {member}. {reason} |
| `action.unban.skipped` | Quack left {member}'s ban in place because case #{other} still applies. |
| `action.dismissed_suffix` | Dismissed by {actor} {time}. |

Failure reasons (`{reason}`):

| Id | Text |
| --- | --- |
| `action.failure.quack_not_in_server` | Quack isn't in this server anymore. |
| `action.failure.quack_missing_permission` | Quack needs the {permission} permission. Ask an administrator to give it to Quack's role, then retry. |
| `action.failure.quack_role_too_low` | {member}'s highest role is at or above Quack's. Ask an administrator to move Quack's role above it, then retry. |
| `action.failure.member_is_owner` | {member} owns the server, and Discord doesn't let anyone time out, kick, or ban the owner. |
| `action.failure.member_not_in_server` | {member} isn't in the server, so there's no one to time out. |
| `action.failure.member_is_administrator` | {member} has the Administrator permission, and Discord doesn't allow timeouts for administrators. |
| `action.failure.timeout_window_passed` | The timeout would already have ended by now, so Quack didn't apply it. |
| `action.failure.discord_refused` | Discord didn't let Quack do this (error {code}). Check Quack's role and permissions, then retry. |
| `action.failure.retries_exhausted` | Quack tried {attempts} times but couldn't confirm the change with Discord. Check {member} in Discord before you retry. |

Member-facing action sentences (dashboard member view; `you`):

| Id | Text |
| --- | --- |
| `member.action.none` | This case is a warning. No other action was taken. |
| `member.action.timeout.in_progress` | Quack is timing you out until {ends_at}. |
| `member.action.timeout.done` | You're timed out until {ends_at}. |
| `member.action.timeout.not_applied` | The timeout wasn't applied. |
| `member.action.kick.in_progress` | Quack is removing you from the server. |
| `member.action.kick.done` | You were removed from the server. |
| `member.action.kick.not_applied` | You weren't removed from the server. |
| `member.action.ban.in_progress` | Quack is banning you from the server. |
| `member.action.ban.done` | You're banned from the server. |
| `member.action.ban.not_applied` | The ban wasn't applied. |
| `member.action.remove_timeout.done` | Your timeout was removed. |
| `member.action.remove_timeout.in_progress` | Quack is removing your timeout. |
| `member.action.remove_timeout.skipped` | Your timeout stays because another case still applies. |
| `member.action.remove_timeout.not_applied` | Your timeout hasn't been removed yet. The moderators can see why. |
| `member.action.unban.done` | You're no longer banned. |
| `member.action.unban.in_progress` | Quack is unbanning you. |
| `member.action.unban.skipped` | Your ban stays because another case still applies. |
| `member.action.unban.not_applied` | Your ban hasn't been removed yet. The moderators can see why. |

`not_applied` covers `failed` (and `cancelled` for original actions).

Staff controls:

| Id | Text |
| --- | --- |
| `action.retry.done` | Quack will try the {action} for case #{number} again. The case shows the result. |
| `action.dismiss.done` | The failed {action} for case #{number} is dismissed. The case keeps its failed attempts. |
| `action.undo.confirm.remove_timeout` | Remove {member}'s timeout from case #{number}? |
| `action.undo.confirm.unban` | Unban {member}? Their ban came from case #{number}. |
| `action.undo.button.remove_timeout` | Yes, remove timeout |
| `action.undo.button.unban` | Yes, unban |
| `action.undo.queued.remove_timeout` | Quack is removing {member}'s timeout. Case #{number} shows the result. |
| `action.undo.queued.unban` | Quack is unbanning {member}. Case #{number} shows the result. |

Voiding:

| Id | Text |
| --- | --- |
| `case.void.modal.title` | Void case #{number} — or "Void case" when the number is unknown |
| `case.void.modal.label` | Why is this case wrong? |
| `case.void.modal.description` | The member can see this reason. |
| `case.void.done` | Case #{number} is voided. It stays in the history but no longer counts toward {rule}. |
| `case.void.undo.remove_timeout` | Quack is removing {member}'s timeout. |
| `case.void.undo.unban` | Quack is unbanning {member}. |
| `case.void.cancelled_action` | Quack won't run the {action}. |
| `case.void.running_action` | Quack had already started the {action}. If Discord confirms it, Quack will undo it. |
| `case.void.appeal_closed` | The open appeal is closed, and {member} gets a DM about it. |

Context and evidence:

| Id | Text |
| --- | --- |
| `case.context.modal.title` | Edit context · case #{number} |
| `case.context.saved` | The context of case #{number} is saved. |
| `case.context.saved_new_evidence` | The context of case #{number} is saved. one: Quack is saving 1 new evidence item. / other: Quack is saving {count} new evidence items. |
| `case.context.too_many_fields` | Case #{number} has more context fields than fit in a Discord form. Edit it on the dashboard. |
| `case.context.no_fields` | The rule for case #{number} has no context fields. |
| `evidence.modal.title` | Add evidence · case #{number} |
| `evidence.modal.label` | Message links |
| `evidence.modal.description` | One Discord message link per line, up to 10. |
| `evidence.added` | one: Quack is saving 1 evidence item for case #{number}. / other: Quack is saving {count} evidence items for case #{number}. Open View evidence to check each one. |
| `evidence.added.already_saved` | one: 1 of these was already saved. / other: {count} of these were already saved. |
| `evidence.view.heading` | ### Evidence {item} of {total} · case #{number} |
| `evidence.view.message` | Message from {author} in {channel}, sent {time} · [Open message]({url}) |
| `evidence.view.edited` | Edited {time}. |
| `evidence.view.text` | > {text} |
| `evidence.view.embeds` | Link previews: {titles or types, comma-separated} |
| `evidence.view.saving` | Quack is still saving this message. |
| `evidence.view.none` | Case #{number} has no evidence yet. Use Add evidence to add message links. |
| `evidence.failure.message_deleted` | Quack couldn't save this message because it was deleted before Quack fetched it. |
| `evidence.failure.moderator_no_access` | Quack didn't save this message because the moderator who added it can't read that channel. |
| `evidence.failure.quack_no_access` | Quack couldn't save this message because it can't read that channel. Give Quack View Channel and Read Message History there, then add the link again. |
| `evidence.failure.other_server` | Quack didn't save this message because it's in another server. |
| `evidence.failure.discord_error` | Quack couldn't save this message because Discord kept failing. Add the link again to retry. |
| `evidence.file.copied` | {name} · saved copy |
| `evidence.file.pending` | {name} · saving… |
| `evidence.file.link_only` | {name} · original link only, because no evidence channel is set |
| `evidence.file.too_large` | {name} · original link only, because Quack copies files up to 10 MB |
| `evidence.file.unsupported_type` | {name} · original link only, because Quack copies only images, video, audio, text, and PDF files |
| `evidence.file.copy_failed` | {name} · Quack couldn't copy this file, so the original link may stop working |
| `evidence.file.limit_reached` | {name} · not saved, because a case keeps at most 100 files and 10 per message |
| `evidence.channel_not_staff_only` | The evidence channel is no longer staff-only, so Quack didn't copy files there. |

### 12.7 DM outcome sentences

Staff-facing (receipt, card, dashboard staff view, log channel). `{because}`
is a §6.5 fragment.

| Id | Text |
| --- | --- |
| `dm.state.in_progress` | Quack is sending {member} a DM. |
| `dm.state.sent` | Quack sent {member} a DM. |
| `dm.state.failed` | Quack couldn't send {member} a DM {because}. They can still see the case by signing in to the dashboard. |
| `dm.state.unknown` | Quack couldn't confirm whether {member} got the DM. Discord didn't answer in time, and Quack won't send it again so it can't arrive twice. They can still see the case by signing in to the dashboard. |
| `dm.state.not_needed.level` | This level doesn't send a DM. |
| `dm.state.not_needed.voided` | Quack didn't send a DM because the case was voided first. |
| `dm.failure.dms_closed` | because their DMs are closed or they blocked Quack |
| `dm.failure.not_in_shared_server` | because they no longer share a server with Quack |
| `dm.failure.dm_channel_unavailable` | because Discord didn't open a DM channel |
| `dm.failure.discord_refused` | because Discord refused the message (error {code}) |

Combined example (failed DM on a receipt): "**DM:** Quack couldn't send
@Sam a DM because their DMs are closed or they blocked Quack. They can still
see the case by signing in to the dashboard."

Member-facing (dashboard member view):

| Id | Text |
| --- | --- |
| `member.dm.in_progress` | Quack is sending you a DM about this case. |
| `member.dm.sent` | Quack sent you a DM about this case. |
| `member.dm.failed` | Quack couldn't send you a DM about this case, so it's shown here instead. |
| `member.dm.unknown` | Quack couldn't confirm whether you got a DM about this case. Everything in it is shown here. |
| `member.dm.not_needed` | This case didn't need a DM. |

### 12.8 DMs to members

`dm.case` (one `Msg`; lines in order, omitted when the condition is false):

| Id | Text | Condition |
| --- | --- | --- |
| `dm.case.heading` | ### You have a new case in {server} | always |
| `dm.case.rule` | Rule: {rule} | always |
| `dm.case.reason` | > {reason} | always |
| `dm.case.intro` | {dm_intro} (the server's text, Markdown allowed, mentions suppressed) | set |
| `dm.case.details_heading` | **Details** | the case has non-link context |
| `dm.case.detail_line` | **{label}:** {value} (booleans Yes/No; `message_link` fields never shown) | per field |
| `dm.case.outcome.warning` | **What happens:** This is a warning. No other action is taken. | no action |
| `dm.case.outcome.timeout` | **What happens:** You're timed out until {ends_at}. | timeout |
| `dm.case.outcome.kick` | **What happens:** You're being removed from the server. | kick |
| `dm.case.outcome.ban` | **What happens:** You're being banned from the server. | ban |
| `dm.case.appeal` | If you think this case is wrong, you can appeal it once. | appealable |
| `dm.case.not_appealable` | This case can't be appealed. | not appealable |
| `dm.case.footer_text` | {dm_footer} (the server's text) | set |
| `dm.case.subtext` | -# Case #{number} · {time} | always |
| `dm.case.button.appeal` | Appeal | appealable (custom ID `1:ap:<case>`) |
| `dm.case.button.dashboard` | Open in dashboard | always (link to `/guilds/{guild_id}/cases/{case_id}/appeal`) |

`dm.appeal.*` (one per decision; every one ends with the subtext
`-# Case #{number} · {server}` and a link button `dm.appeal.button` "View
appeal", or "Reply on the dashboard" for information requests):

| Id | Heading | Body |
| --- | --- | --- |
| `dm.appeal.information_requested` | ### The moderators of {server} need more information about your appeal | > {reason}\nReply on the dashboard to continue your appeal for case #{number}. |
| `dm.appeal.reopened` | ### The moderators of {server} reopened your appeal | > {reason}\nThey need more information. Reply on the dashboard to continue your appeal for case #{number}. |
| `dm.appeal.accepted` | ### Your appeal for case #{number} in {server} was accepted | > {reason}\nThe case no longer counts against you. If it came with a timeout or ban, Quack is removing it. |
| `dm.appeal.accepted.rejoin_ban` | (appended) | Once your ban is removed, you can rejoin with this invite: {invite} |
| `dm.appeal.accepted.rejoin_kick` | (appended) | You can rejoin the server with this invite: {invite} |
| `dm.appeal.rejected` | ### Your appeal for case #{number} in {server} was rejected | > {reason}\nThe case still counts. |
| `dm.appeal.closed` | ### Your appeal for case #{number} in {server} was closed | > {reason} |
| `dm.appeal.closed_case_voided` | ### Your appeal for case #{number} in {server} was closed | The case was voided, so there's nothing left to appeal. It no longer counts against you. |
| `dm.appeal.button.reply` | Reply on the dashboard | information requested, reopened |

### 12.9 Appeals

Member side:

| Id | Text |
| --- | --- |
| `appeal.form.title` | Appeal this case |
| `appeal.form.label` | What should the moderators reconsider? |
| `appeal.form.description` | Explain what happened. You can appeal each case once. |
| `appeal.submitted` | Your appeal for case #{number} is submitted. Follow it on the dashboard; Quack will also try to DM you when the moderators decide. |
| `appeal.submit.exists` | You already appealed case #{number}. Open your appeal to check where it stands. |
| `appeal.submit.not_allowed` | Case #{number} can't be appealed. |
| `appeal.submit.voided` | Case #{number} was voided, so there's nothing to appeal. |
| `appeal.submit.not_yours` | This button isn't for your account. Sign in to the dashboard to see your own cases. |
| `appeal.information.saved` | Your answer is added to the appeal for case #{number}. The moderators will review it. |
| `appeal.information.not_requested` | The moderators haven't asked for more information on this appeal. |

Staff side (appeal post, appeal card, `/appeals`):

| Id | Text |
| --- | --- |
| `appeal.post.heading` | ### Appeal for case #{number} · {rule} |
| `appeal.post.from` | From {member} · submitted {time} |
| `appeal.post.state.pending` | Waiting for a decision. |
| `appeal.post.state.needs_information` | Waiting for {member} to add information. |
| `appeal.post.state.accepted` | Accepted by {actor} {time}. |
| `appeal.post.state.rejected` | Rejected by {actor} {time}. |
| `appeal.post.state.closed` | Closed by {actor} {time}. |
| `appeal.post.state.closed_case_voided` | Closed {time} because the case was voided. |
| `appeal.post.statement_heading` | **Statement** |
| `appeal.post.statement` | > {statement, first 1,500 characters} |
| `appeal.post.statement_cut` | -# The full statement is on the dashboard. |
| `appeal.post.information_heading` | **Added information** |
| `appeal.post.reason_heading` | **Reason given to the member** |
| `appeal.button.accept` | Accept |
| `appeal.button.reject` | Reject |
| `appeal.button.open` | Open appeal |
| `appeal.modal.accept.title` | Accept appeal |
| `appeal.modal.reject.title` | Reject appeal |
| `appeal.modal.reason.label` | Reason the member will see |
| `appeal.reason.default.accept` | We reviewed your appeal and voided the case. |
| `appeal.reason.default.reject` | We reviewed your appeal, and the case stands. |
| `appeal.decided.accepted` | You accepted the appeal for case #{number}. The case is voided, and {member} gets a DM. |
| `appeal.decided.rejected` | You rejected the appeal for case #{number}. {member} gets a DM. |
| `appeals.heading` | ### Appeals waiting for a decision |
| `appeals.row` | Case #{number} · {member} · {rule} · submitted {time} |
| `appeals.empty` | No appeals are waiting for a decision. |
| `appeals.button.open` | Open |

### 12.10 Log channel entries

Each post is: the lead sentence below, an optional quote, and the subtext
`-# {time} · [Open in dashboard](<link>)`. `{verb}` for actions:
`time out`, `remove from the server`, `ban`, `remove the timeout of`,
`unban`.

| Id | Kind / result | Text |
| --- | --- | --- |
| `log.case_created` | `case_created` ok | {actor} added {case} for {member} under {rule}: level {level_index} (starts at case {from}), case count {case_count}, {level outcome}. |
| `log.case_created.honeypot` | `case_created` ok, source honeypot | Quack added {case} for {member} under {rule} because they posted in the honeypot channel: {level outcome}. |
| `log.case_created.replaces` | (appended) | It replaces case #{other}. |
| `log.case_create_refused.quack_role_too_low` | `case_create_refused` refused | Quack couldn't add a case for {member} under {rule}: their highest role is at or above Quack's. Move Quack's role above it. |
| `log.case_create_refused.quack_missing_permission` | `case_create_refused` refused | Quack couldn't add a case for {member} under {rule}: Quack needs the {permission} permission. |
| `log.case_create_refused.quack_not_in_server` | `case_create_refused` refused | Quack couldn't add a case for {member} under {rule} because Quack isn't in the server. |
| `log.case_voided` | `case_voided` ok, cause staff | {actor} voided {case} for {member}.\n> {void_reason} |
| `log.case_voided.appeal` | `case_voided` ok, cause appeal | {actor} accepted {member}'s appeal, so {case} is voided. |
| `log.case_context_edited` | `case_context_edited` ok | {actor} edited the case context of {case}. |
| `log.evidence_added` | `evidence_added` ok (not posted) | {actor} added one: 1 evidence item / other: {count} evidence items to {case}. |
| `log.action_done.timeout` | `action_done` ok | Quack timed out {member} until {ends_at} for {case}. |
| `log.action_done.kick` | `action_done` ok | Quack removed {member} from the server for {case}. |
| `log.action_done.kick_no_change` | `action_done` ok | {member} had already left when Quack tried to remove them for {case}. |
| `log.action_done.ban` | `action_done` ok | Quack banned {member} for {case}. |
| `log.action_done.remove_timeout` | `action_done` ok | Quack removed {member}'s timeout from {case}. |
| `log.action_done.remove_timeout_no_change` | `action_done` ok | {member}'s timeout from {case} had already ended. |
| `log.action_done.unban` | `action_done` ok | Quack unbanned {member}. The ban came from {case}. |
| `log.action_done.unban_no_change` | `action_done` ok | {member} wasn't banned anymore when Quack tried to undo the ban from {case}. |
| `log.action_failed` | `action_failed` failed | Quack couldn't {verb} {member} for {case}. {reason} — with a **Retry** button while retryable |
| `log.action_retry_requested` | `action_retry_requested` ok | {actor} asked Quack to try the {action} for {case} again. |
| `log.action_dismissed` | `action_dismissed` ok | {actor} dismissed the failed {action} for {case}. |
| `log.action_undo_requested.remove_timeout` | `action_undo_requested` ok | {actor} asked Quack to remove {member}'s timeout from {case}. |
| `log.action_undo_requested.unban` | `action_undo_requested` ok | {actor} asked Quack to unban {member}. The ban came from {case}. |
| `log.action_undo_skipped.timeout` | `action_undo_skipped` ok | Quack left {member}'s timeout in place because case #{other} still applies, even though {case} was voided. |
| `log.action_undo_skipped.ban` | `action_undo_skipped` ok | Quack left {member}'s ban in place because case #{other} still applies, even though {case} was voided. |
| `log.dm_sent.case` | `dm_sent` ok (not posted) | Quack sent {member} a DM about {case}. |
| `log.dm_sent.appeal` | `dm_sent` ok (not posted) | Quack sent {member} a DM about their appeal for {case}. |
| `log.dm_sent.ticket` | `dm_sent` ok (not posted) | Quack sent {member} the transcript of their ticket. |
| `log.dm_failed.case` | `dm_failed` failed, state failed | Quack couldn't send {member} a DM about {case} {because}. They can still see the case by signing in to the dashboard. |
| `log.dm_failed.case_unknown` | `dm_failed` failed, state unknown | Quack couldn't confirm whether {member} got the DM about {case}. Quack won't send it again so it can't arrive twice. They can still see the case by signing in to the dashboard. |
| `log.dm_failed.appeal` | `dm_failed` failed, state failed | Quack couldn't send {member} a DM about their appeal for {case} {because}. They can still see the decision on the dashboard. |
| `log.dm_failed.appeal_unknown` | `dm_failed` failed, state unknown | Quack couldn't confirm whether {member} got the DM about their appeal for {case}. They can still see the decision on the dashboard. |
| `log.dm_failed.ticket` | `dm_failed` failed | Quack couldn't send {member} the transcript of their ticket {because}. Staff can find it in the ticket post. |
| `log.dm_failed.ticket_unknown` | `dm_failed` failed, state unknown | Quack couldn't confirm whether {member} got the transcript of their ticket. Staff can find it in the ticket post. |
| `log.appeal_submitted` | `appeal_submitted` ok | {member} appealed {case}. |
| `log.appeal_information_requested` | `appeal_information_requested` ok | {actor} asked {member} for more information about their appeal for {case}. |
| `log.appeal_information_provided` | `appeal_information_provided` ok | {member} added information to their appeal for {case}. |
| `log.appeal_accepted` | `appeal_accepted` ok | {actor} accepted {member}'s appeal for {case}. |
| `log.appeal_rejected` | `appeal_rejected` ok | {actor} rejected {member}'s appeal for {case}. |
| `log.appeal_closed` | `appeal_closed` ok | {actor} closed {member}'s appeal for {case}. |
| `log.appeal_closed.case_voided` | `appeal_closed` ok, by Quack | Quack closed {member}'s appeal for {case} because the case was voided. |
| `log.appeal_reopened` | `appeal_reopened` ok | {actor} reopened {member}'s appeal for {case} and asked for more information. |
| `log.rule_created` | `rule_created` ok | {actor} created the rule {rule}. |
| `log.rule_edited` | `rule_edited` ok | {actor} edited the rule {rule}. New cases use version {version}. |
| `log.rule_archived` | `rule_archived` ok | {actor} archived the rule {rule}. It can't be applied until someone restores it. |
| `log.rule_restored` | `rule_restored` ok | {actor} restored the rule {rule}. |
| `log.rule_imported` | `rule_imported` ok | {actor} imported the rule {rule}. |
| `log.settings_changed` | `settings_changed` ok | {actor} changed Quack's settings: {fields}. |
| `log.module_enabled` | `module_enabled` ok | {actor} turned on {module}. |
| `log.module_disabled` | `module_disabled` ok, by staff | {actor} turned off {module}. |
| `log.module_disabled.channel_deleted` | `module_disabled` ok, by Quack | Quack turned off {module} because its channel was deleted. |
| `log.module_settings_changed` | `module_settings_changed` ok | {actor} changed the {module} settings: {fields}. |
| `log.quack_joined.first` | `quack_joined` ok | Quack joined the server and created the starter rule {rule}. Review it on the dashboard. |
| `log.quack_joined.again` | `quack_joined` ok | Quack rejoined the server. |
| `log.quack_left` | `quack_left` ok (not posted) | Quack left the server. |
| `log.channel_setting_cleared` | `channel_setting_cleared` ok | Quack stopped using the {setting} because its channel was deleted. |
| `log.log_channel_failed.not_staff_only` | `log_channel_failed` failed (not posted) | Quack couldn't post in the log channel because it isn't staff-only anymore. The entry is still in the moderation log. |
| `log.log_channel_failed.discord_refused` | `log_channel_failed` failed (not posted) | Quack couldn't post in the log channel because Discord refused (error {code}). The entry is still in the moderation log. |
| `log.log_channel_failed.retries_exhausted` | `log_channel_failed` failed (not posted) | Quack couldn't post an entry in the log channel because Discord kept failing. The entry is still in the moderation log. |
| `log.ticket_opened` | `ticket_opened` ok | {member} opened a ticket: {thread}. |
| `log.ticket_closed` | `ticket_closed` ok | {actor} closed {member}'s ticket. The transcript is saved. |
| `log.ticket_closed.partial` | (appended) | Some messages may be missing from the transcript. |
| `log.honeypot_case_failed` | `honeypot_case_failed` failed | Quack couldn't add a honeypot case for {member}: {refusal}. |
| `log.legacy` | `legacy` (imported, not posted) | Imported from the previous version of Quack: {action}. |

`{fields}` uses these names: `setting.log_channel` "log channel",
`setting.appeal_channel` "appeal channel", `setting.evidence_channel`
"evidence channel", `setting.appeal_invite` "rejoin invite",
`setting.dm_intro` "DM introduction", `setting.dm_footer` "DM footer",
`setting.moderator_roles` "moderator roles", `setting.rules_manager_roles`
"rules manager roles", `setting.starter_notice` "starter rule notice",
`setting.ticket_entry_channel` "ticket entry channel",
`setting.ticket_queue_channel` "ticket staff channel",
`setting.transcript_retention` "transcript retention",
`setting.honeypot_channel` "honeypot channel", `setting.honeypot_rule`
"honeypot rule", `setting.honeypot_warning` "honeypot warning",
`setting.activity_routes` "server activity channels",
`setting.activity_content` "message text in the server activity log".
`{module}`: `module.tickets` "tickets", `module.activity_log` "the server
activity log", `module.honeypot` "the honeypot". `{refusal}` reuses the
§12.3 refusal texts without their first sentence, with the first letter
lowercased.

### 12.11 Setup

| Id | Text |
| --- | --- |
| `setup.home.heading` | ### Set up Quack |
| `setup.home.intro` | Choose what to set up. Changes save right away and also show on the dashboard. |
| `setup.home.log` | **Log channel:** {channel}, or "Not set" |
| `setup.home.appeals` | **Appeals:** {channel}, or "Not set"{", rejoin invite set" when set} |
| `setup.home.evidence` | **Evidence copies:** {channel}, or "Off" |
| `setup.home.tickets` | **Tickets:** On/Off{" · members open tickets in {entry} · new tickets go to {queue}" when set} |
| `setup.home.honeypot` | **Honeypot:** On/Off{" · {channel} · rule {rule}" when set} |
| `setup.home.activity_log` | **Server activity log:** On/Off{" · {channel}" or " · several channels"} |
| `setup.button.log` | Log channel |
| `setup.button.appeals` | Appeals |
| `setup.button.evidence` | Evidence copies |
| `setup.button.tickets` | Tickets |
| `setup.button.honeypot` | Honeypot |
| `setup.button.activity_log` | Server activity log |
| `setup.button.back` | Back |
| `setup.button.create` | Create #{name} |
| `setup.button.create_ticket_channels` | Create #support and #ticket-log |
| `setup.button.turn_on` | Turn on |
| `setup.button.turn_off` | Turn off |
| `setup.button.stop_log` | Stop posting |
| `setup.button.stop_evidence` | Stop copying files |
| `setup.button.set_invite` | Set rejoin invite |
| `setup.button.remove_invite` | Remove invite |
| `setup.button.edit_warning` | Edit warning |
| `setup.select.channel` | Choose a channel |
| `setup.select.entry` | Channel where members open tickets |
| `setup.select.queue` | Staff channel for new tickets |
| `setup.select.rule` | Rule the honeypot applies |
| `setup.log.explain` | Quack posts moderation log entries in this staff-only channel. |
| `setup.log.saved` | Quack will post moderation log entries in {channel}. |
| `setup.log.cleared` | Quack stopped posting moderation log entries. |
| `setup.appeals.explain` | New appeals are posted in this staff-only channel for moderators to review. Members whose appeal is accepted can get a rejoin invite. |
| `setup.appeals.saved` | New appeals will be posted in {channel}. |
| `setup.appeals.invite_saved` | Members whose appeal is accepted will get this invite: {invite} |
| `setup.appeals.invite_cleared` | The rejoin invite is removed. |
| `setup.evidence.explain` | Quack copies evidence files to this staff-only channel so they last after the original message is deleted. |
| `setup.evidence.saved` | Quack will copy evidence files to {channel}. |
| `setup.evidence.cleared` | Quack stopped copying evidence files. Existing copies stay where they are. |
| `setup.tickets.explain` | Members open private tickets from a button in the entry channel. Staff see new tickets in the staff channel. |
| `setup.tickets.entry_saved` | Members will open tickets from {channel}. |
| `setup.tickets.queue_saved` | New tickets will be posted in {channel}. |
| `setup.tickets.on` | Tickets are on. The Open a ticket button is in {channel}. |
| `setup.tickets.off` | Tickets are off. Open tickets stay open until someone closes them. |
| `setup.honeypot.explain` | A post in the honeypot channel adds a case under the chosen rule. Staff and Quack can post there safely. |
| `setup.honeypot.channel_saved` | The honeypot channel is {channel}. |
| `setup.honeypot.rule_saved` | Posts in the honeypot channel will add a case under {rule}. |
| `setup.honeypot.rule_created` | Quack created the rule {rule} for the honeypot. Change it on the dashboard if you want a different outcome. |
| `setup.honeypot.warning_saved` | The honeypot warning is updated. |
| `setup.honeypot.on` | The honeypot is on in {channel}. |
| `setup.honeypot.off` | The honeypot is off. Posts there no longer add cases. |
| `setup.activity_log.explain` | Quack posts message edits and deletions, members joining and leaving, bans, and server changes here. |
| `setup.activity_log.saved` | Server activity will be posted in {channel}. |
| `setup.activity_log.on` | The server activity log is on. |
| `setup.activity_log.off` | The server activity log is off. |
| `setup.created` | Quack created {channel} with staff-only permissions and saved it. |
| `setup.create_failed.permission` | Quack couldn't create #{name} because it doesn't have the Manage Channels permission. Choose an existing channel instead. |
| `setup.create_failed.discord` | Quack couldn't create #{name} because Discord refused (error {code}). Choose an existing channel instead. |
| `setup.channel_not_staff_only` | {channel} isn't staff-only, so Quack didn't save it:\n{problems, one per line}\nFix its permissions, then choose it again. |
| `setup.problem.not_text` | It isn't a text channel in this server. |
| `setup.problem.everyone_can_view` | @everyone can view it. Deny View Channel for @everyone. |
| `setup.problem.role_can_view` | The role {role} can view it but isn't a staff role. |
| `setup.problem.member_can_view` | {member} can view it but isn't staff. |
| `setup.quack_missing_channel_permissions` | Quack needs {permissions, joined with commas and "and"} in {channel}. Update the channel's permissions, then choose it again. |
| `setup.module.not_ready` | {Module} can't be turned on yet:\n{problems, one per line} |
| `module.problem.ticket_entry_missing` | Choose the channel where members open tickets. |
| `module.problem.ticket_queue_missing` | Choose the staff channel for new tickets. |
| `module.problem.ticket_same_channel` | The entry channel and the staff channel must be different. |
| `module.problem.honeypot_channel_missing` | Choose the honeypot channel. |
| `module.problem.honeypot_rule_missing` | Choose the rule the honeypot applies. |
| `module.problem.honeypot_rule_required_fields` | The rule {rule} has required context fields, which the honeypot can't fill in. Choose another rule or make those fields optional. |
| `module.problem.activity_no_route` | Choose a channel for server activity. |
| `module.problem.activity_needs_audit_log` | Quack needs the View Audit Log permission to show who banned or unbanned a member. |
| `module.problem.channel_permissions` | Quack needs {permissions} in {channel}. |
| `module.problem.channel_not_staff_only` | {channel} isn't staff-only. |
| `setup.invite.modal.title` | Rejoin invite |
| `setup.invite.modal.label` | Invite link |
| `setup.invite.modal.description` | A Discord invite like https://discord.gg/abc123. Members get it when their appeal is accepted. |
| `setup.invite.invalid` | That isn't a Discord invite link. Use a link like https://discord.gg/abc123. |
| `setup.warning.modal.title` | Honeypot warning |
| `setup.warning.modal.label` | Warning text |
| `setup.warning.modal.description` | Shown in the honeypot channel. Leave it empty to use Quack's default warning. |

Created channels (name, topic, first message):

| Id | Name | Topic | First message |
| --- | --- | --- | --- |
| `channel.log` | `moderation-log` | Quack's moderation log: cases, actions, appeals, and setting changes. | ### Moderation log\nQuack posts moderation log entries here. Only staff can see this channel. |
| `channel.appeals` | `appeals` | Appeals for moderators to review. | ### Appeals\nNew appeals appear here. Accept or reject them here or on the dashboard. |
| `channel.evidence` | `evidence` | Copies of evidence files for Quack cases. | ### Evidence\nQuack copies evidence files here so they last after the original message is deleted. Please don't delete messages here. |
| `channel.ticket_entry` | `support` | Need help from the staff? Open a ticket below. | (the ticket panel, §12.13) |
| `channel.ticket_queue` | `ticket-log` | New tickets and their transcripts. | ### Tickets\nNew tickets appear here. Open a ticket's thread to help. |
| `channel.honeypot` | `honeypot` | Don't post here. Read the warning below. | (the honeypot warning, §12.13) |
| `channel.activity_log` | `server-log` | Server activity: edits, deletions, joins, leaves, bans, and server changes. | ### Server activity log\nQuack posts server activity here. Only staff can see this channel. |

### 12.12 Help, rules, lists

| Id | Text |
| --- | --- |
| `help.overview` | ### Quack\nQuack keeps track of moderation cases. Rules managers set up rules on the dashboard, moderators apply them, and Quack picks the outcome and carries it out.\n\n**Moderators:** /case add, /case view, /case list, /case history, /case void, /appeals\n**Rules:** /rules (rules are edited on the dashboard)\n**Setup:** /setup (needs Manage Server)\n\nUse /help with a topic for more. |
| `help.cases` | ### Cases\nUse /case add, or right-click a message or member and choose **Apps → Add case**. Choose the rule, and Quack counts the member's earlier cases under it, picks the level, and carries out its action. Set private to True to keep the receipt to yourself.\n\nWhen you add a case from a message, Quack saves that message as evidence. Add more with **Add evidence** on the case. The member can see the case context, but not who added the case or who wrote the evidence.\n\nMade a mistake? Use /case void or **Void case**. The case stays in the history but stops counting, and Quack removes its timeout or ban. |
| `help.rules` | ### Rules\nA rule is one kind of problem, like spam. Each rule has levels that start at a case count: for example a warning for the first case, a 1-hour timeout from the third, and a ban from the fifth. Counts are per member and per rule, and voided cases don't count. A rule can count only recent cases, like the last 30 days.\n\nRules are edited on the dashboard, and editing a rule never changes old cases. Use /rules to see this server's rules. |
| `help.appeals` | ### Appeals\nA member can appeal each case once, from Quack's DM or from the dashboard, even after a ban. Choose the appeal channel with /setup. Moderators accept or reject appeals there, with /appeals, or on the dashboard, and the member gets a DM with the decision. Accepting an appeal voids the case and removes its timeout or ban. |
| `help.setup` | ### Setup\nRun /setup to choose the log channel, the appeal channel, and the evidence channel, and to turn on tickets, the honeypot, and the server activity log. Quack can create each channel for you with staff-only permissions. Everything in /setup is also in the dashboard settings. |
| `help.dashboard_link` | Open the dashboard |
| `rules.heading` | ### Rules |
| `rules.row` | **{name}** · {levels, `level.summary.item` joined with " · "} · [Open]({url}) |
| `rules.footer` | -# Rules are edited on the dashboard. one: 1 archived rule is there too. / other: {count} archived rules are there too. |
| `rules.empty` | This server has no active rules. Create one on the dashboard. |
| `case.list.heading` | ### Recent cases |
| `case.list.heading_member` | ### Recent cases about {member} |
| `case.list.heading_rule` | ### Recent cases under {rule} |
| `case.list.heading_both` | ### Recent cases about {member} under {rule} |
| `case.list.row` | **#{number}** · {member} · {rule} · {date}{" · Voided"}{" · Imported"} |
| `case.list.empty` | No cases match. |
| `case.history.heading` | ### Case history for {member} |
| `case.history.totals` | {total} cases · {valid} valid · {voided} voided |
| `case.history.empty` | {member} has no cases in this server. |

### 12.13 Module messages

Tickets:

| Id | Text |
| --- | --- |
| `ticket.panel.heading` | ### Need help from the staff? |
| `ticket.panel.text` | Open a ticket to talk privately with the moderators. |
| `ticket.panel.off` | Tickets are turned off right now. |
| `ticket.panel.button` | Open a ticket |
| `ticket.panel.moved` | Tickets moved to {channel}. Open a ticket there. |
| `ticket.open.opening` | Quack is opening your ticket. It will appear in {channel} in a moment. |
| `ticket.open.ready` | Your ticket is ready: {thread}. Tell the staff what you need there. |
| `ticket.open.exists` | You already have an open ticket: {thread}. |
| `ticket.open.closing` | Your last ticket is still closing. Try again in a minute. |
| `ticket.open.disabled` | Tickets are turned off in this server. |
| `ticket.open.failed` | Quack couldn't open a ticket because Discord refused (error {code}). Ask a server administrator to check Quack's permissions in {channel}. |
| `ticket.welcome` | {member}, your ticket is open. Tell us what you need help with, and a moderator will reply here. |
| `ticket.button.close` | Close ticket |
| `ticket.button.open_thread` | Open thread |
| `ticket.button.fix_access` | Fix access |
| `ticket.post.heading` | ### Ticket from {member} |
| `ticket.post.open` | Open · {thread} · opened {time} |
| `ticket.post.opening` | Opening… |
| `ticket.post.closing` | Closing. Quack is saving the transcript. |
| `ticket.post.closed` | Closed by {actor} {time}. The transcript is attached. |
| `ticket.post.partial` | The transcript may be missing some messages, because Quack couldn't read the thread's full history. |
| `ticket.close.closing` | Quack is closing this ticket and saving the transcript. |
| `ticket.close.not_open` | This ticket is already closed or closing. |
| `ticket.close.not_allowed` | Only the member who opened this ticket and moderators can close it. |
| `ticket.fix.done` | Ticket access is fixed: only {member} and staff can see the thread. |
| `ticket.fix.failed` | Quack couldn't fix the ticket's access because Discord refused (error {code}). |
| `dm.ticket_closed.heading` | ### Your ticket in {server} is closed |
| `dm.ticket_closed.text` | Here's a copy of the conversation for your records. |
| `ticket.thread_name` | ticket-{short id} |
| `ticket.transcript.header` | Ticket transcript · {server} · opened {time UTC} by {member name} ({member id}) · closed {time UTC} |
| `ticket.transcript.line` | [{time UTC}] {author name} ({author id}): {text} |
| `ticket.transcript.file_line` | `  file: {name} ({size} bytes)` |
| `ticket.transcript.partial` | Some messages may be missing: Quack couldn't read the thread's full history. |

Server activity log (posted in the routed channel):

| Id | Text |
| --- | --- |
| `activity.message_edited` | A message from {author} was edited in {channel}. |
| `activity.before` | **Before:**\n> {text} |
| `activity.before_unknown` | **Before:** Quack hadn't seen this message, so the earlier text isn't known. |
| `activity.after` | **After:**\n> {text} |
| `activity.message_deleted` | A message from {author} was deleted in {channel}. |
| `activity.message_deleted_unknown` | A message was deleted in {channel}. Quack hadn't seen it, so its author and text aren't known. |
| `activity.messages_deleted_unknown` | {count} messages were deleted in {channel}. Quack hadn't seen them, so their authors and text aren't known. |
| `activity.messages_bulk_deleted` | {count} messages were deleted at once in {channel}. |
| `activity.bulk_known` | one: Quack had seen 1 of them: / other: Quack had seen {known} of them: |
| `activity.bulk_row` | {author}: {text, cut to 200 characters} |
| `activity.files` | Files: {names, comma-separated} |
| `activity.embeds` | Link previews: {types} |
| `activity.text_hidden` | Message text isn't shown, because this server's activity log hides it. |
| `activity.message_ids` | -# Message IDs: {ids, comma-separated, at most 50} |
| `activity.subtext` | -# Message {message_id} · {time} |
| `activity.member_joined` | {member} joined the server. Their account was created {time}. |
| `activity.member_left` | **{username}** ({member_id}) left the server. |
| `activity.member_banned` | {member} was banned by {actor}. |
| `activity.member_unbanned` | {member} was unbanned by {actor}. |
| `activity.reason` | **Reason:**\n> {reason} |
| `activity.server_changed` | The server settings changed. |
| `activity.channel_created` | The channel {channel name} was created. |
| `activity.channel_changed` | The settings of {channel name} changed. |
| `activity.channel_deleted` | The channel #{name} was deleted. |

Honeypot:

| Id | Text |
| --- | --- |
| `honeypot.warning.heading` | ### Don't post here |
| `honeypot.warning.one_outcome` | Posting here adds a case under {rule}, and Quack will {outcome}. |
| `honeypot.warning.many_outcomes` | Posting here adds a case under {rule}. Depending on earlier cases, Quack will {outcomes joined with ", " and " or "}. |
| `honeypot.warning.warning_only` | Posting here adds a case under {rule}. |
| `honeypot.warning.tail` | This channel catches spam accounts. Staff can post here safely. |
| `honeypot.warning.count` | -# one: 1 honeypot case so far. / other: {count} honeypot cases so far. |
| `honeypot.warning.off` | -# The honeypot is turned off right now. |
| `honeypot.outcome.ban` | ban you |
| `honeypot.outcome.kick` | remove you from the server |
| `honeypot.outcome.timeout` | time you out for {duration} |
| `honeypot.outcome.warning` | give you a warning |

A custom warning replaces the heading and the generated sentences; the
count line is always appended.

### 12.14 API messages

The `message` of each API error (§14.6). Where an `error.*` or `refusal.*`
id fits, the API uses it; the API-only texts are:

| Id | Text |
| --- | --- |
| `api.sign_in_required` | Sign in with Discord to continue. |
| `api.session_expired` | Your session has ended. Sign in again. |
| `api.origin_rejected` | This request didn't come from the Quack dashboard. |
| `api.csrf_rejected` | Quack couldn't verify this request came from your session. Reload the page and try again. |
| `api.not_found` | Quack couldn't find that. |
| `api.server_not_found` | Quack isn't in that server, or you're not a member of it. |
| `api.file_unavailable` | That file isn't available anymore. |
| `api.validation_failed` | Some fields need fixing. |
| `api.invalid_json` | The request body isn't valid JSON. |
| `api.body_too_large` | The request is too large. The limit is 256 KB. |
| `api.unsupported_media_type` | Send the request body as JSON. |
| `api.idempotency_key_required` | This request needs an Idempotency-Key header. |
| `api.idempotency_key_reused` | This Idempotency-Key was already used for a different request. Use a new key. |
| `api.rate_limited` | You're doing that too often. Try again in {seconds} seconds. |
| `api.rule_changed` | Someone else saved this rule. Reload it to see the latest version before you save. |
| `api.rule_name_taken` | Another active rule is already called {name}. Choose a different name. |
| `api.rule_used_by_honeypot` | The honeypot uses this rule, so it can't be archived. Choose another rule for the honeypot first. |
| `api.required_field_on_honeypot_rule` | The honeypot uses this rule, so its fields can't be required. Make the fields optional or choose another rule for the honeypot. |
| `api.too_many_rules` | A server can have at most 100 active rules. Archive one first. |
| `api.settings_changed` | Someone else changed the staff roles. Reload the settings and try again. |
| `api.appeal_exists` | You already appealed this case. |
| `api.appeal_not_allowed` | This case can't be appealed. |
| `api.ticket_not_open` | This ticket is already closed or closing. |
| `api.module_not_ready` | {Module} can't be turned on yet. |
| `api.discord_unavailable` | Quack couldn't reach Discord, so nothing changed. Try again in a moment. |
| `api.unavailable` | Quack is having trouble right now, so nothing changed. Try again in a moment. |
| `api.internal_error` | Something went wrong inside Quack. If you changed something, check whether it was saved before you try again. |
| `api.sign_in_failed` | Discord sign-in didn't finish. Start again. |
| `api.sign_in_denied` | You cancelled the Discord sign-in. |

---

## 13. Settings and optional modules

### 13.1 Server settings and shared pieces

#### 13.1.1 Core settings

One `guild_settings` row per server, created by bootstrap (§17.5).

| Setting (API field) | Default | Validation | Who changes it |
| --- | --- | --- | --- |
| `moderator_role_ids` | `[]` | 0–25 distinct role IDs; each must be a current role of the server (except IDs already saved, which are kept and ignored while deleted); not @everyone. | `manage_staff_roles` (others may resend the same set) |
| `rules_manager_role_ids` | `[]` | Same rules. | `manage_settings` |
| `log_channel_id` | NULL | Staff-only check (§13.1.3) with Quack needing View Channel and Send Messages. | `manage_settings` |
| `appeal_channel_id` | NULL | Staff-only; View Channel, Send Messages. | `manage_settings` |
| `evidence_channel_id` | NULL | Staff-only; View Channel, Send Messages, Attach Files. | `manage_settings` |
| `appeal_invite_url` | NULL | `https://discord.gg/<code>` or `https://discord.com/invite/<code>`, code 2–32 of `[A-Za-z0-9-]`; stored as `https://discord.gg/<code>`. | `manage_settings` |
| `dm_intro` | NULL | 0–1,000 characters; empty stores NULL. | `manage_settings` |
| `dm_footer` | NULL | 0–500 characters; empty stores NULL. | `manage_settings` |
| `starter_rule_id` | set at bootstrap | read-only | — |
| `starter_notice_acknowledged_at` | NULL | Set by the acknowledge endpoint; first timestamp kept. | `manage_settings` |

Several settings may name the same channel.

#### 13.1.2 Updating settings

`PATCH /api/guilds/{guild_id}/settings` writes only the fields it contains,
in one transaction:

1. Validate every sent field (Discord checks happen before the transaction;
   failures are reported per field).
2. `SELECT … FROM guild_settings WHERE guild_id = $1 FOR UPDATE`.
3. When the request contains `moderator_role_ids` or
   `rules_manager_role_ids`, it must also contain `expected_version`; a
   mismatch with `guild_settings.version` → `409 settings_changed` and
   nothing is saved. This stops a stale page from putting back roles an
   administrator just removed.
4. Channel checks use the moderator roles being saved when the request
   contains them, else the stored ones.
5. Update the sent columns, `version = version + 1`, `updated_at`,
   `updated_by`; log `settings_changed` with the field names; `NOTIFY
   quack_settings`.

Module enable and disable are separate endpoints (§13.1.7).

#### 13.1.3 Staff-only channel check

`quack_core::permissions::check_staff_channel(channel, guild, settings, quack_needs) -> Vec<ChannelProblem>`,
pure over cached channel and role data (REST fallback in the adapter). A
staff destination passes when:

1. It is a text channel (type 0) in this server (`not_text`).
2. The @everyone overwrite denies View Channel (`everyone_can_view`).
3. Every role overwrite that allows View Channel is for a role that is staff
   for Discord access: it has Administrator or Moderate Members, or it is a
   configured moderator role (`role_can_view`).
4. Every member overwrite that allows View Channel is for Quack, the owner,
   or a member who is staff for Discord access (`member_can_view`).
5. Quack's computed permissions in the channel include every permission in
   `quack_needs` (`quack_missing`).

The check runs when a channel is saved and again before every post to a
staff destination (log channel, appeal posts, evidence copies, ticket staff
posts, server activity posts). Changing staff roles later never edits
existing channel overwrites.

#### 13.1.4 Creating channels

`/setup` "Create" buttons call `quack_engine::setup::create_channel(kind)`:
the name and topic come from §12.11, the first message is posted, and the
channel is saved in the same step. Overwrites:

| Kind | @everyone | Quack | Staff roles* |
| --- | --- | --- | --- |
| Staff destination (log, appeals, evidence, ticket queue, server activity) | deny View Channel | allow View Channel, Send Messages, Read Message History, Attach Files, Embed Links | allow View Channel, Send Messages, Read Message History |
| Ticket entry | allow View Channel, Read Message History, Send Messages in Threads; deny Send Messages, Create Public Threads, Create Private Threads | allow View Channel, Send Messages, Read Message History, Create Private Threads, Manage Threads, Send Messages in Threads | — |
| Honeypot | allow View Channel, Send Messages, Read Message History | allow View Channel, Send Messages, Read Message History, Manage Messages | — |

\* Configured moderator roles that still exist; when none exist, every role
with Administrator or Moderate Members.

Creation needs Manage Channels: without it, `setup.create_failed.permission`.

#### 13.1.5 Channel reconciliation

A channel deleted while Quack is running (`CHANNEL_DELETE`) or while it was
away (missing from the `GUILD_CREATE` channel list) clears every setting
that points at it, in one transaction per server, guarded on the old value:

| Setting | Effect |
| --- | --- |
| `log_channel_id`, `appeal_channel_id`, `evidence_channel_id` | Set NULL; log `channel_setting_cleared`. |
| Ticket entry or queue channel | Set NULL; tickets disabled (`channel_deleted`); log `module_disabled`. |
| Honeypot channel | Set NULL; honeypot disabled (`channel_deleted`); warning row `gone`; log `module_disabled`. |
| Server activity route | Route removed; when no route remains, the module is disabled (`channel_deleted`); log `module_settings_changed` or `module_disabled`. |
| A ticket's thread (`THREAD_DELETE`, or on `GUILD_CREATE` an `open` ticket whose thread is not in the event's active-thread list and `GET /channels/{thread}` answers 404) | The ticket moves to `closing`, closed by Quack (§13.2.4). |

Tracked messages in a deleted channel are handled by their sync (§10.3).

#### 13.1.6 The `Module` contract

Each optional module is one crate implementing one trait, defined in
`quack-engine`:

```rust
pub trait Module: Send + Sync + 'static {
    /// Which module this is.
    fn name(&self) -> ModuleName;
    /// Whether this module consumes a gateway event kind.
    fn wants(&self, kind: EventKind) -> bool;
    /// Handle one gateway event (already filtered by `wants`).
    fn on_event<'a>(&'a self, ctx: &'a Ctx, event: &'a GatewayEvent) -> BoxFuture<'a, ()>;
    /// Custom-ID codes this module owns (§11.5).
    fn component_codes(&self) -> &'static [&'static str];
    /// Handle a button, select, or modal with one of those codes.
    fn on_component<'a>(&'a self, ctx: &'a Ctx, call: ComponentCall) -> BoxFuture<'a, Msg>;
    /// The `/setup` area this module owns.
    fn setup_area(&self) -> SetupArea;
    /// Render its `/setup` panel.
    fn setup_panel<'a>(&'a self, ctx: &'a Ctx, guild: GuildId) -> BoxFuture<'a, Msg>;
    /// Apply one `/setup` operation and return the re-rendered panel.
    fn on_setup<'a>(&'a self, ctx: &'a Ctx, who: &'a Principal, op: SetupOp) -> BoxFuture<'a, Msg>;
    /// Job kinds this module runs (§16.5).
    fn job_kinds(&self) -> &'static [JobKind];
    /// Run one claimed job of those kinds.
    fn run_job<'a>(&'a self, ctx: &'a Ctx, job: &'a ClaimedJob) -> BoxFuture<'a, JobResult>;
    /// Why the module can't be turned on right now (empty = it can).
    fn enable_problems<'a>(&'a self, ctx: &'a Ctx, guild: GuildId) -> BoxFuture<'a, Vec<ModuleProblem>>;
    /// Called in the enable/disable transaction's aftermath (e.g. bump a panel).
    fn on_enabled_changed<'a>(&'a self, ctx: &'a Ctx, guild: GuildId, enabled: bool) -> BoxFuture<'a, ()>;
}
```

`Ctx` gives access to the database pool, the `Discord` port, the caches,
the engine (for the honeypot's system case entry), telemetry, and
`quack-text`. HTTP routes for a module are thin handlers in `quack-api`
that call the module crate's public service functions and DTOs (§14.3); a
module never depends on `quack-api`.

#### 13.1.7 Enable and disable

`POST /api/guilds/{guild_id}/modules/{module}/enable` and `…/disable`
(`manage_settings` + 2FA), also used by `/setup`:

- Enable: `enable_problems` empty → one transaction sets `enabled = true`,
  clears `disabled_reason`, bumps `version`, logs `module_enabled`, `NOTIFY
  quack_settings`; then `on_enabled_changed`. Otherwise `409
  module_not_ready` with `problems` (§12.11 `module.problem.*`).
- Disable: sets `enabled = false`, `disabled_reason = NULL`, logs
  `module_disabled`. Saved settings are kept.
- Enabling an enabled module or disabling a disabled one is a no-op that
  returns the current state and writes nothing.

### 13.2 Tickets

Private support threads between a member and staff. Tickets are separate
from cases and appeals.

#### 13.2.1 Settings (`ticket_settings`)

| Column | Default | Validation |
| --- | --- | --- |
| `enabled` | false | via enable/disable only |
| `disabled_reason` | NULL | `channel_deleted` |
| `entry_channel_id` | NULL | text channel in the server |
| `queue_channel_id` | NULL | staff-only; Quack needs View Channel, Send Messages, Attach Files |
| `transcript_retention_days` | 90 | 1–365 |

Enable problems: entry missing, queue missing, entry equals queue, Quack
lacking View Channel, Send Messages, Read Message History, Create Private
Threads, Send Messages in Threads, or Manage Threads in the entry channel,
queue not staff-only or lacking permissions.

Changing the entry channel moves the panel (§10.3: the sync posts the
panel in the new channel, then edits the old message to
`ticket.panel.moved`). `PUT …/modules/tickets/settings` validates every
field even while disabled.

#### 13.2.2 States

`ticket_state`: `opening`, `open`, `closing`, `closed`. A partial unique
index allows one ticket per member in `opening`, `open`, or `closing`, so a
member has at most one live ticket and the "slot" is freed exactly when the
ticket reaches `closed`. `closing` always ends (§13.2.4).

| From | Event | To |
| --- | --- | --- |
| (none) | Member clicks **Open a ticket** | `opening` (+ `open_ticket` job) |
| `opening` | Thread created, member added, welcome posted | `open` |
| `opening` | Thread creation refused (403/404) | `closed` (`close_reason = open_failed`) |
| `open` | Member or moderator closes (button or dashboard) | `closing` (+ `close_ticket` job) |
| `open` | Thread deleted (`THREAD_DELETE`) | `closing` (closed by Quack) |
| `closing` | Close job finishes, or its budget runs out | `closed` |

#### 13.2.3 Opening

1. Component `to`: defer ephemerally. Tickets disabled → `ticket.open.disabled`.
2. Transaction: insert `tickets` (`opening`, `short_id` = first 10
   characters of the ticket's u22) and the `open_ticket` job. A unique
   violation → load the member's live ticket and answer
   `ticket.open.exists` (`open`), `ticket.open.closing` (`closing`), or
   `ticket.open.opening` (`opening`).
3. The bot runs the `open_ticket` handler inline once; on `Done` it answers
   `ticket.open.ready`, otherwise `ticket.open.opening` (the job continues).

`open_ticket` job (idempotent; budget 4, backoff 2 s, 10 s, 30 s):

1. State not `opening` → `Done`.
2. No `thread_id`: look for an active thread named `ticket-<short_id>` under
   the entry channel (`GET /guilds/{g}/threads/active`) and adopt it; else
   create a private thread (type 12, `invitable: false`,
   `auto_archive_duration: 1440`). Save `thread_id` at once. Refused →
   `closed` with `open_failed`.
3. Add the member to the thread (`PUT /channels/{t}/thread-members/{m}`).
4. Post `ticket.welcome` with a **Close ticket** button (pings the member;
   nonce = ticket u22 + `w`).
5. Create the `ticket_post` tracked message in the queue channel.
6. Transaction: `open`, `opened_at`; log `ticket_opened`; register the
   thread in the in-memory thread map.

#### 13.2.4 Journal and closing

- **Journal.** `MESSAGE_CREATE` in a thread of an `open` ticket inserts
  `ticket_messages` (message ID, author, author name, text, file names and
  sizes, sent time; `ON CONFLICT DO NOTHING`). Edits and deletes never
  change journal rows, so the transcript keeps original wording. An event
  dropped from a full gateway queue sets `tickets.journal_gaps = true`.
- **Closing.** Component `tc` or `POST …/tickets/{id}/close`: the ticket's
  member or `moderate` + 2FA; ticket `open` (else `ticket.close.not_open`).
  Transaction: `closing`, `closed_by`, `close_requested_at`, `close_ticket`
  job. Answer `ticket.close.closing`.

`close_ticket` job (each step records its completion column, so a retry
resumes; budget 6, backoff 5 s, 30 s, 2 min, 10 min, 1 h):

1. Lock the thread (`PATCH /channels/{t}` `locked: true`); 404 → skip.
   `locked_at`.
2. Transcript (unless `transcript_saved_at`): read the thread's history
   (100 per page, oldest last, at most 10,000 messages) and merge with the
   journal by message ID: journal text wins, history adds messages the
   journal lacks. History unavailable (404, 403) → journal only.
   `transcript_complete = history read fully AND NOT journal_gaps`. Render
   with `ticket.transcript.*` (UTC times), cut at 8 MiB with
   `ticket.transcript.partial`. Insert `ticket_transcripts` (content,
   complete, `expires_at = now + retention`). `transcript_saved_at`.
3. Bump the `ticket_post` (its sync attaches `ticket-<short_id>.txt`).
4. Insert the `ticket_closed` DM with its job (the DM attaches the
   transcript file; `ON CONFLICT DO NOTHING` on `ticket_id`).
5. Delete the thread (`DELETE /channels/{t}`); 404 → done; 403 →
   `thread_delete_failed = true` and continue. `thread_deleted_at`.
6. Transaction: `closed`, `closed_at`; set journal rows' `expires_at`; log
   `ticket_closed`; drop the thread from the thread map.

Budget exhausted → step 6 runs anyway with whatever is saved (a journal-only
transcript if step 2 never finished, `transcript_complete = false`), so
no ticket stays `closing`.

- **Fix access** (component `tf`, `moderate` + 2FA, ticket `open`): re-add the
  member; list thread members and remove everyone except the member, Quack,
  the owner, and staff for Discord access. Answer `ticket.fix.done` or
  `ticket.fix.failed`. Not logged.

#### 13.2.5 Storage, jobs, retention

Tables `ticket_settings`, `tickets`, `ticket_messages`,
`ticket_transcripts` (§15). Jobs `open_ticket`, `close_ticket`. The tracked
messages `ticket_panel` and `ticket_post`. Housekeeping deletes
`ticket_transcripts` and `ticket_messages` rows past `expires_at`; tickets
themselves are kept. Discord copies (staff post attachment, the member's DM)
are not deleted.

Staff with `moderate` see tickets on the dashboard; Discord access to a
private thread still needs Manage Threads or being added (known gap
§20.4).

### 13.3 Server activity log

Posts Discord events to staff-only channels. It is not a searchable archive
and writes nothing to the moderation log except its own settings changes.

#### 13.3.1 Settings

`activity_log_settings`: `enabled` (false), `disabled_reason`,
`include_message_content` (true), `include_file_names` (true),
`include_embed_types` (true). `activity_log_routes`: `(guild_id, event,
channel_id)`, one channel per event. `activity_event`:

| Event | Source | Text |
| --- | --- | --- |
| `message_edited` | `MESSAGE_UPDATE` with an edited timestamp and changed text or files | `activity.message_edited` + before/after |
| `message_deleted` | `MESSAGE_DELETE` | `activity.message_deleted` or the unknown variants |
| `messages_bulk_deleted` | `MESSAGE_DELETE_BULK` | `activity.messages_bulk_deleted` |
| `member_joined` | `GUILD_MEMBER_ADD` | `activity.member_joined` |
| `member_left` | `GUILD_MEMBER_REMOVE` | `activity.member_left` |
| `member_banned` | audit log entry 22, executor not Quack | `activity.member_banned` |
| `member_unbanned` | audit log entry 23, executor not Quack | `activity.member_unbanned` |
| `server_changed` | `GUILD_UPDATE` | `activity.server_changed` |
| `channel_changed` | `CHANNEL_CREATE/UPDATE/DELETE` | `activity.channel_*` |

`/setup` routes all nine events to one channel. The dashboard can route
each event separately. Enable problems: no route; a route channel not
staff-only or Quack lacking View Channel and Send Messages there; Quack
lacking View Audit Log in the server.

#### 13.3.2 Behaviour

- **One message cache** (§11.10). Twilight's message cache is off, so the
  module's cache is the only one. Messages are cached on `MESSAGE_CREATE`
  in servers where the module is enabled, for text channels and threads.
- **Consistent filtering.** Messages written by Quack or by webhooks are
  ignored for create, edit, and delete. Messages in a channel that is itself
  an activity route destination are ignored. Other bots are logged.
- **Edits.** Missing fields in a partial update count as unknown, not
  empty. When the text and file list are unchanged (an embed-only update),
  nothing is posted. The cache keeps the newest version.
- **Deletes.** A cached message posts at once with its author and text.
  Uncached deletes are batched per channel for 60 seconds or until 50 IDs:
  one ID → `activity.message_deleted_unknown`; more →
  `activity.messages_deleted_unknown` with `activity.message_ids`.
- **Privacy flags.** `include_message_content = false` replaces text with
  `activity.text_hidden`; the other flags drop file names and embed types.
- **Delivery.** In memory, not durable: a bounded queue of 10,000 posts and
  2 sender tasks. Each post re-checks the destination (§13.1.3, result
  cached per channel for 60 seconds) and creates the message with a nonce
  (event hash, 25 characters). `RetryAfter` or `Unknown` → up to 2 more
  tries after 1 s and 2 s with the same nonce; definite refusals are not
  retried. Counted as
  `quack.activity_log.events{event,outcome}` with outcomes `posted`,
  `filtered`, `dropped` (queue full), `failed`.
- **Graceful shutdown** flushes pending delete batches and drains the queue
  within the shutdown deadline (§17.5).

### 13.4 Honeypot

A trap channel. A post there from a member who is not staff adds a case
under the honeypot's rule through the normal case path.

#### 13.4.1 Settings (`honeypot_settings`)

| Column | Default | Validation |
| --- | --- | --- |
| `enabled` | false | via enable/disable only |
| `disabled_reason` | NULL | `channel_deleted` |
| `channel_id` | NULL | text channel; Quack needs View Channel, Send Messages, Read Message History, Manage Messages |
| `rule_id` | NULL | active rule of this server with no required fields (§3.8) |
| `warning_text` | NULL | 0–1,500 characters; NULL or empty uses the generated warning |

Enable problems: channel missing, rule missing, rule with required fields,
Quack lacking channel permissions. Turning it on from `/setup` with no rule
chosen uses or creates the "Honeypot" rule (§3.8).

#### 13.4.2 From post to case

1. `MESSAGE_CREATE` in `channel_id` of a server with the honeypot enabled.
   Ignored when the author is a bot (any bot, including Quack), a webhook,
   or staff for Discord access (owner, Administrator, Moderate Members, a
   configured moderator role; from the message's member payload and the
   config cache). This matches the core rule that cases are never about bots
   (§2.3 check 5).
2. Insert `honeypot_hits` (message ID unique, member, `pending`) and enqueue
   the `honeypot_case` job keyed `honeypot_case:<guild>:<member>`
   (coalesced). A full gateway queue drops the event and counts
   `quack.honeypot.hits{outcome="dropped"}`.
3. `honeypot_case` job (budget 5, backoff 5 s, 30 s, 2 min, 10 min):
   1. Load the member's `pending` hits.
   2. A hit of this member in `case_created` within the last 30 seconds →
      attach the pending hits to that case (a burst is one case).
   3. Otherwise apply the rule (§4.2) with source `honeypot`, actor Quack,
      operation key `honeypot:<guild>:<first pending message ID>`, and the
      first hit's message as evidence.
   4. Created or replayed → hits `case_created` with `case_id`; enqueue
      `honeypot_cleanup` per hit; bump the warning.
   5. A refusal (§2.3) or a missing rule → hits `failed` with the refusal;
      log `honeypot_case_failed` (for honeypot cases the engine does not
      also write `case_create_refused`). Their messages are kept.
   6. `DiscordUnavailable` → retry.
4. `honeypot_cleanup` job (keyed `honeypot_cleanup:<message_id>`; budget 10,
   backoff 1, 2, 4, 8, 16, 32, 64, 64, 64 s): delete the message; 404 counts
   as done; record `cleaned_up_at`.

#### 13.4.3 The warning

The `honeypot_warning` tracked message in the honeypot channel renders
`honeypot.warning.*` from the rule's levels (or the custom text) plus the
count of distinct cases created from hits. It is bumped by a new honeypot
case, settings changes, enable/disable, and its own deletion (re-posted).

---

## 14. HTTP API

The API serves the dashboard only. It lives under `/api` on the same origin
as the dashboard files, served by `quack serve`. axum 0.8, routes in
`quack-api`, documented with utoipa.

### 14.1 Conventions

- JSON in and out (`Content-Type: application/json`); other request media
  types → `415 unsupported_media_type`.
- Discord IDs and UUIDs are **strings** in JSON. Timestamps are RFC 3339 UTC
  with milliseconds. Enums are the snake_case values defined in this
  document.
- `{guild_id}` is the Discord server ID. `{case_ref}` is a case number, a
  case UUID, or a legacy ULID (mapped, §19.3). Other path IDs are UUIDs.
- Lists use keyset pagination: `limit` (1–100, default 50) and `cursor`
  (opaque base64url); responses carry `next_cursor` (null on the last
  page). No offsets.
- Every write (POST, PUT, PATCH) under `/api/guilds` and `/api/me` requires
  `Idempotency-Key` (8–200 printable ASCII characters) and is idempotent
  through `operation_keys` (§4.3), except the auth routes.
- Every response carries `traceparent` (W3C) so a report can be matched to
  its trace; errors also put it in `error.trace_id`.

### 14.2 Routes

Capability "staff:X" means a session, membership in the server (checked
live), capability X, and the 2FA rule. "member" means a signed-in session
acting as the member. Rate classes are in §14.7. Pages are dashboard
routes (§14.11).

| # | Method | Path | Auth | Request | Response | Idem. | Rate | Page |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | GET | `/api/auth/login` | public | query `redirect_to` | 302 to Discord | — | sign_in | `/login` |
| 2 | GET | `/api/auth/callback` | public | query `code`, `state` or `error` | 302 to `redirect_to` or `/login?error=…` | — | sign_in | (Discord redirect) |
| 3 | GET | `/api/auth/me` | session | — | `Me` | — | read | every signed-in page |
| 4 | POST | `/api/auth/logout` | session + CSRF | — | 204 | naturally | write | sidebar |
| 5 | GET | `/api/guilds` | session | — | `{guilds: GuildSummary[]}` | — | read | `/guilds` |
| 6 | GET | `/api/guilds/{guild_id}/me` | session + member of server | — | `GuildMe` | — | read | staff layout |
| 7 | GET | `/api/guilds/{guild_id}/health` | staff:manage_settings | — | `Health` | — | read | overview, settings |
| 8 | GET | `/api/guilds/{guild_id}/statistics` | staff:moderate | query `from`, `to` | `Statistics` | — | read | overview |
| 9 | GET | `/api/guilds/{guild_id}/directory/members` | staff:moderate | query `query` (1–100 chars) and `limit` (1–25, default 10), or `ids` (comma-separated, ≤ 100) | `{members: DirectoryMember[]}` | — | read | cases, case dialog, member chips |
| 10 | GET | `/api/guilds/{guild_id}/directory/channels` | staff:manage_settings | — | `{channels: DirectoryChannel[]}` | — | read | settings, modules |
| 11 | GET | `/api/guilds/{guild_id}/directory/roles` | staff:manage_settings | — | `{roles: DirectoryRole[]}` | — | read | settings |
| 12 | GET | `/api/guilds/{guild_id}/rules` | staff:moderate or manage_rules | query `archived` (bool, default false) | `{rules: RuleSummary[]}` | — | read | rules, cases, overview |
| 13 | POST | `/api/guilds/{guild_id}/rules` | staff:manage_rules | `RuleDefinition` | 201 `Rule` | yes | write | rules/new |
| 14 | GET | `/api/guilds/{guild_id}/rules/{rule_id}` | staff:moderate or manage_rules | — | `Rule` | — | read | rules/$ruleId, case dialog |
| 15 | PUT | `/api/guilds/{guild_id}/rules/{rule_id}` | staff:manage_rules | `{expected_version, definition: RuleDefinition}` | `Rule` | yes | write | rules/$ruleId |
| 16 | POST | `/api/guilds/{guild_id}/rules/{rule_id}/archive` | staff:manage_rules | — | `Rule` | yes | write | rules |
| 17 | POST | `/api/guilds/{guild_id}/rules/{rule_id}/restore` | staff:manage_rules | — | `Rule` | yes | write | rules |
| 18 | GET | `/api/guilds/{guild_id}/rules/{rule_id}/export` | staff:manage_rules | — | `RuleFile` | — | read | rules/$ruleId |
| 19 | POST | `/api/guilds/{guild_id}/rules/import` | staff:manage_rules | `{confirm: true, file: RuleFile, name?: string}` | 201 `Rule` | yes | write | rules |
| 20 | GET | `/api/guilds/{guild_id}/cases` | staff:moderate | query `member_id`, `moderator_id`, `rule_id`, `state` (`valid`\|`voided`), `action_state`, `appeal_state`, `number`, `created_after`, `created_before`, `limit`, `cursor` | `{cases: CaseSummary[], next_cursor}` | — | read | cases, overview, member |
| 21 | POST | `/api/guilds/{guild_id}/cases` | staff:moderate | `{member_id, rule_id, context: {key: value}, evidence_links: string[], replaces_case_number?: number}` | 201 `Case` | yes | case_create | case dialog |
| 22 | GET | `/api/guilds/{guild_id}/cases/{case_ref}` | staff:moderate | — | `Case` | — | read | cases/$caseRef |
| 23 | POST | `/api/guilds/{guild_id}/cases/{case_ref}/void` | staff:moderate | `{reason}` | `Case` | yes | write | cases/$caseRef |
| 24 | PUT | `/api/guilds/{guild_id}/cases/{case_ref}/context` | staff:moderate | `{context: {key: value}}` | `Case` | yes | write | cases/$caseRef |
| 25 | POST | `/api/guilds/{guild_id}/cases/{case_ref}/evidence` | staff:moderate | `{links: string[]}` (1–10) | `Case` | yes | write | cases/$caseRef |
| 26 | GET | `/api/guilds/{guild_id}/cases/{case_ref}/evidence/{file_id}` | staff:moderate | — | 302 to a signed Discord URL | — | read | cases/$caseRef |
| 27 | GET | `/api/guilds/{guild_id}/members/{member_id}/summary` | staff:moderate | — | `MemberSummary` | — | read | members/$memberId |
| 28 | GET | `/api/guilds/{guild_id}/actions` | staff:moderate | query `state` (default `failed`), `dismissed` (bool, default false), `limit`, `cursor` | `{actions: ActionListItem[], next_cursor}` | — | read | failures, overview |
| 29 | POST | `/api/guilds/{guild_id}/actions/{action_id}/retry` | staff:moderate | — | `Action` | yes | action_control | failures, cases/$caseRef |
| 30 | POST | `/api/guilds/{guild_id}/actions/{action_id}/dismiss` | staff:moderate | — | `Action` | yes | action_control | failures, cases/$caseRef |
| 31 | POST | `/api/guilds/{guild_id}/actions/{action_id}/undo` | staff:moderate | — | 201 `Action` (the undo) | yes | action_control | cases/$caseRef |
| 32 | GET | `/api/guilds/{guild_id}/appeals` | staff:moderate | query `state`, `limit`, `cursor` | `{appeals: AppealSummary[], next_cursor}` | — | read | appeals |
| 33 | GET | `/api/guilds/{guild_id}/appeals/{appeal_id}` | staff:moderate | — | `Appeal` | — | read | appeals/$appealId |
| 34 | POST | `/api/guilds/{guild_id}/appeals/{appeal_id}/decision` | staff:moderate | `{decision, reason, expected_version}` | `Appeal` | yes | write | appeals/$appealId |
| 35 | GET | `/api/guilds/{guild_id}/mod-log` | staff:moderate | §9.6 filters | `{entries: ModLogEntry[], next_cursor}` | — | read | mod-log, member |
| 36 | GET | `/api/guilds/{guild_id}/settings` | staff:manage_settings | — | `Settings` | — | read | settings, overview |
| 37 | PATCH | `/api/guilds/{guild_id}/settings` | staff:manage_settings (+ manage_staff_roles for moderator roles) | `SettingsPatch` | `Settings` | yes | write | settings |
| 38 | POST | `/api/guilds/{guild_id}/settings/starter-notice/acknowledge` | staff:manage_settings | — | `Settings` | yes | write | overview, settings |
| 39 | POST | `/api/guilds/{guild_id}/modules/{module}/enable` | staff:manage_settings | — | `ModuleState` | yes | write | module pages |
| 40 | POST | `/api/guilds/{guild_id}/modules/{module}/disable` | staff:manage_settings | — | `ModuleState` | yes | write | module pages |
| 41 | GET | `/api/guilds/{guild_id}/modules/tickets` | staff:manage_settings | — | `TicketsModule` | — | read | modules/tickets |
| 42 | PUT | `/api/guilds/{guild_id}/modules/tickets/settings` | staff:manage_settings | `TicketSettings` | `TicketsModule` | yes | write | modules/tickets |
| 43 | GET | `/api/guilds/{guild_id}/modules/activity-log` | staff:manage_settings | — | `ActivityLogModule` | — | read | modules/activity-log |
| 44 | PUT | `/api/guilds/{guild_id}/modules/activity-log/settings` | staff:manage_settings | `ActivityLogSettings` | `ActivityLogModule` | yes | write | modules/activity-log |
| 45 | GET | `/api/guilds/{guild_id}/modules/honeypot` | staff:manage_settings | — | `HoneypotModule` | — | read | modules/honeypot |
| 46 | PUT | `/api/guilds/{guild_id}/modules/honeypot/settings` | staff:manage_settings | `HoneypotSettings` | `HoneypotModule` | yes | write | modules/honeypot |
| 47 | GET | `/api/guilds/{guild_id}/tickets` | staff:moderate | query `state`, `limit`, `cursor` | `{tickets: Ticket[], next_cursor}` | — | read | modules/tickets |
| 48 | GET | `/api/guilds/{guild_id}/tickets/{ticket_id}` | the ticket's member, or staff:moderate | — | `Ticket` | — | read | modules/tickets |
| 49 | GET | `/api/guilds/{guild_id}/tickets/{ticket_id}/transcript` | the ticket's member, or staff:moderate | — | `{content, complete, expires_at}` | — | read | modules/tickets |
| 50 | POST | `/api/guilds/{guild_id}/tickets/{ticket_id}/close` | the ticket's member, or staff:moderate | — | `Ticket` | yes | write | modules/tickets |
| 51 | GET | `/api/me/cases` | member | query `guild_id` (optional), `limit`, `cursor` | `{cases: MemberCaseSummary[], next_cursor}` | — | read | member case view |
| 52 | GET | `/api/me/cases/{case_ref}` | member | — | `MemberCase` | — | read | `/guilds/$guildId/cases/$caseId/appeal` |
| 53 | GET | `/api/me/cases/{case_ref}/evidence/{file_id}` | member | — | 302 | — | read | member case view |
| 54 | POST | `/api/me/cases/{case_ref}/appeal` | member | `{statement}` | 201 `MemberCase` | yes | member_write | member case view |
| 55 | POST | `/api/me/cases/{case_ref}/appeal/information` | member | `{text}` | `MemberCase` | yes | member_write | member case view |
| 56 | GET | `/livez` | public | — | `{"status":"ok"}` | — | none | (probes) |
| 57 | GET | `/readyz` | public | — | `Readiness` (200 or 503) | — | none | (probes) |

`{module}` is `tickets`, `activity-log`, or `honeypot`.

**Dropped from the Go API** (no dashboard caller or replaced): `POST
/auth/logout-all`; JSON-mode sign-in and bearer sessions; `/status`,
`/metrics`, `/ops/status`, `/guilds/{id}/ops/status` (replaced by OTLP and
route 7); logging and honeypot `…/status` routes; logging
`repair-channel` and honeypot `repair` (channel deletion is reconciled
automatically, §13.1.5; re-enabling is route 39); ticket `resolve`,
`cancel`, `reopen`; `/members/me/appeals/{id}` (folded into route 52);
`/guilds/{id}/appeals/{id}/reversals` and `/cases/{ref}/reversals`
(automatic undo plus route 31); per-verb appeal routes (route 34);
`/templates/*` (renamed `/rules/*`); `/users/{id}/cases` (route 20 with
`member_id` plus route 27); `/directory/users` (merged into route 9).

### 14.3 Wire types

TypeScript-style notation; `?` means the field may be absent, `| null`
means present and nullable. All types derive `ToSchema`.

```ts
type Me = { account: { id: string; username: string; global_name: string | null; avatar_url: string },
            csrf_token: string, session: { expires_at: string } }
type Capabilities = { moderate: boolean; manage_rules: boolean; manage_settings: boolean; manage_staff_roles: boolean }
type GuildSummary = { guild_id: string; name: string; icon_url: string | null; installed: boolean;
                      capabilities: Capabilities; mfa_required: boolean; can_install: boolean }
type GuildMe = { guild: { guild_id: string; name: string; icon_url: string | null; owner_id: string },
                 capabilities: Capabilities; mfa_required: boolean; is_owner: boolean }
type Health = { quack_in_server: boolean; missing_permissions: Permission[];
                channels: { setting: string; channel_id: string; problems: ChannelProblem[] }[];
                modules: Record<"tickets" | "activity_log" | "honeypot", { enabled: boolean; problems: ModuleProblem[] }> }
type Statistics = { from: string; to: string; cases_total: number;
  cases_by_day: Bucket[]; cases_by_rule: Bucket[]; cases_by_state: Bucket[]; cases_by_source: Bucket[];
  actions_by_kind: Bucket[]; actions_by_state: Bucket[]; appeals_by_state: Bucket[]; mod_log_by_kind: Bucket[] }
type Bucket = { key: string; label: string; count: number }
type DirectoryMember = { id: string; username: string; global_name: string | null; nick: string | null;
                         display_name: string; avatar_url: string; bot: boolean; in_server: boolean }
type DirectoryChannel = { id: string; name: string; kind: "text" | "announcement" | "forum" | "voice" | "stage" | "category";
                          parent_id: string | null; position: number }
type DirectoryRole = { id: string; name: string; color: number; position: number; managed: boolean }
type RuleSummary = { id: string; name: string; description: string; current_version: number;
                     archived_at: string | null; levels_text: string; field_count: number;
                     is_starter: boolean; used_by_honeypot: boolean }
type Rule = { id: string; current_version: number; archived_at: string | null; created_at: string;
              created_by: string | null; updated_at: string; updated_by: string | null;
              definition: RuleDefinition; versions: { version: number; created_at: string; created_by: string | null; reconstructed: boolean }[] }
type RuleFile = { format: "quack-rule"; format_version: 1; exported_at: string; rule: RuleDefinition }
type CaseSummary = { id: string; number: number; member_id: string; rule_id: string | null; rule_name: string;
                     rule_version: number | null; source: CaseSource; moderator_id: string | null; created_at: string;
                     voided_at: string | null; action: { kind: ActionKind; state: ActionState; outcome_text: string } | null;
                     dm_state: DmState; appeal_state: AppealState | null }
type Case = CaseSummary & { case_count: number | null; level_index: number | null; level_from_case_count: number | null;
  reason: string; appealable: boolean; fields: Field[]; context: Record<string, string | number | boolean>;
  void: { by: string | null; reason: string; at: string; cause: "staff" | "appeal_accepted" } | null;
  replaces_case_number: number | null; replaced_by_case_number: number | null;
  actions: Action[]; dm: Dm; evidence: Evidence[]; appeal: AppealSummary | null;
  imported: { v4_action: string; v4_moderator: string | null; v4_reason: string } | null;
  timeline: ModLogEntry[]; member_timeline: { at: string; text: string }[] }
type Action = { id: string; case_id: string; case_number: number; kind: ActionKind; purpose: "original" | "undo";
  state: ActionState; outcome_text: string; timeout_ends_at: string | null; delete_message_seconds: number | null;
  attempts: number; next_attempt_at: string | null; failure_reason: ActionFailure | null;
  http_status: number | null; discord_code: number | null; skipped_because_case_number: number | null;
  requested_by: string | null; dismissed_at: string | null; dismissed_by: string | null; done_at: string | null;
  created_at: string; attempt_log: { number: number; started_at: string; finished_at: string | null;
  outcome: AttemptOutcome | null; http_status: number | null; discord_code: number | null }[];
  can_retry: boolean; can_dismiss: boolean; can_undo: boolean }
type ActionListItem = Action & { member_id: string; rule_name: string }
type Dm = { state: DmState; failure: DmFailure | null; outcome_text: string; settled_at: string | null }
type Evidence = { id: string; state: EvidenceState; failure: EvidenceFailure | null; state_text: string;
  channel_id: string; message_id: string; url: string; author_id: string | null; content: string | null;
  message_created_at: string | null; message_edited_at: string | null;
  embeds: { type: string; title: string | null; description: string | null; url: string | null }[];
  files: { id: string; filename: string; content_type: string; size_bytes: number; copy_state: FileCopyState; state_text: string }[];
  added_by: string | null; created_at: string }
type AppealSummary = { id: string; case_id: string; case_number: number; member_id: string; rule_name: string;
                       state: AppealState; version: number; created_at: string; updated_at: string }
type Appeal = AppealSummary & { statement: string; decided_by: string | null; decided_at: string | null;
  decision_reason: string | null; events: { id: string; kind: AppealEventKind; actor_id: string | null; text: string; created_at: string }[];
  case: CaseSummary }
type MemberCaseSummary = { id: string; number: number; guild: { guild_id: string; name: string; icon_url: string | null };
  rule_name: string; reason: string; created_at: string; voided_at: string | null; action_text: string;
  can_appeal: boolean; appeal_state: AppealState | null }
type MemberCase = MemberCaseSummary & { void_reason: string | null; undo_text: string | null; timeout_ends_at: string | null;
  dm_text: string; context: { label: string; kind: FieldKind; value: string | number | boolean }[];
  evidence: { id: string; content: string | null; embeds: { type: string; title: string | null; description: string | null }[];
              files: { id: string; filename: string; content_type: string; size_bytes: number; available: boolean }[] }[];
  replaces_case_number: number | null; replaced_by_case_number: number | null;
  appeal: { id: string; state: AppealState; statement: string; created_at: string; can_reply: boolean;
            events: { kind: AppealEventKind; text: string; at: string; by_you: boolean }[] } | null;
  timeline: { at: string; text: string }[] }
type MemberSummary = { member_id: string; total: number; valid: number; voided: number;
                       by_rule: { rule_id: string | null; rule_name: string; valid: number; voided: number }[] }
type ModLogEntry = { id: string; kind: ModLogKind; result: ModLogResult; actor_id: string | null; source: ModLogSource;
  case_id: string | null; case_number: number | null; member_id: string | null; appeal_id: string | null;
  action_id: string | null; rule_id: string | null; ticket_id: string | null; detail: object; text: string; created_at: string }
type Settings = { version: number; moderator_role_ids: string[]; rules_manager_role_ids: string[];
  log_channel_id: string | null; appeal_channel_id: string | null; evidence_channel_id: string | null;
  appeal_invite_url: string | null; dm_intro: string | null; dm_footer: string | null;
  starter_rule_id: string | null; starter_notice_acknowledged_at: string | null;
  modules: Record<"tickets" | "activity_log" | "honeypot", ModuleState> }
type SettingsPatch = Partial<Omit<Settings, "version" | "starter_rule_id" | "starter_notice_acknowledged_at" | "modules">>
                     & { expected_version?: number }
type ModuleState = { enabled: boolean; disabled_reason: "channel_deleted" | null; problems: ModuleProblem[] }
type ModuleProblem = { code: string; message: string }
type TicketSettings = { entry_channel_id: string | null; queue_channel_id: string | null; transcript_retention_days: number }
type TicketsModule = ModuleState & { settings: TicketSettings; open_tickets: number }
type ActivityLogSettings = { routes: Partial<Record<ActivityEvent, string>>; include_message_content: boolean;
                             include_file_names: boolean; include_embed_types: boolean }
type ActivityLogModule = ModuleState & { settings: ActivityLogSettings; posted_last_hour: number; dropped_last_hour: number }
type HoneypotSettings = { channel_id: string | null; rule_id: string | null; warning_text: string | null }
type HoneypotModule = ModuleState & { settings: HoneypotSettings; cases_created: number; pending_hits: number; failed_hits: number }
type Ticket = { id: string; short_id: string; member_id: string; state: TicketState; thread_id: string | null;
  opened_at: string | null; closed_at: string | null; closed_by: string | null; close_reason: "open_failed" | null;
  transcript_available: boolean; transcript_complete: boolean | null; created_at: string }
type Readiness = { ready: boolean; checks: Record<"database" | "migrations" | "gateway" | "workers" | "shutdown", "ok" | "failing"> }
```

`activity_log` module counts come from in-process counters since start.

**Statistics** (route 8): `to` defaults to now and `from` to one calendar
month before `to`; the range must be 1 second to 366 days (else
`validation_failed`). Rows are counted by `created_at` in `[from, to)`
using their current state, grouped by UTC day (`cases_by_day`), by rule
(`cases_by_rule`, imported cases under "Imported"), by valid/voided, by
source, by action kind and state (original and undo actions), by appeal
state, and by moderation log kind. Buckets without rows are omitted. One
query per group, all in one `REPEATABLE READ` read-only transaction. No
per-moderator ranking.

### 14.4 Authentication

Discord OAuth 2.0 authorization code flow with PKCE (S256), scopes
`identify guilds`, redirect URI `<QUACK_PUBLIC_URL>/api/auth/callback`
(register it in the Discord developer portal). The `oauth2` crate builds the
URLs and does the exchange through a small reqwest adapter.

1. **Login** (route 1). `redirect_to` must start with `/`, must not start
   with `//`, and must contain no `\` or control characters; otherwise
   `/guilds`. Generate `state` (32 random bytes), a PKCE verifier (64
   characters), and a browser nonce (32 random bytes). Insert
   `oauth_states (state_hash = SHA-256(state), code_verifier, redirect_to,
   nonce_hash, expires_at = now + 10 min)`. Set cookie `quack_oauth=<nonce>`
   (`HttpOnly; Secure`¹`; SameSite=Lax; Path=/api/auth; Max-Age=600`).
   302 to `https://discord.com/oauth2/authorize?…&state=…&code_challenge=…&code_challenge_method=S256`.
2. **Callback** (route 2). `error` → 302 `/login?error=denied`.
   `DELETE FROM oauth_states WHERE state_hash = $1 AND expires_at > now() RETURNING *`
   (single use); no row, or the cookie nonce's hash differs → 302
   `/login?error=failed`. Exchange the code (10 s deadline, response body ≤
   1 MiB, no redirects), `GET /users/@me` with the access token, upsert
   `discord_accounts (mfa_enabled, mfa_checked_at = now())`, create the
   session, clear `quack_oauth`, set the session cookie, 302 to
   `redirect_to`. Any failure → 302 `/login?error=failed`. Metric
   `quack.auth.sign_ins{outcome}` (`ok`, `denied`, `failed`).
3. **Session.** 32 random bytes, sent base64url in the cookie; the table
   stores only `SHA-256(id)`. Row: account, CSRF token (32 random bytes),
   the Discord access token encrypted with AES-256-GCM under
   `QUACK_SESSION_SECRET` (needed only for route 5), `created_at`,
   `last_seen_at`, `expires_at = now + min(Discord token lifetime, 7 days)`.
   There is no refresh: when the session ends the member signs in again.
   Each request: `SELECT … WHERE id_hash = $1 AND expires_at > now()`; touch
   `last_seen_at` at most every 5 minutes.
4. **Cookie.** `__Host-quack_session` when `QUACK_PUBLIC_URL` is `https`,
   `quack_session` otherwise (local development). `HttpOnly; Secure`¹`;
   SameSite=Lax; Path=/; Max-Age=<seconds until expires_at>`.
5. **Logout** (route 4) deletes the row and clears the cookie.

¹ `Secure` is set whenever `QUACK_PUBLIC_URL` is `https`.

Server membership for staff routes is checked live through the `Discord`
port (member cache, REST fallback), never from the session.
Route 5 combines the account's Discord server list (`GET
/users/@me/guilds` with the session's token, cached 60 s), the servers where
Quack is installed, and capabilities computed per installed server
(`can_install` = not installed and the account has Manage Server there).

### 14.5 CSRF and origin

For POST, PUT, PATCH, and DELETE under `/api`:

1. `Origin` must equal the origin of `QUACK_PUBLIC_URL`. Without `Origin`,
   `Sec-Fetch-Site` must be `same-origin`. Otherwise `403 origin_rejected`.
2. `X-CSRF-Token` must equal the session's CSRF token (constant-time
   compare). Otherwise `403 csrf_rejected`.

No CORS headers are sent: the dashboard is same-origin, and other origins
are refused by rule 1.

### 14.6 Errors

Envelope:

```json
{ "error": {
    "code": "validation_failed",
    "message": "Some fields need fixing.",
    "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736",
    "fields": [ { "field": "levels[1].from_case_count", "code": "level_order", "message": "…" } ],
    "problems": [ { "code": "ticket_queue_missing", "message": "Choose the staff channel for new tickets." } ],
    "retry_after_seconds": 12
} }
```

`fields`, `problems`, and `retry_after_seconds` appear only where listed.
`message` is always a §12 text; it never contains Discord or database
error text.

| Status | `code` | Message id | Extra |
| --- | --- | --- | --- |
| 400 | `validation_failed` | `api.validation_failed` | `fields` (field path, problem code from §3.3 / §4.6 / §13.1, message) |
| 400 | `invalid_json` | `api.invalid_json` | |
| 401 | `sign_in_required` | `api.sign_in_required` | |
| 401 | `session_expired` | `api.session_expired` | |
| 403 | `origin_rejected` | `api.origin_rejected` | |
| 403 | `csrf_rejected` | `api.csrf_rejected` | |
| 403 | `missing_capability` | `error.missing_capability.<capability>` | |
| 403 | `mfa_required` | `error.mfa_required` | |
| 404 | `not_found` | `api.not_found`, `error.case_not_found`, `error.appeal_not_found`, `error.rule_not_found` | |
| 404 | `server_not_found` | `api.server_not_found` | |
| 404 | `file_unavailable` | `api.file_unavailable` | |
| 409 | `idempotency_key_reused` | `api.idempotency_key_reused` | |
| 409 | `rule_changed` | `api.rule_changed` | |
| 409 | `rule_name_taken` | `api.rule_name_taken` | |
| 409 | `rule_used_by_honeypot` | `api.rule_used_by_honeypot` | |
| 409 | `required_field_on_honeypot_rule` | `api.required_field_on_honeypot_rule` | |
| 409 | `too_many_rules` | `api.too_many_rules` | |
| 409 | `settings_changed` | `api.settings_changed` | |
| 409 | `case_already_voided` | `error.case_already_voided` | |
| 409 | `case_voided` | `error.case_voided` or `appeal.submit.voided` | |
| 409 | `action_not_retryable` | `error.action_not_retryable` / `error.action_not_dismissable` | |
| 409 | `action_not_undoable` | `error.action_not_undoable` | |
| 409 | `appeal_changed` | `error.appeal_changed` | |
| 409 | `appeal_exists` | `api.appeal_exists` | |
| 409 | `ticket_not_open` | `api.ticket_not_open` | |
| 409 | `module_not_ready` | `api.module_not_ready` | `problems` |
| 409 | `replacement_invalid` | `error.replacement_invalid` | |
| 413 | `body_too_large` | `api.body_too_large` | |
| 415 | `unsupported_media_type` | `api.unsupported_media_type` | |
| 422 | `rule_archived` | `error.rule_archived` | |
| 422 | `member_refused` | the matching `refusal.*` | `fields: [{field: "member_id", code: <refusal>}]` |
| 422 | `appeal_not_allowed` | `api.appeal_not_allowed` | |
| 422 | `timeout_window_passed` | `error.timeout_window_passed` | |
| 422 | `evidence_limit_reached` | `error.evidence_limit_reached` | |
| 428 | `idempotency_key_required` | `api.idempotency_key_required` | |
| 429 | `rate_limited` | `api.rate_limited` | `retry_after_seconds`, header `Retry-After` |
| 502 | `discord_unavailable` | `api.discord_unavailable` | |
| 503 | `unavailable` | `api.unavailable` | |
| 500 | `internal_error` | `api.internal_error` | |

`member_refused` codes: `quack_not_in_server`, `actor_not_in_server`,
`member_not_in_server`, `member_is_self`, `member_is_bot`,
`member_is_owner`, `member_role_too_high`, `quack_role_too_low`,
`quack_missing_permission`, `member_is_administrator`.

Member routes answer `404 not_found` for a case or appeal that is not
theirs. Unknown paths and wrong methods under `/api` are `404 not_found`.

### 14.7 Rate limits

In-process token buckets (one map per class, at most 100,000 keys, LRU
eviction). A refused request gets 429 with `Retry-After`.

| Class | Key | Limit |
| --- | --- | --- |
| `sign_in` | client IP | 20 per 10 minutes |
| `read` | account | 120 per minute |
| `write` | account + server | 30 per minute |
| `case_create` | account + server | 20 per minute |
| `action_control` | account + server | 10 per minute |
| `member_write` | account | 10 per minute |

Client IP is the socket peer, or, when `QUACK_TRUSTED_PROXY_HOPS = n > 0`,
the n-th address from the right of `X-Forwarded-For`. Metric
`quack.http.rate_limited{class}`.

### 14.8 Limits, timeouts, headers

- Request body ≤ 262,144 bytes (`413 body_too_large`).
- Whole request deadline 10 s (`503 unavailable`); header read 5 s; idle
  keep-alive 60 s. Database statement timeout 5 s (2 s inside case
  creation, §4.2).
- Headers on every response: `X-Content-Type-Options: nosniff`,
  `Referrer-Policy: no-referrer`, `X-Frame-Options: DENY`,
  `Permissions-Policy: camera=(), microphone=(), geolocation=()`, and
  `Strict-Transport-Security: max-age=31536000` when the public URL is
  `https`.
- `/api` and health responses also: `Cache-Control: no-store`,
  `Content-Security-Policy: default-src 'none'; frame-ancestors 'none'`.
- Request logs and spans use the route pattern, never the raw path or
  query (the callback's `code` and `state` never reach telemetry).

### 14.9 Health endpoints

- `GET /livez`: 200 `{"status":"ok"}` while the process serves HTTP. Never
  checks dependencies, so outages don't cause restart loops.
- `GET /readyz`: 200 when every check is `ok`, else 503; body `Readiness`.
  Checks: `database` (`SELECT 1` within 1 s), `migrations` (the applied set
  equals the binary's set, verified at startup), `gateway` (shard
  connected and `READY` received), `workers` (every job dispatcher ticked
  within the last 15 s), `shutdown` (not shutting down).

### 14.10 Dashboard files

`quack serve` serves the built dashboard from `QUACK_DASHBOARD_DIR`:

| Request | Response |
| --- | --- |
| `GET /assets/<file>` that exists | the file, `Cache-Control: public, max-age=31536000, immutable` |
| `GET /<file>` that exists (e.g. `favicon.svg`) | the file, `Cache-Control: no-cache` |
| `GET /<path>` with no extension, not under `/api`, `/livez`, `/readyz` | `index.html`, `Cache-Control: no-cache`, dashboard CSP below |
| `GET /<path>` with an extension that doesn't exist | 404, plain text |
| Other methods outside `/api` | 405 with `Allow: GET, HEAD` |

Text responses (HTML, JS, CSS, JSON, SVG) are gzip-compressed when the
client accepts it. Dashboard CSP: `default-src 'self'; img-src 'self'
https://cdn.discordapp.com https://media.discordapp.net data:; style-src
'self' 'unsafe-inline'; script-src 'self' https://visitors.now; connect-src
'self' https://visitors.now; frame-ancestors 'none'; base-uri 'none';
form-action 'self'`.

### 14.11 Dashboard URL map

Stable public paths that Discord messages link to:

| Path | Who | Linked from |
| --- | --- | --- |
| `/guilds/{guild_id}` | staff | `/help`, `/setup` |
| `/guilds/{guild_id}/cases` | staff | `/case list` |
| `/guilds/{guild_id}/cases/{number}` | staff | case card, receipt, log posts about a case |
| `/guilds/{guild_id}/members/{member_id}` | staff | `/case history` |
| `/guilds/{guild_id}/appeals/{appeal_id}` | staff | appeal post, `/appeals`, log posts about appeals |
| `/guilds/{guild_id}/failures` | staff | log posts about failed actions |
| `/guilds/{guild_id}/rules`, `/guilds/{guild_id}/rules/{rule_id}` | staff | `/rules`, log posts about rules |
| `/guilds/{guild_id}/mod-log` | staff | log posts (subtext) |
| `/guilds/{guild_id}/settings` | staff | `/setup`, log posts about settings |
| `/guilds/{guild_id}/modules/{tickets\|activity-log\|honeypot}` | staff | module log posts, `/setup` |
| `/guilds/{guild_id}/cases/{case_id}/appeal` | the case's member | case DM, appeal DMs, appeal replies |

The dashboard keeps redirects from the Go-era paths `/guilds/{id}/audit`
→ `/mod-log` and `/guilds/{id}/modules/logging` → `/modules/activity-log`.
Member links from old DMs carry internal ULIDs (server and case); the member
page ignores the server segment and resolves the case by ULID (§19.3).

### 14.12 OpenAPI and the TypeScript client

- Handlers and DTOs carry utoipa annotations (`#[utoipa::path]`,
  `ToSchema`); routes are registered with `utoipa_axum::router::OpenApiRouter`
  (one path per `routes!` invocation). `quack_api::openapi::document()`
  builds the OpenAPI 3.1 document.
- `contracts/http/openapi.json` is committed. The test
  `quack_api::openapi_is_current` fails when it differs from
  `document()`; `UPDATE_OPENAPI=1 cargo nextest run -p quack-api
  openapi_is_current` rewrites it.
- `apps/dashboard`: `bun run api` runs `openapi-typescript
  ../../contracts/http/openapi.json -o src/api/schema.gen.ts`. CI regenerates
  and fails on a diff.

---

## 15. Database schema

PostgreSQL 18. Rules:

- Primary keys are UUIDv7 (`uuid`, generated in Rust with
  `Uuid::now_v7()`, `DEFAULT uuidv7()` as a fallback), except `guilds`,
  whose key is the Discord server ID, and natural-key tables.
- Discord snowflakes are `bigint` (always positive, below 2^63); newtypes in
  Rust (`GuildId`, `MemberId`, `AccountId`, `ChannelId`, `MessageId`,
  `RoleId`), strings in JSON.
- `timestamptz` everywhere; the application writes UTC.
- Every tenant table has `guild_id`. A child that must belong to the same
  server as its parent uses a composite foreign key `(guild_id, parent_id)`
  against `UNIQUE (guild_id, id)`.
- States are Postgres enums; invariants are `CHECK` constraints.
- Migrations are immutable SQL files in `crates/quack-db/migrations/`
  (`0001_core.sql`, `0002_modules.sql`, then one file per change), applied by
  `quack migrate` (sqlx migrator, ledger `_sqlx_migrations`). `quack serve`
  only verifies the applied set equals the binary's set, unless started with
  `--migrate`.
- `NOTIFY` channels: `quack_jobs` (payload: job kind), `quack_rules`
  (payload: guild ID), `quack_settings` (payload: guild ID).

### 15.1 Enums

```sql
CREATE TYPE case_source            AS ENUM ('discord', 'dashboard', 'honeypot', 'imported');
CREATE TYPE void_cause             AS ENUM ('staff', 'appeal_accepted');
CREATE TYPE action_kind            AS ENUM ('timeout', 'kick', 'ban', 'remove_timeout', 'unban');
CREATE TYPE action_state           AS ENUM ('pending', 'running', 'done', 'failed', 'cancelled', 'skipped');
CREATE TYPE action_failure         AS ENUM ('quack_not_in_server', 'quack_missing_permission', 'quack_role_too_low',
                                            'member_is_owner', 'member_not_in_server', 'member_is_administrator',
                                            'timeout_window_passed', 'discord_refused', 'retries_exhausted');
CREATE TYPE attempt_outcome        AS ENUM ('done', 'no_change', 'refused', 'rate_limited', 'unknown', 'interrupted');
CREATE TYPE discord_permission     AS ENUM ('moderate_members', 'kick_members', 'ban_members');
CREATE TYPE dm_kind                AS ENUM ('case', 'appeal_update', 'ticket_closed');
CREATE TYPE dm_state               AS ENUM ('pending', 'sending', 'sent', 'failed', 'unknown', 'not_needed');
CREATE TYPE dm_failure             AS ENUM ('dms_closed', 'not_in_shared_server', 'dm_channel_unavailable', 'discord_refused');
CREATE TYPE evidence_state         AS ENUM ('saving', 'saved', 'failed');
CREATE TYPE evidence_failure       AS ENUM ('message_deleted', 'moderator_no_access', 'quack_no_access', 'other_server', 'discord_error');
CREATE TYPE file_copy_state        AS ENUM ('pending', 'copied', 'link_only', 'too_large', 'unsupported_type', 'copy_failed', 'limit_reached');
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
CREATE TYPE module_disabled_reason AS ENUM ('channel_deleted');
CREATE TYPE activity_event         AS ENUM ('message_edited', 'message_deleted', 'messages_bulk_deleted', 'member_joined',
                                            'member_left', 'member_banned', 'member_unbanned', 'server_changed',
                                            'channel_changed');
CREATE TYPE honeypot_hit_state     AS ENUM ('pending', 'case_created', 'failed');
```

### 15.2 Servers, accounts, sessions

```sql
-- One row per Discord server Quack has ever joined. Never deleted.
CREATE TABLE guilds (
  guild_id        bigint PRIMARY KEY CHECK (guild_id > 0),          -- the Discord server ID
  name            text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 100),
  icon_hash       text NULL CHECK (char_length(icon_hash) <= 64),
  owner_id        bigint NOT NULL CHECK (owner_id > 0),
  mfa_required    boolean NOT NULL DEFAULT false,                   -- Discord MFA level is elevated
  installed       boolean NOT NULL DEFAULT true,                    -- false after Quack leaves
  first_joined_at timestamptz NOT NULL DEFAULT now(),
  left_at         timestamptz NULL,
  updated_at      timestamptz NOT NULL DEFAULT now()
);

-- Per-server case number counter; its row lock serializes case creation.
CREATE TABLE case_counters (
  guild_id    bigint PRIMARY KEY REFERENCES guilds (guild_id),
  last_number bigint NOT NULL DEFAULT 0 CHECK (last_number >= 0)
);

-- Discord accounts that signed in to the dashboard; holds the 2FA confirmation.
CREATE TABLE discord_accounts (
  account_id     bigint PRIMARY KEY CHECK (account_id > 0),
  mfa_enabled    boolean NOT NULL,
  mfa_checked_at timestamptz NOT NULL,                              -- when Discord last reported mfa_enabled
  updated_at     timestamptz NOT NULL DEFAULT now()
);

-- Dashboard sessions. Only the SHA-256 of the session ID is stored.
CREATE TABLE sessions (
  id_hash                  bytea PRIMARY KEY CHECK (octet_length(id_hash) = 32),
  account_id               bigint NOT NULL CHECK (account_id > 0),
  csrf_token               text NOT NULL CHECK (char_length(csrf_token) = 43),
  discord_token_ciphertext bytea NOT NULL,                          -- AES-256-GCM(nonce || ciphertext || tag)
  created_at               timestamptz NOT NULL DEFAULT now(),
  last_seen_at             timestamptz NOT NULL DEFAULT now(),
  expires_at               timestamptz NOT NULL
);
CREATE INDEX sessions_account ON sessions (account_id);
CREATE INDEX sessions_expiry  ON sessions (expires_at);

-- One-time OAuth login states (10 minutes).
CREATE TABLE oauth_states (
  state_hash    bytea PRIMARY KEY CHECK (octet_length(state_hash) = 32),
  code_verifier text NOT NULL CHECK (char_length(code_verifier) BETWEEN 43 AND 128),
  redirect_to   text NOT NULL CHECK (char_length(redirect_to) BETWEEN 1 AND 2048),
  nonce_hash    bytea NOT NULL CHECK (octet_length(nonce_hash) = 32),
  created_at    timestamptz NOT NULL DEFAULT now(),
  expires_at    timestamptz NOT NULL
);
CREATE INDEX oauth_states_expiry ON oauth_states (expires_at);
```

### 15.3 Rules and settings

```sql
-- Rule identity; the definition lives in rule_versions.
CREATE TABLE rules (
  id              uuid PRIMARY KEY DEFAULT uuidv7(),
  guild_id        bigint NOT NULL REFERENCES guilds (guild_id),
  name            text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 100),  -- current version's name
  current_version integer NOT NULL CHECK (current_version >= 1),
  archived_at     timestamptz NULL,
  created_at      timestamptz NOT NULL DEFAULT now(),
  created_by      bigint NULL,                                      -- NULL = Quack
  updated_at      timestamptz NOT NULL DEFAULT now(),
  updated_by      bigint NULL,
  UNIQUE (guild_id, id)
);
CREATE UNIQUE INDEX rules_active_name ON rules (guild_id, lower(name)) WHERE archived_at IS NULL;
CREATE INDEX rules_guild ON rules (guild_id, archived_at);

-- Immutable rule versions.
CREATE TABLE rule_versions (
  rule_id       uuid NOT NULL,
  version       integer NOT NULL CHECK (version >= 1),
  guild_id      bigint NOT NULL,
  definition    jsonb NOT NULL CHECK (jsonb_typeof(definition) = 'object'),  -- RuleDefinition (§3.2)
  reconstructed boolean NOT NULL DEFAULT false,                     -- partial, rebuilt by the importer (§19.5)
  created_at    timestamptz NOT NULL DEFAULT now(),
  created_by    bigint NULL,
  PRIMARY KEY (rule_id, version),
  FOREIGN KEY (guild_id, rule_id) REFERENCES rules (guild_id, id)
);

ALTER TABLE rules ADD FOREIGN KEY (id, current_version)
  REFERENCES rule_versions (rule_id, version) DEFERRABLE INITIALLY DEFERRED;

CREATE FUNCTION reject_change() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  RAISE EXCEPTION '% rows are immutable', TG_TABLE_NAME USING ERRCODE = 'insufficient_privilege';
END $$;
CREATE TRIGGER rule_versions_immutable BEFORE UPDATE OR DELETE ON rule_versions
  FOR EACH ROW EXECUTE FUNCTION reject_change();

-- Core server settings.
CREATE TABLE guild_settings (
  guild_id                       bigint PRIMARY KEY REFERENCES guilds (guild_id),
  version                        bigint NOT NULL DEFAULT 1,       -- optimistic check for staff role edits
  moderator_role_ids             bigint[] NOT NULL DEFAULT '{}' CHECK (cardinality(moderator_role_ids) <= 25),
  rules_manager_role_ids         bigint[] NOT NULL DEFAULT '{}' CHECK (cardinality(rules_manager_role_ids) <= 25),
  log_channel_id                 bigint NULL,
  appeal_channel_id              bigint NULL,
  evidence_channel_id            bigint NULL,
  appeal_invite_url              text NULL CHECK (appeal_invite_url ~ '^https://discord\.gg/[A-Za-z0-9-]{2,32}$'),
  dm_intro                       text NULL CHECK (char_length(dm_intro) BETWEEN 1 AND 1000),
  dm_footer                      text NULL CHECK (char_length(dm_footer) BETWEEN 1 AND 500),
  starter_rule_id                uuid NULL,
  starter_notice_acknowledged_at timestamptz NULL,
  updated_at                     timestamptz NOT NULL DEFAULT now(),
  updated_by                     bigint NULL,
  FOREIGN KEY (guild_id, starter_rule_id) REFERENCES rules (guild_id, id)
);
```

### 15.4 Cases, actions, DMs, evidence, appeals

Module tables (§15.7) are created before `dms` because `dms` references
`tickets`; the listing here groups tables by topic.

```sql
-- One row per case. Never deleted.
CREATE TABLE cases (
  id                         uuid PRIMARY KEY DEFAULT uuidv7(),
  guild_id                   bigint NOT NULL REFERENCES guilds (guild_id),
  number                     bigint NOT NULL CHECK (number >= 1),
  member_id                  bigint NOT NULL CHECK (member_id > 0),
  source                     case_source NOT NULL,
  moderator_id               bigint NULL,                         -- NULL for honeypot and some imported cases
  rule_id                    uuid NULL,
  rule_version               integer NULL,
  case_count                 integer NULL CHECK (case_count >= 1),
  level_index                smallint NULL CHECK (level_index BETWEEN 1 AND 10),
  level_from_case_count      integer NULL CHECK (level_from_case_count >= 1),
  action_kind                action_kind NULL CHECK (action_kind IN ('timeout', 'kick', 'ban')),
  timeout_seconds            integer NULL CHECK (timeout_seconds BETWEEN 1 AND 2419200),
  ban_delete_message_seconds integer NULL CHECK (ban_delete_message_seconds BETWEEN 0 AND 604800),
  sends_dm                   boolean NOT NULL,
  context                    jsonb NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(context) = 'object'),  -- field key → value
  operation_key              text NULL CHECK (char_length(operation_key) <= 200),
  replaces_case_id           uuid NULL,
  voided_at                  timestamptz NULL,
  voided_by                  bigint NULL,                         -- NULL = Quack (never today) or imported
  void_reason                text NULL CHECK (char_length(void_reason) BETWEEN 1 AND 1000),
  void_cause                 void_cause NULL,
  imported                   jsonb NULL,                          -- {v4_action, v4_moderator, v4_reason, v4_source_id}
  created_at                 timestamptz NOT NULL DEFAULT now(),
  updated_at                 timestamptz NOT NULL DEFAULT now(),
  UNIQUE (guild_id, number),
  UNIQUE (guild_id, id),
  UNIQUE (guild_id, operation_key),
  UNIQUE (replaces_case_id),
  FOREIGN KEY (guild_id, rule_id) REFERENCES rules (guild_id, id),
  FOREIGN KEY (rule_id, rule_version) REFERENCES rule_versions (rule_id, version),
  FOREIGN KEY (guild_id, replaces_case_id) REFERENCES cases (guild_id, id),
  CHECK ((voided_at IS NULL) = (void_reason IS NULL) AND (voided_at IS NULL) = (void_cause IS NULL)),
  CHECK ((source = 'imported') = (imported IS NOT NULL)),
  CHECK (source = 'imported' OR (rule_id IS NOT NULL AND rule_version IS NOT NULL AND case_count IS NOT NULL
                                 AND level_index IS NOT NULL AND level_from_case_count IS NOT NULL)),
  CHECK (source IN ('honeypot', 'imported') OR moderator_id IS NOT NULL),
  CHECK ((action_kind IS NOT DISTINCT FROM 'timeout') = (timeout_seconds IS NOT NULL)),
  CHECK ((action_kind IS NOT DISTINCT FROM 'ban') = (ban_delete_message_seconds IS NOT NULL)),
  CHECK (source <> 'imported' OR (action_kind IS NULL AND NOT sends_dm))
);
CREATE INDEX cases_escalation ON cases (guild_id, member_id, rule_id, created_at)
  WHERE voided_at IS NULL AND source <> 'imported';
CREATE INDEX cases_member     ON cases (guild_id, member_id, number DESC);
CREATE INDEX cases_rule       ON cases (guild_id, rule_id, number DESC);
CREATE INDEX cases_moderator  ON cases (guild_id, moderator_id, number DESC) WHERE moderator_id IS NOT NULL;
CREATE INDEX cases_member_all ON cases (member_id, created_at DESC, id DESC);  -- a member's own cases

-- Original actions (timeout, kick, ban) and undo actions (remove_timeout, unban).
CREATE TABLE actions (
  id                      uuid PRIMARY KEY DEFAULT uuidv7(),
  guild_id                bigint NOT NULL,
  case_id                 uuid NOT NULL,
  member_id               bigint NOT NULL,                        -- copy of the case's member, for the undo guard
  kind                    action_kind NOT NULL,
  undo_of_action_id       uuid NULL,                              -- set exactly for undo actions
  state                   action_state NOT NULL DEFAULT 'pending',
  timeout_ends_at         timestamptz NULL,                       -- fixed at case creation (§5.2)
  delete_message_seconds  integer NULL CHECK (delete_message_seconds BETWEEN 0 AND 604800),
  requested_by            bigint NULL,                            -- staff who requested an undo or retry; NULL = Quack
  attempts                integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
  next_attempt_at         timestamptz NULL,
  last_attempt_at         timestamptz NULL,
  failure_reason          action_failure NULL,
  failure_permission      discord_permission NULL,                -- for quack_missing_permission
  failure_http_status     smallint NULL,
  failure_discord_code    integer NULL,
  skipped_because_case_id uuid NULL,
  no_change               boolean NOT NULL DEFAULT false,         -- done because the desired state already held
  done_at                 timestamptz NULL,
  dismissed_at            timestamptz NULL,
  dismissed_by            bigint NULL,
  created_at              timestamptz NOT NULL DEFAULT now(),
  updated_at              timestamptz NOT NULL DEFAULT now(),
  UNIQUE (guild_id, id),
  UNIQUE (undo_of_action_id),
  FOREIGN KEY (guild_id, case_id) REFERENCES cases (guild_id, id),
  FOREIGN KEY (guild_id, undo_of_action_id) REFERENCES actions (guild_id, id),
  FOREIGN KEY (guild_id, skipped_because_case_id) REFERENCES cases (guild_id, id),
  CHECK ((kind IN ('remove_timeout', 'unban')) = (undo_of_action_id IS NOT NULL)),
  CHECK ((kind = 'timeout') = (timeout_ends_at IS NOT NULL)),
  CHECK ((kind = 'ban') = (delete_message_seconds IS NOT NULL)),
  CHECK ((state = 'failed') = (failure_reason IS NOT NULL)),
  CHECK ((failure_reason IS NOT DISTINCT FROM 'quack_missing_permission') = (failure_permission IS NOT NULL)),
  CHECK ((state = 'skipped') = (skipped_because_case_id IS NOT NULL)),
  CHECK ((state = 'done') = (done_at IS NOT NULL)),
  CHECK (dismissed_at IS NULL OR state = 'failed'),
  CHECK ((dismissed_at IS NULL) = (dismissed_by IS NULL))
);
CREATE UNIQUE INDEX actions_one_original ON actions (case_id) WHERE undo_of_action_id IS NULL;
CREATE INDEX actions_case   ON actions (case_id);
CREATE INDEX actions_member ON actions (guild_id, member_id, kind) WHERE state IN ('pending', 'running', 'done');
CREATE INDEX actions_review ON actions (guild_id, updated_at DESC, id DESC) WHERE state = 'failed';

-- Every attempt, recorded before the Discord call.
CREATE TABLE action_attempts (
  guild_id     bigint NOT NULL,
  action_id    uuid NOT NULL,
  number       integer NOT NULL CHECK (number >= 1),
  started_at   timestamptz NOT NULL,
  finished_at  timestamptz NULL,
  outcome      attempt_outcome NULL,
  http_status  smallint NULL,
  discord_code integer NULL,
  PRIMARY KEY (action_id, number),
  FOREIGN KEY (guild_id, action_id) REFERENCES actions (guild_id, id),
  CHECK ((finished_at IS NULL) = (outcome IS NULL))
);

-- Case DMs, appeal update DMs, ticket transcript DMs.
CREATE TABLE dms (
  id                   uuid PRIMARY KEY DEFAULT uuidv7(),
  guild_id             bigint NOT NULL,
  member_id            bigint NOT NULL,
  kind                 dm_kind NOT NULL,
  case_id              uuid NULL,
  appeal_event_id      uuid NULL,
  ticket_id            uuid NULL,
  state                dm_state NOT NULL DEFAULT 'pending',
  failure              dm_failure NULL,
  failure_discord_code integer NULL,
  channel_id           bigint NULL,                               -- the DM channel
  message_id           bigint NULL,
  attempted_at         timestamptz NULL,                          -- set when state became 'sending'
  settled_at           timestamptz NULL,
  created_at           timestamptz NOT NULL DEFAULT now(),
  UNIQUE (appeal_event_id),
  UNIQUE (ticket_id),
  FOREIGN KEY (guild_id, case_id) REFERENCES cases (guild_id, id),
  FOREIGN KEY (guild_id, appeal_event_id) REFERENCES appeal_events (guild_id, id),
  FOREIGN KEY (guild_id, ticket_id) REFERENCES tickets (guild_id, id),
  CHECK (kind <> 'case'          OR (case_id IS NOT NULL AND appeal_event_id IS NULL AND ticket_id IS NULL)),
  CHECK (kind <> 'appeal_update' OR (case_id IS NOT NULL AND appeal_event_id IS NOT NULL AND ticket_id IS NULL)),
  CHECK (kind <> 'ticket_closed' OR (ticket_id IS NOT NULL AND case_id IS NULL AND appeal_event_id IS NULL)),
  CHECK ((state = 'failed') = (failure IS NOT NULL)),
  CHECK ((state = 'sent') = (message_id IS NOT NULL)),
  CHECK ((settled_at IS NULL) = (state IN ('pending', 'sending')))
);
CREATE UNIQUE INDEX dms_one_per_case ON dms (case_id) WHERE kind = 'case';

-- Saved messages. One row per (case, message).
CREATE TABLE evidence (
  id                          uuid PRIMARY KEY DEFAULT uuidv7(),
  guild_id                    bigint NOT NULL,
  case_id                     uuid NOT NULL,
  channel_id                  bigint NOT NULL,
  message_id                  bigint NOT NULL,
  added_by                    bigint NULL,                        -- NULL = Quack (honeypot, imported)
  state                       evidence_state NOT NULL DEFAULT 'saving',
  failure                     evidence_failure NULL,
  author_id                   bigint NULL,
  content                     text NULL CHECK (char_length(content) <= 4000),
  message_created_at          timestamptz NULL,
  message_edited_at           timestamptz NULL,
  embeds                      jsonb NOT NULL DEFAULT '[]'
                              CHECK (jsonb_typeof(embeds) = 'array' AND jsonb_array_length(embeds) <= 10),
  copy_skipped_not_staff_only boolean NOT NULL DEFAULT false,
  created_at                  timestamptz NOT NULL DEFAULT now(),
  saved_at                    timestamptz NULL,
  UNIQUE (guild_id, id),
  UNIQUE (case_id, message_id),
  FOREIGN KEY (guild_id, case_id) REFERENCES cases (guild_id, id),
  CHECK ((state = 'failed') = (failure IS NOT NULL))
);
CREATE INDEX evidence_case ON evidence (case_id, created_at, id);

-- Files of a saved message, and their copies in the evidence channel.
CREATE TABLE evidence_files (
  id                 uuid PRIMARY KEY DEFAULT uuidv7(),
  guild_id           bigint NOT NULL,
  case_id            uuid NOT NULL,
  evidence_id        uuid NOT NULL,
  position           smallint NOT NULL CHECK (position BETWEEN 1 AND 10),
  filename           text NOT NULL CHECK (char_length(filename) BETWEEN 1 AND 255),
  content_type       text NOT NULL CHECK (char_length(content_type) <= 191),
  size_bytes         bigint NOT NULL CHECK (size_bytes >= 0),
  original_url       text NOT NULL CHECK (char_length(original_url) <= 2048),
  copy_state         file_copy_state NOT NULL DEFAULT 'pending',
  copy_channel_id    bigint NULL,
  copy_message_id    bigint NULL,
  copy_attachment_id bigint NULL,
  created_at         timestamptz NOT NULL DEFAULT now(),
  UNIQUE (evidence_id, position),
  FOREIGN KEY (guild_id, evidence_id) REFERENCES evidence (guild_id, id),
  FOREIGN KEY (guild_id, case_id) REFERENCES cases (guild_id, id),
  CHECK ((copy_state = 'copied') = (copy_message_id IS NOT NULL AND copy_channel_id IS NOT NULL
                                    AND copy_attachment_id IS NOT NULL))
);
CREATE INDEX evidence_files_case ON evidence_files (case_id);

-- One appeal per case.
CREATE TABLE appeals (
  id               uuid PRIMARY KEY DEFAULT uuidv7(),
  guild_id         bigint NOT NULL,
  case_id          uuid NOT NULL,
  member_id        bigint NOT NULL,
  state            appeal_state NOT NULL DEFAULT 'pending',
  version          integer NOT NULL DEFAULT 1 CHECK (version >= 1),
  statement        text NOT NULL CHECK (char_length(statement) BETWEEN 1 AND 4000),
  decided_by       bigint NULL,                                   -- latest staff decision
  decided_at       timestamptz NULL,
  decision_reason  text NULL CHECK (char_length(decision_reason) BETWEEN 1 AND 2000),
  first_decided_at timestamptz NULL,                              -- first accept/reject/close, for metrics
  created_at       timestamptz NOT NULL DEFAULT now(),
  updated_at       timestamptz NOT NULL DEFAULT now(),
  UNIQUE (case_id),
  UNIQUE (guild_id, id),
  FOREIGN KEY (guild_id, case_id) REFERENCES cases (guild_id, id)
);
CREATE INDEX appeals_guild_state   ON appeals (guild_id, state, created_at, id);
CREATE INDEX appeals_guild_created ON appeals (guild_id, created_at DESC, id DESC);

-- Appeal timeline: statement, replies, decisions.
CREATE TABLE appeal_events (
  id         uuid PRIMARY KEY DEFAULT uuidv7(),
  guild_id   bigint NOT NULL,
  appeal_id  uuid NOT NULL,
  kind       appeal_event_kind NOT NULL,
  actor_id   bigint NULL,                                         -- member or staff; NULL = Quack
  text       text NOT NULL CHECK (char_length(text) BETWEEN 1 AND 4000),
  created_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (guild_id, id),
  FOREIGN KEY (guild_id, appeal_id) REFERENCES appeals (guild_id, id)
);
CREATE INDEX appeal_events_appeal ON appeal_events (appeal_id, created_at, id);
```

### 15.5 Moderation log, tracked messages, jobs, idempotency, drafts

```sql
-- Append-only moderation log (§9).
CREATE TABLE mod_log (
  id         uuid PRIMARY KEY DEFAULT uuidv7(),
  guild_id   bigint NOT NULL REFERENCES guilds (guild_id),
  kind       mod_log_kind NOT NULL,
  result     mod_log_result NOT NULL,
  actor_id   bigint NULL,                                         -- NULL = Quack
  source     mod_log_source NOT NULL,
  case_id    uuid NULL,
  member_id  bigint NULL,
  appeal_id  uuid NULL,
  action_id  uuid NULL,
  rule_id    uuid NULL,
  ticket_id  uuid NULL,
  detail     jsonb NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(detail) = 'object'),
  trace_id   text NULL CHECK (trace_id ~ '^[0-9a-f]{32}$'),
  created_at timestamptz NOT NULL DEFAULT now(),
  FOREIGN KEY (guild_id, case_id)   REFERENCES cases (guild_id, id),
  FOREIGN KEY (guild_id, appeal_id) REFERENCES appeals (guild_id, id),
  FOREIGN KEY (guild_id, action_id) REFERENCES actions (guild_id, id),
  FOREIGN KEY (guild_id, rule_id)   REFERENCES rules (guild_id, id),
  FOREIGN KEY (guild_id, ticket_id) REFERENCES tickets (guild_id, id)
);
CREATE INDEX mod_log_guild_time ON mod_log (guild_id, created_at DESC, id DESC);
CREATE INDEX mod_log_kind       ON mod_log (guild_id, kind, created_at DESC, id DESC);
CREATE INDEX mod_log_case       ON mod_log (case_id, created_at, id) WHERE case_id IS NOT NULL;
CREATE INDEX mod_log_member     ON mod_log (guild_id, member_id, created_at DESC, id DESC) WHERE member_id IS NOT NULL;
CREATE INDEX mod_log_actor      ON mod_log (guild_id, actor_id, created_at DESC, id DESC) WHERE actor_id IS NOT NULL;
CREATE INDEX mod_log_appeal     ON mod_log (appeal_id) WHERE appeal_id IS NOT NULL;
CREATE INDEX mod_log_rule       ON mod_log (rule_id, created_at DESC, id DESC) WHERE rule_id IS NOT NULL;
-- plus the triggers in §9.3

-- Tracked Discord messages (§10). owner_id is a case, appeal, or ticket ID by kind.
CREATE TABLE messages (
  id                uuid PRIMARY KEY DEFAULT uuidv7(),
  guild_id          bigint NOT NULL REFERENCES guilds (guild_id),
  kind              message_kind NOT NULL,
  owner_id          uuid NULL,
  channel_id        bigint NOT NULL,
  message_id        bigint NULL,                                  -- NULL until first posted
  private           boolean NOT NULL DEFAULT false,               -- an ephemeral receipt
  interaction_token text NULL,                                    -- secret; never logged
  token_expires_at  timestamptz NULL,
  wanted_revision   integer NOT NULL DEFAULT 1,
  posted_revision   integer NOT NULL DEFAULT 0,
  state             message_state NOT NULL DEFAULT 'active',
  stale_channel_id  bigint NULL,                                  -- previous location after a move
  stale_message_id  bigint NULL,
  last_error        text NULL CHECK (char_length(last_error) <= 200),  -- machine code, e.g. "http_403_50001"
  created_at        timestamptz NOT NULL DEFAULT now(),
  updated_at        timestamptz NOT NULL DEFAULT now(),
  CHECK ((kind IN ('ticket_panel', 'honeypot_warning')) = (owner_id IS NULL)),
  CHECK ((interaction_token IS NULL) = (token_expires_at IS NULL)),
  CHECK (NOT private OR kind = 'case_receipt'),
  CHECK (posted_revision <= wanted_revision),
  CHECK ((stale_channel_id IS NULL) = (stale_message_id IS NULL))
);
CREATE UNIQUE INDEX messages_owner     ON messages (kind, owner_id) WHERE owner_id IS NOT NULL;
CREATE UNIQUE INDEX messages_singleton ON messages (guild_id, kind) WHERE owner_id IS NULL;

-- The job queue (§16).
CREATE TABLE jobs (
  id              uuid PRIMARY KEY DEFAULT uuidv7(),
  kind            job_kind NOT NULL,
  key             text NULL UNIQUE CHECK (char_length(key) <= 200),   -- dedupe key
  guild_id        bigint NULL,                                    -- for logs only; no foreign key
  payload         jsonb NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(payload) = 'object'),
  run_at          timestamptz NOT NULL DEFAULT now(),
  attempts        integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
  max_attempts    integer NOT NULL CHECK (max_attempts >= 1),
  locked_until    timestamptz NULL,
  locked_by       text NULL CHECK (char_length(locked_by) <= 64),
  rerun_requested boolean NOT NULL DEFAULT false,
  last_error      text NULL CHECK (char_length(last_error) <= 500),
  trace_context   text NULL CHECK (char_length(trace_context) <= 256), -- W3C traceparent of the enqueuer
  dead_at         timestamptz NULL,
  created_at      timestamptz NOT NULL DEFAULT now(),
  updated_at      timestamptz NOT NULL DEFAULT now(),
  CHECK ((locked_until IS NULL) = (locked_by IS NULL))
);
CREATE INDEX jobs_ready ON jobs (kind, run_at) WHERE dead_at IS NULL;
CREATE INDEX jobs_dead  ON jobs (dead_at) WHERE dead_at IS NOT NULL;

-- Operation keys (§4.3). `response` is a result reference, e.g. {"case_id": "…"}.
CREATE TABLE operation_keys (
  scope           text NOT NULL CHECK (char_length(scope) BETWEEN 1 AND 300),
  key             text NOT NULL CHECK (char_length(key) BETWEEN 1 AND 200),
  request_hash    bytea NOT NULL CHECK (octet_length(request_hash) = 32),
  response_status smallint NOT NULL,
  response        jsonb NOT NULL,
  created_at      timestamptz NOT NULL DEFAULT now(),
  expires_at      timestamptz NOT NULL,
  PRIMARY KEY (scope, key)
);
CREATE INDEX operation_keys_expiry ON operation_keys (expires_at);

-- Multi-page /case add drafts (§11.6).
CREATE TABLE case_drafts (
  id                 uuid PRIMARY KEY,
  guild_id           bigint NOT NULL REFERENCES guilds (guild_id),
  actor_id           bigint NOT NULL,
  member_id          bigint NOT NULL,
  rule_id            uuid NULL,
  rule_version       integer NULL,
  private            boolean NOT NULL,
  receipt_channel_id bigint NOT NULL,                             -- where the command was used
  source_channel_id  bigint NULL,                                 -- the message command's message
  source_message_id  bigint NULL,
  values             jsonb NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(values) = 'object'),
  page               smallint NOT NULL DEFAULT 1 CHECK (page BETWEEN 1 AND 3),
  expires_at         timestamptz NOT NULL,
  created_at         timestamptz NOT NULL DEFAULT now(),
  updated_at         timestamptz NOT NULL DEFAULT now(),
  FOREIGN KEY (guild_id, rule_id) REFERENCES rules (guild_id, id),
  CHECK ((rule_id IS NULL) = (rule_version IS NULL)),
  CHECK ((source_channel_id IS NULL) = (source_message_id IS NULL))
);
CREATE INDEX case_drafts_expiry ON case_drafts (expires_at);

-- Importer runs (§19).
CREATE TABLE import_runs (
  id                 uuid PRIMARY KEY DEFAULT uuidv7(),
  mode               text NOT NULL CHECK (mode IN ('dry_run', 'apply')),
  source_fingerprint text NOT NULL,                               -- SHA-256 of the source ledger and row counts
  started_at         timestamptz NOT NULL DEFAULT now(),
  finished_at        timestamptz NULL,
  report             jsonb NULL
);
```

### 15.6 Ownership notes

- `messages.owner_id` is polymorphic (case, appeal, or ticket by `kind`)
  and has no foreign key; the sync job treats a missing owner as `gone`.
- `jobs` rows are work, not records: deleted on success, kept as `dead` for
  7 days.
- Nothing references `sessions`, `oauth_states`, `operation_keys`,
  `case_drafts`, or `jobs`; housekeeping deletes their expired rows.
- Rows in `cases`, `rule_versions`, `mod_log`, `appeals`, `appeal_events`,
  `actions`, `action_attempts`, `dms`, and `evidence*` are never deleted.

### 15.7 Module tables

```sql
CREATE TABLE ticket_settings (
  guild_id                  bigint PRIMARY KEY REFERENCES guilds (guild_id),
  enabled                   boolean NOT NULL DEFAULT false,
  disabled_reason           module_disabled_reason NULL,
  entry_channel_id          bigint NULL,
  queue_channel_id          bigint NULL,
  transcript_retention_days smallint NOT NULL DEFAULT 90 CHECK (transcript_retention_days BETWEEN 1 AND 365),
  version                   bigint NOT NULL DEFAULT 1,
  updated_at                timestamptz NOT NULL DEFAULT now(),
  updated_by                bigint NULL,
  CHECK (NOT enabled OR (entry_channel_id IS NOT NULL AND queue_channel_id IS NOT NULL
                         AND entry_channel_id <> queue_channel_id)),
  CHECK (NOT (enabled AND disabled_reason IS NOT NULL))
);

CREATE TABLE tickets (
  id                   uuid PRIMARY KEY DEFAULT uuidv7(),
  guild_id             bigint NOT NULL REFERENCES guilds (guild_id),
  short_id             text NOT NULL CHECK (short_id ~ '^[A-Za-z0-9_-]{10}$'),  -- in the thread name
  member_id            bigint NOT NULL,
  state                ticket_state NOT NULL DEFAULT 'opening',
  entry_channel_id     bigint NOT NULL,                           -- the thread's parent
  thread_id            bigint NULL UNIQUE,
  opened_at            timestamptz NULL,
  close_requested_at   timestamptz NULL,
  closed_at            timestamptz NULL,
  closed_by            bigint NULL,                               -- member or staff; NULL = Quack
  close_reason         ticket_close_reason NULL,
  journal_gaps         boolean NOT NULL DEFAULT false,            -- a journal event was dropped
  locked_at            timestamptz NULL,                          -- close step 1 done
  transcript_saved_at  timestamptz NULL,                          -- close step 2 done
  thread_deleted_at    timestamptz NULL,                          -- close step 5 done
  thread_delete_failed boolean NOT NULL DEFAULT false,
  created_at           timestamptz NOT NULL DEFAULT now(),
  updated_at           timestamptz NOT NULL DEFAULT now(),
  UNIQUE (guild_id, id),
  UNIQUE (guild_id, short_id),
  CHECK ((state = 'closed') = (closed_at IS NOT NULL))
);
CREATE UNIQUE INDEX tickets_one_live ON tickets (guild_id, member_id) WHERE state IN ('opening', 'open', 'closing');
CREATE INDEX tickets_guild_state ON tickets (guild_id, state, created_at, id);

-- Ticket journal: original text of every message seen in an open ticket.
CREATE TABLE ticket_messages (
  guild_id    bigint NOT NULL,
  ticket_id   uuid NOT NULL,
  message_id  bigint NOT NULL,
  author_id   bigint NOT NULL,
  author_name text NOT NULL CHECK (char_length(author_name) BETWEEN 1 AND 100),
  text        text NOT NULL CHECK (char_length(text) <= 4000),
  files       jsonb NOT NULL DEFAULT '[]' CHECK (jsonb_typeof(files) = 'array'),  -- [{name, size}]
  sent_at     timestamptz NOT NULL,
  expires_at  timestamptz NULL,                                   -- set at close
  PRIMARY KEY (ticket_id, message_id),
  FOREIGN KEY (guild_id, ticket_id) REFERENCES tickets (guild_id, id)
);
CREATE INDEX ticket_messages_expiry ON ticket_messages (expires_at) WHERE expires_at IS NOT NULL;

CREATE TABLE ticket_transcripts (
  ticket_id   uuid PRIMARY KEY,
  guild_id    bigint NOT NULL,
  content     text NOT NULL CHECK (octet_length(content) <= 8388608),
  complete    boolean NOT NULL,
  captured_at timestamptz NOT NULL DEFAULT now(),
  expires_at  timestamptz NOT NULL,
  FOREIGN KEY (guild_id, ticket_id) REFERENCES tickets (guild_id, id)
);
CREATE INDEX ticket_transcripts_expiry ON ticket_transcripts (expires_at);

CREATE TABLE activity_log_settings (
  guild_id                bigint PRIMARY KEY REFERENCES guilds (guild_id),
  enabled                 boolean NOT NULL DEFAULT false,
  disabled_reason         module_disabled_reason NULL,
  include_message_content boolean NOT NULL DEFAULT true,
  include_file_names      boolean NOT NULL DEFAULT true,
  include_embed_types     boolean NOT NULL DEFAULT true,
  version                 bigint NOT NULL DEFAULT 1,
  updated_at              timestamptz NOT NULL DEFAULT now(),
  updated_by              bigint NULL,
  CHECK (NOT (enabled AND disabled_reason IS NOT NULL))
);

CREATE TABLE activity_log_routes (
  guild_id   bigint NOT NULL REFERENCES activity_log_settings (guild_id),
  event      activity_event NOT NULL,
  channel_id bigint NOT NULL,
  PRIMARY KEY (guild_id, event)
);
CREATE INDEX activity_log_routes_channel ON activity_log_routes (channel_id);

CREATE TABLE honeypot_settings (
  guild_id        bigint PRIMARY KEY REFERENCES guilds (guild_id),
  enabled         boolean NOT NULL DEFAULT false,
  disabled_reason module_disabled_reason NULL,
  channel_id      bigint NULL,
  rule_id         uuid NULL,
  warning_text    text NULL CHECK (char_length(warning_text) BETWEEN 1 AND 1500),
  version         bigint NOT NULL DEFAULT 1,
  updated_at      timestamptz NOT NULL DEFAULT now(),
  updated_by      bigint NULL,
  FOREIGN KEY (guild_id, rule_id) REFERENCES rules (guild_id, id),
  CHECK (NOT enabled OR (channel_id IS NOT NULL AND rule_id IS NOT NULL)),
  CHECK (NOT (enabled AND disabled_reason IS NOT NULL))
);

-- One row per post in the honeypot channel.
CREATE TABLE honeypot_hits (
  guild_id      bigint NOT NULL REFERENCES guilds (guild_id),
  message_id    bigint NOT NULL,
  channel_id    bigint NOT NULL,
  member_id     bigint NOT NULL,
  state         honeypot_hit_state NOT NULL DEFAULT 'pending',
  case_id       uuid NULL,
  failure       text NULL CHECK (char_length(failure) <= 64),     -- a MemberRefusal code or 'rule_unavailable'
  cleaned_up_at timestamptz NULL,                                 -- the post was deleted
  created_at    timestamptz NOT NULL DEFAULT now(),
  updated_at    timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (guild_id, message_id),
  FOREIGN KEY (guild_id, case_id) REFERENCES cases (guild_id, id),
  CHECK ((state = 'case_created') = (case_id IS NOT NULL)),
  CHECK ((state = 'failed') = (failure IS NOT NULL))
);
CREATE INDEX honeypot_hits_member ON honeypot_hits (guild_id, member_id, state, created_at);
```

Creation order inside the migrations: enums; `guilds`, `case_counters`,
`discord_accounts`, `sessions`, `oauth_states`; `rules`, `rule_versions`,
`guild_settings`; `cases`, `actions`, `action_attempts`, `evidence`,
`evidence_files`, `appeals`, `appeal_events`; the ticket tables; `dms`;
`mod_log`; `messages`, `jobs`, `operation_keys`, `case_drafts`,
`import_runs`; the remaining module tables; triggers.

### 15.8 Hot-path queries

| Query | SQL shape | Index |
| --- | --- | --- |
| Case number | `UPDATE case_counters SET last_number = last_number + 1 WHERE guild_id = $1 RETURNING last_number` | `case_counters_pkey` |
| Escalation count | `SELECT count(*) FROM cases WHERE guild_id = $1 AND member_id = $2 AND rule_id = $3 AND voided_at IS NULL AND source <> 'imported' AND created_at >= $4` | `cases_escalation` |
| Case by number | `… WHERE guild_id = $1 AND number = $2` | `cases_guild_id_number_key` |
| Case list, newest first | `… WHERE guild_id = $1 AND number < $cursor ORDER BY number DESC LIMIT $n` (+ filters) | `cases_guild_id_number_key`; `cases_member` / `cases_rule` / `cases_moderator` with those filters |
| A member's own cases | `… WHERE member_id = $1 AND (created_at, id) < $cursor ORDER BY created_at DESC, id DESC` | `cases_member_all` |
| Undo guard | §5.6 | `actions_member` |
| Actions that need review | `… WHERE guild_id = $1 AND state = 'failed' AND dismissed_at IS NULL ORDER BY updated_at DESC, id DESC` | `actions_review` |
| Job claim | §16.2 | `jobs_ready` |
| Operation key replay | `SELECT … FROM operation_keys WHERE scope = $1 AND key = $2` | `operation_keys_pkey` |
| Session | `SELECT … FROM sessions WHERE id_hash = $1 AND expires_at > now()` | `sessions_pkey` |
| Moderation log page | `… WHERE guild_id = $1 AND (created_at, id) < $cursor ORDER BY created_at DESC, id DESC LIMIT $n` | `mod_log_guild_time` (or the filter's partial index) |
| Case timeline | `… FROM mod_log WHERE case_id = $1 ORDER BY created_at, id` | `mod_log_case` |
| Pending appeals | `… WHERE guild_id = $1 AND state = 'pending' ORDER BY created_at, id` | `appeals_guild_state` |
| Tracked message bump | §10.2 | `messages_owner` / `messages_singleton` |
| Rules cache load | `SELECT r.*, v.definition FROM rules r JOIN rule_versions v ON v.rule_id = r.id AND v.version = r.current_version WHERE r.guild_id = ANY($1) AND r.archived_at IS NULL` | `rules_guild`, `rule_versions_pkey` |
| Live ticket of a member | `… WHERE guild_id = $1 AND member_id = $2 AND state IN ('opening','open','closing')` | `tickets_one_live` |
| Honeypot burst | `… WHERE guild_id = $1 AND member_id = $2 AND state = 'case_created' AND created_at >= now() - interval '30 seconds'` | `honeypot_hits_member` |
| Expiry sweeps | `DELETE … WHERE expires_at < now()` in batches of 1,000 | `*_expiry` |

---

## 16. Job queue

All background work is a row in `jobs`. Jobs are inserted in the same
transaction as the change that needs them, so committed work is never lost
and rolled-back work never runs.

### 16.1 Lifecycle

```
enqueue ──> ready (run_at ≤ now, unlocked) ──claim──> locked (attempts + 1)
                ^                                         │
                │                 Retry(at) / Defer(at)   │  Done ──> deleted (or re-armed when rerun_requested)
                └─────────────────────────────────────────┤
                       lease expired (crash) ─────────────┘  Dead ──> dead_at set, kept 7 days
```

`JobResult` (returned by handlers):

| Result | Effect |
| --- | --- |
| `Done` | Delete the row, unless `rerun_requested` is set: then clear it, unlock, reset `attempts = 0`, `run_at = now()`. |
| `Retry { at, error }` | Unlock, `run_at = at`, `last_error = error`. A handler returns `Retry` only when `attempts < max_attempts`; on the last attempt it settles its domain state and returns `Done` or `Dead`. |
| `Defer { at }` | Like `Retry` but `attempts = attempts - 1` (the attempt did not count). |
| `Dead { error }` | `dead_at = now()`, unlock, `last_error`. Domain state is already terminal. |
| `Reschedule { at }` | Periodic jobs: unlock, `attempts = 0`, `run_at = at`. |

`locked_by` is a fresh token per claim (`<instance id>:<claim uuid>`), and
every settle statement is fenced with `WHERE id = $1 AND locked_by = $2`, so
a worker that lost its lease cannot settle. Handlers settle their domain
state and the job in **one transaction** (`quack_db::jobs::settle(tx,
claim, result)`).

### 16.2 Claim

```sql
WITH picked AS (
  SELECT id, locked_until AS previous_lock
  FROM jobs
  WHERE kind = $1 AND dead_at IS NULL AND run_at <= now()
    AND (locked_until IS NULL OR locked_until < now())
  ORDER BY run_at, id
  LIMIT $2
  FOR UPDATE SKIP LOCKED
)
UPDATE jobs j
SET locked_until = now() + make_interval(secs => $3),
    locked_by    = $4 || ':' || gen_random_uuid()::text,
    attempts     = j.attempts + 1,
    updated_at   = now()
FROM picked
WHERE j.id = picked.id
RETURNING j.id, j.kind, j.key, j.guild_id, j.payload, j.attempts, j.max_attempts,
          j.locked_by, j.trace_context, j.run_at, j.created_at, picked.previous_lock;
```

Uses `jobs_ready`. A non-NULL `previous_lock` means the previous attempt's
lease expired: `quack.job.lease_expired{kind}` is incremented and the handler
resolves the unknown attempt per §16.5.

### 16.3 Enqueue and coalescing

```sql
INSERT INTO jobs (kind, key, guild_id, payload, run_at, max_attempts, trace_context)
VALUES ($1, $2, $3, $4, $5, $6, $7)
ON CONFLICT (key) DO UPDATE SET
  run_at          = LEAST(jobs.run_at, EXCLUDED.run_at),
  payload         = EXCLUDED.payload,
  rerun_requested = jobs.rerun_requested OR jobs.locked_until IS NOT NULL,
  attempts        = CASE WHEN jobs.dead_at IS NOT NULL THEN 0 ELSE jobs.attempts END,
  dead_at         = NULL,
  updated_at      = now();
SELECT pg_notify('quack_jobs', $1::text);
```

A keyed job therefore exists at most once: enqueueing it again while it
waits moves it earlier; while it runs, marks it to run once more after it
finishes; when it is dead, revives it with a fresh budget. The notify is
delivered on commit. Metric `quack.job.enqueued{kind}`.

### 16.4 Dispatch and wake-up

- One dispatcher task per kind. It wakes on `NOTIFY quack_jobs` for its
  kind, on a 2-second poll, or at the earliest known future `run_at`
  (whichever is first), then claims up to `min(free permits, 10)` jobs.
- Permits: a per-kind limit (§16.5) and a global limit
  `QUACK_WORKER_CONCURRENCY` (default 16) shared by all kinds.
- One dedicated `PgListener` connection; after it reconnects, every
  dispatcher claims immediately (NOTIFY messages may have been missed).
- Each handler runs inside a span `job {kind}` linked to `trace_context`
  (§18.4) and a `tokio::time::timeout` of the lease minus 5 seconds. A
  timeout drops the future without settling; the lease expires and the next
  claim resolves the attempt.
- Every dispatcher updates a heartbeat (an atomic timestamp) on each loop;
  readiness requires all heartbeats within 15 seconds.
- Ordering is FIFO by `run_at` per kind; actions are additionally
  serialized per member (§5.4).

### 16.5 Job kinds

| Kind | Payload | Key | Per-kind limit | Lease | Backoff (after attempt n) | Max attempts | Why it is idempotent | Attempt started, outcome unknown → |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `run_action` | `{action_id}` | `action:<id>` | 8 | 60 s | 5 s, 15 s, 1 min, 5 min, 15 min, 1 h, 3 h | 8 | Fixed timeout end; ban, unban, kick, remove timeout converge (§5.2); state checked first. | Retry (`interrupted`). |
| `send_dm` | `{dm_id}` | `dm:<id>` | 4 | 60 s | 5 s, 30 s (only before `sending`) | 3 | `sending` is recorded before the send and is never sent again; the nonce dedupes the in-job repeat. | DM becomes `unknown`; never resent. |
| `sync_message` | `{message_row_id}` | `message:<id>` | 4 | 30 s | 2 s, 10 s, 1 min, 5 min, 30 min | 6 | Renders current state; edits converge; creates use a per-revision nonce. | Retry (an edit repeats harmlessly; a create repeats with the same nonce). |
| `post_mod_log` | `{entry_id}` | `mod_log_post:<id>` | 2 | 30 s | 10 s, 30 s, 1 min, 2 min | 5 | One message per entry, nonce = entry ID; retries stay inside Discord's nonce window. | Retry with the same nonce; exhausted → `log_channel_failed`. |
| `capture_evidence` | `{evidence_id}` | `evidence:<id>` | 4 | 120 s | 10 s, 1 min, 5 min | 4 | Fetch is a read; each file is marked copied only after Discord confirms; uploads use the file ID as nonce. | Retry. |
| `honeypot_case` | `{guild_id, member_id}` | `honeypot_case:<guild>:<member>` | 2 | 60 s | 5 s, 30 s, 2 min, 10 min | 5 | Case creation uses a deterministic operation key; hit updates are guarded on `pending`. | Retry. |
| `honeypot_cleanup` | `{guild_id, channel_id, message_id}` | `honeypot_cleanup:<message_id>` | 2 | 30 s | 1 s, 2 s, 4 s, 8 s, 16 s, 32 s, 64 s, 64 s, 64 s | 10 | Delete is idempotent (404 = done). | Retry. |
| `open_ticket` | `{ticket_id}` | `ticket_open:<id>` | 2 | 60 s | 2 s, 10 s, 30 s | 4 | Adopts an existing thread named `ticket-<short_id>`; each step checks saved progress. | Retry. |
| `close_ticket` | `{ticket_id}` | `ticket_close:<id>` | 2 | 300 s | 5 s, 30 s, 2 min, 10 min, 1 h | 6 | Each step records its completion column. | Retry; exhausted → closed with what is saved (§13.2.4). |
| `housekeeping` | `{}` | `housekeeping` | 1 | 300 s | `Reschedule` every 10 min | 1 | Deletes of expired rows. | Runs again next time. |

### 16.6 Periodic work

- `housekeeping` is ensured at startup (`INSERT … ON CONFLICT (key) DO
  NOTHING`) and reschedules itself every 10 minutes. Each run deletes, in
  batches of 1,000: expired `sessions`, `oauth_states`, `operation_keys`,
  `case_drafts`; `jobs` with `dead_at` older than 7 days; `ticket_transcripts`
  and `ticket_messages` past `expires_at`. It also marks private receipts
  whose token expired as `gone` and clears expired `interaction_token`
  values.
- DB-backed gauges (§18.2) are observed by a separate 60-second task, not a
  job.

### 16.7 Shutdown

On shutdown (§17.5) dispatchers stop claiming at once. In-flight handlers
get up to 20 seconds to finish; after that their futures are dropped
without settling, their leases expire, and the next process resolves them by
the per-kind rule above. Nothing is lost because every state change was
committed before the Discord call (§5.4, §6.4).

---

## 17. Architecture

### 17.1 One binary

One binary, `quack`, runs as one process per Discord application. All
durable state is in PostgreSQL, so a restart loses nothing.

| Command | What it does | Needs |
| --- | --- | --- |
| `quack serve [--config <file>] [--migrate]` | Gateway, interactions, HTTP API, dashboard files, job workers. With `--migrate`, applies pending migrations first; otherwise refuses to start when migrations are not current. | all serve keys |
| `quack migrate [--config <file>]` | Applies pending migrations, prints the applied list. | `QUACK_DATABASE_URL` |
| `quack import-mysql --source <mysql-url> [--dry-run] [--report <file>] [--config <file>]` | One-time import (§19). | `QUACK_DATABASE_URL` |
| `quack commands sync [--config <file>]` | Registers slash commands (§11.2). | Discord token and application ID |

Exit codes: 0 success, 1 runtime failure, 2 usage or configuration error
(all problems printed at once).

### 17.2 Crates

Workspace in `crates/`, edition 2024, resolver 3, Rust 1.100, pinned
dependencies from the stack audit plus the additions in §20.5.

| Crate | Owns | Depends on | Public surface |
| --- | --- | --- | --- |
| `quack-core` | Pure domain. No I/O, no async, no SQL, no Discord types. | — | `ids` (`GuildId`, `AccountId`, `MemberId`, `ChannelId`, `RoleId`, `MessageId` snowflake newtypes; `CaseId`, `RuleId`, `ActionId`, `AppealId`, `AppealEventId`, `DmId`, `EvidenceId`, `FileId`, `TicketId`, `MessageRowId`, `JobId` UUID newtypes; `to_u22`/`from_u22`; `ulid_to_uuid`); `rule` (`RuleDefinition`, `Field`, `FieldKind`, `Level`, `LevelAction`, `Strictness`, `RuleProblem`); `escalation` (`select_level`, `LevelChoice`, `counting_cutoff`); `permissions` (`Permissions` bits, `Capability`, `Capabilities`, `ActorFacts`, `derive_capabilities`, `ApplyFacts`, `MemberRefusal`, `check_member`, `check_action_permission`, `check_staff_channel`, `ChannelProblem`, `is_staff_for_access`); `case` (`CaseSource`, `VoidCause`, `CaseDecision`, `ContextValues`, `ContextProblem`, `validate_context`, `MessageLink::parse`); `action` (`ActionKind`, `ActionState`, `ActionFailure`, `AttemptOutcome`, `ActionEvent`, `transition`, `RETRY_BACKOFF`, `undo_kind`); `dm` (`DmKind`, `DmState`, `DmFailure`, `transition`); `evidence` (`EvidenceState`, `EvidenceFailure`, `FileCopyState`, `copy_eligibility`); `appeal` (`AppealState`, `AppealEventKind`, `AppealDecision`, `transition`, `eligibility`); `modlog` (`ModLogKind`, `ModLogResult`, `ModLogSource`, `ModLogEntry`, `is_posted`, `redact`); `modules` (`ModuleName`, `ModuleProblem`, `ActivityEvent`, `TicketState`, `HoneypotHitState`); `limits`; `DomainError`. |
| `quack-text` | Every user-facing string (§12) and the neutral message model. | core | `Msg`, `Block`, `Row`, `Button`, `ButtonAction`, `Select`, `Tone`, `Surface` (`Discord` renders mentions, `Plain` renders names); one function per message id in modules `command`, `error`, `case`, `action`, `dm`, `evidence`, `appeal`, `log`, `setup`, `help`, `rules`, `ticket`, `activity`, `honeypot`, `api`; `fmt` (`duration`, `plural`, `escape_markdown`, `timestamp`). |
| `quack-telemetry` | OTel setup and the instrument catalog. | — | `init(TelemetryConfig) -> Result<TelemetryGuard>`, `TelemetryGuard::shutdown`, `Metrics` (every instrument in §18.2 as a typed field) and `metrics()`, attribute enums, `trace::{current_traceparent, link_from_traceparent}`, span constructors (`span::interaction`, `span::gateway`, `span::job`, `span::db`, `span::discord`). |
| `quack-db` | PostgreSQL: migrations, every query (sqlx macros, offline data in `.sqlx/`), the job queue, sessions, tracked messages. | core, telemetry | `Db` (pool + `begin`), `migrate`, `verify_migrations`; query modules `guilds`, `settings`, `rules`, `cases`, `actions`, `dms`, `evidence`, `appeals`, `modlog`, `messages`, `operation_keys`, `drafts`, `sessions`, `accounts`, `stats`, `tickets`, `activity_log`, `honeypot`, `import`; `jobs` (`NewJob`, `enqueue`, `claim`, `settle`, `ClaimedJob`, `JobResult`, `Listener`); `notify` (`quack_rules`, `quack_settings`). |
| `quack-discord` | Twilight 0.18: gateway, caches, REST with typed outcomes, rendering, custom IDs, command definitions. | core, text, telemetry | `DiscordClient` (implements `quack_engine::Discord` through a thin adapter in the binary — see note), `Gateway` (`run(tx)` yielding `GatewayEvent`), `GuildCache`, `MemberCache`, `render(&Msg) -> RenderedMessage`, `custom_id::{CustomId, encode, decode, decode_legacy}`, `commands::{definitions, sync}`, `Responder` (ack, defer, modal, edit original, follow-up), `permissions::{in_channel, in_guild}`. |
| `quack-engine` | Use cases and job handlers; defines the `Discord` port and the `Module` contract. | core, text, db, telemetry | `Engine`, `Ctx`, `Principal`, `Discord` (trait), `Outcome`, `Refusal`, `ApplyFacts` loader; `cases::{apply_rule, void_case, edit_context, add_evidence, get, list, member_summary}`; `actions::{run, retry, dismiss, undo}`; `dms::send`; `evidence::{capture, resolve_file}`; `appeals::{submit, add_information, decide, get, list}`; `messages::{bump, sync}`; `modlog::{list, post}`; `rules::{create, edit, archive, restore, import, export, get, list}`; `settings::{get, patch, acknowledge_starter_notice}`; `setup::{create_channel, panel, apply}`; `guilds::{bootstrap, left, reconcile_channels}`; `stats::compute`; `drafts`; `jobs::Runner`; `Module`; `RulesCache`, `ConfigCache`. |
| `quack-tickets`, `quack-activity-log`, `quack-honeypot` | The optional modules (§13). | core, text, db, engine, telemetry | `module() -> Arc<dyn Module>`; service functions and DTOs used by `quack-api` (`settings`, `update_settings`, `list`, `get`, `transcript`, `close` for tickets). |
| `quack-api` | axum HTTP API, OAuth, sessions, CSRF, rate limits, OpenAPI, static files. | core, text, db, engine, modules, telemetry | `router(ApiState) -> axum::Router`, `ApiState`, `openapi::document()`, extractors (`SessionAccount`, `Staff<Capability>`, `MemberSelf`, `IdempotencyKey`), `ApiError`. |
| `quack-bot` | Interaction and gateway routing into engine calls and replies. | core, text, engine, discord, modules, telemetry | `Bot::new`, `Bot::on_interaction`, `Bot::on_event`. |
| `quack-import` | MySQL → PostgreSQL importer (only crate with sqlx `mysql`). | core, db | `run(ImportOptions) -> Result<Report>`, `Report`. |
| `quack-testkit` | Test helpers (dev-dependency only). | core, engine, db | `FakeDiscord`, `TestDb` (Postgres 18 in Testcontainers, migrated), `fixtures`. |
| `quack` (bin) | Config, CLI, wiring, shutdown. | everything | `main`. |

Note: `quack-discord` may not depend on `quack-engine` (adapters never
import each other's implementation, and the engine must not see Twilight).
The binary therefore contains `DiscordPort`, a ~200-line adapter that
implements `quack_engine::Discord` by calling `quack_discord::DiscordClient`
and the caches, mapping types both ways.

Rules: dependencies point toward `quack-core`; adapters never import each
other; `quack-engine` uses Postgres concretely (tests run against real
Postgres) and reaches Discord only through `Discord`. No other traits exist
unless they have two real implementations (`Module` has three).

### 17.3 The `Discord` port

```rust
/// Everything the engine and modules need from Discord. Implemented by the
/// binary's `DiscordPort` (over quack-discord) and by quack-testkit's `FakeDiscord`.
pub trait Discord: Send + Sync + 'static {
    // Facts (gateway cache first, REST fallback; never cached as "absent").
    fn guild_facts(&self, guild: GuildId) -> BoxFuture<'_, Outcome<GuildFacts>>;
    fn member(&self, guild: GuildId, member: MemberId) -> BoxFuture<'_, Outcome<Option<MemberFacts>>>;
    fn channel_facts(&self, guild: GuildId, channel: ChannelId) -> BoxFuture<'_, Outcome<Option<ChannelFacts>>>;

    // Actions (§5.1). `reason` is the X-Audit-Log-Reason value.
    fn timeout(&self, guild: GuildId, member: MemberId, until: OffsetDateTime, reason: &str) -> BoxFuture<'_, Outcome<()>>;
    fn remove_timeout(&self, guild: GuildId, member: MemberId, reason: &str) -> BoxFuture<'_, Outcome<()>>;
    fn kick(&self, guild: GuildId, member: MemberId, reason: &str) -> BoxFuture<'_, Outcome<()>>;
    fn ban(&self, guild: GuildId, member: MemberId, delete_message_seconds: u32, reason: &str) -> BoxFuture<'_, Outcome<()>>;
    fn unban(&self, guild: GuildId, member: MemberId, reason: &str) -> BoxFuture<'_, Outcome<()>>;

    // Messages.
    fn open_dm(&self, account: AccountId) -> BoxFuture<'_, Outcome<ChannelId>>;
    fn send(&self, channel: ChannelId, msg: &Msg, files: &[OutgoingFile], nonce: &Nonce) -> BoxFuture<'_, Outcome<MessageId>>;
    fn edit(&self, channel: ChannelId, message: MessageId, msg: &Msg, files: &[OutgoingFile]) -> BoxFuture<'_, Outcome<()>>;
    fn edit_interaction_original(&self, token: &InteractionToken, msg: &Msg) -> BoxFuture<'_, Outcome<(ChannelId, MessageId)>>;
    fn delete_message(&self, channel: ChannelId, message: MessageId) -> BoxFuture<'_, Outcome<()>>;
    fn fetch_message(&self, channel: ChannelId, message: MessageId) -> BoxFuture<'_, Outcome<FetchedMessage>>;
    fn fetch_history(&self, channel: ChannelId, before: Option<MessageId>, limit: u8) -> BoxFuture<'_, Outcome<Vec<FetchedMessage>>>;

    // Files.
    fn download_attachment(&self, url: &Url, max_bytes: u64) -> BoxFuture<'_, Outcome<Bytes>>;
    fn refresh_attachment_url(&self, url: &Url) -> BoxFuture<'_, Outcome<Url>>;

    // Channels and threads.
    fn create_channel(&self, guild: GuildId, spec: &ChannelSpec) -> BoxFuture<'_, Outcome<ChannelId>>;
    fn create_private_thread(&self, parent: ChannelId, name: &str) -> BoxFuture<'_, Outcome<ChannelId>>;
    fn find_active_thread(&self, guild: GuildId, parent: ChannelId, name: &str) -> BoxFuture<'_, Outcome<Option<ChannelId>>>;
    fn add_thread_member(&self, thread: ChannelId, member: MemberId) -> BoxFuture<'_, Outcome<()>>;
    fn remove_thread_member(&self, thread: ChannelId, member: MemberId) -> BoxFuture<'_, Outcome<()>>;
    fn thread_members(&self, thread: ChannelId) -> BoxFuture<'_, Outcome<Vec<MemberId>>>;
    fn lock_thread(&self, thread: ChannelId) -> BoxFuture<'_, Outcome<()>>;
    fn delete_channel(&self, channel: ChannelId) -> BoxFuture<'_, Outcome<()>>;
}

/// The typed result of one Discord call.
pub enum Outcome<T> {
    /// Discord confirmed it.
    Done(T),
    /// Discord definitely refused; nothing changed.
    Refused(Refusal),
    /// Rate limited before anything happened; try again after the delay.
    RetryAfter(Duration),
    /// The call may or may not have taken effect (timeout, 5xx, connection reset).
    Unknown(UnknownCause),
}

pub struct Refusal { pub status: u16, pub code: Option<u32>, pub kind: RefusalKind }

pub enum RefusalKind {
    MissingPermissions,   // 403, 50013
    MissingAccess,        // 403, 50001
    CannotDm,             // 403, 50007
    NoMutualGuilds,       // 50278
    UnknownGuild,         // 404, 10004
    UnknownMember,        // 404, 10007
    UnknownUser,          // 404, 10013
    UnknownBan,           // 404, 10026
    UnknownMessage,       // 404, 10008
    UnknownChannel,       // 404, 10003
    TooLarge,             // 413, 40005
    Other,                // any other 4xx except 429
}

pub enum UnknownCause { Timeout, ServerError(u16), Connection }
```

Every REST call has a 10-second deadline (30 seconds for file downloads and
uploads), runs inside a `discord {method} {route}` span, and records
`quack.discord.*` metrics (§18.2). Twilight's rate limiter is used; when it
reports a wait longer than 10 seconds the call returns `RetryAfter`.
Twilight's own retries are off; the per-kind policies above decide.

`GuildFacts` (owner, MFA required, roles with positions and permissions,
Quack's member and computed permissions), `MemberFacts` (roles, highest
position, Administrator flag, bot flag, timeout end, display names),
`ChannelFacts` (type, parent, overwrites, Quack's computed permissions),
and `FetchedMessage` (author, content, timestamps, embeds, attachments,
server) are plain structs in `quack-engine`.

### 17.4 Dependency diagram

```
                       ┌──────────────┐
                       │  quack (bin) │  config, CLI, DiscordPort, wiring
                       └──────┬───────┘
          ┌──────────────┬────┴─────────┬───────────────┬──────────────┐
          v              v              v               v              v
     quack-bot       quack-api    quack-import     quack-discord   (modules)
          │              │              │               │          tickets
          │              │              │               │          activity-log
          │              │              │               │          honeypot
          └───────┬──────┴──────┬───────┘               │              │
                  v             v                       │              │
             quack-engine <─────┴───────────────────────┼──────────────┘
                  │                                     │
          ┌───────┼─────────────┐                       │
          v       v             v                       v
      quack-db  quack-text  quack-telemetry        quack-text, quack-telemetry
          │       │
          └───┬───┘
              v
         quack-core
```

`quack-bot` also depends on `quack-discord` (to reply to interactions) and
on the module crates (to route their components).

### 17.5 Startup and shutdown

**Startup (`quack serve`):**

1. Parse CLI; load the TOML file (if given) and `QUACK_*` variables;
   validate every key, reporting all problems; exit 2 on any.
2. Initialize telemetry (§18.6). From here on, everything is traced.
3. Connect the pool (`QUACK_DATABASE_MAX_CONNECTIONS`, acquire timeout 5 s,
   `statement_timeout = 5s` per connection). With `--migrate`, apply
   migrations; then verify they are current (else exit 1).
4. Build the Discord REST client; `GET /users/@me` to validate the token
   and learn Quack's own account ID.
5. Sync commands (§11.2).
6. Start the job runner: `LISTEN quack_jobs`, ensure the `housekeeping`
   job, start one dispatcher per kind. Start the 60-second gauge task.
7. Start the cache listener (`LISTEN quack_rules, quack_settings`).
8. Connect the gateway shard; start the event dispatch queues.
9. Bind `QUACK_LISTEN_ADDR` and serve HTTP.
10. `/readyz` turns ready when the gateway reports `READY` and every
    dispatcher has ticked.

**Server bootstrap** (`guilds::bootstrap`, on `GUILD_CREATE`, one
transaction):

1. Upsert `guilds` (name, icon, owner, MFA, `installed = true`,
   `left_at = NULL`).
2. First time only: create `case_counters`, `guild_settings`, the module
   settings rows (all off), the starter rule (§3.7) with version 1, set
   `starter_rule_id`; log `quack_joined` (`first_join`).
3. Returning after leaving: log `quack_joined` (`first_join = false`).
4. Reconcile channel settings against the event's channel list (§13.1.5).

**Shutdown** (SIGTERM or SIGINT; total budget 30 seconds; set the platform's
grace period above 35 seconds):

1. Mark not ready (`/readyz` → 503).
2. Stop job dispatchers from claiming; stop accepting new interactions
   (new ones get no ack and Discord shows them as failed).
3. Start HTTP graceful shutdown (in-flight requests finish, up to 10 s).
4. Close the gateway shard with a normal close frame.
5. Flush the server activity log's pending delete batches and drain its
   queue (up to 5 s).
6. Wait for in-flight job handlers (up to 20 s, from step 2).
7. Shut down telemetry (flush spans, metrics, logs; up to 5 s).
8. Close the pool. Exit 0.

### 17.6 Caches and invalidation

- **Rules cache**: loaded for all installed servers at startup in one
  query, then per server on `NOTIFY quack_rules '<guild_id>'` (sent in every
  transaction that creates, edits, archives, restores, or imports a rule).
  Used by autocomplete, pickers, `/case add` modals, `/setup` rule select.
- **Config cache**: `guild_settings` plus the three module settings rows
  per server; reloaded on `NOTIFY quack_settings '<guild_id>'`. Used by
  autocomplete capability checks, setup modals, honeypot filtering, activity
  routing.
- After the listener reconnects, both caches reload fully.
- Caches never grant access by themselves for writes: every write re-checks
  capabilities with fresh Discord facts and reads rows inside its
  transaction.

### 17.7 Configuration

Environment variables (`QUACK_*`) or an optional TOML file
(`--config <file>`) using the same names in lowercase without the prefix
(`database_url = "…"`). Environment variables win over the file. Unknown
keys in the file are an error. Secrets belong in the environment.

| Variable | Type | Default | Purpose |
| --- | --- | --- | --- |
| `QUACK_DATABASE_URL` | Postgres URL | required | Connection string. |
| `QUACK_DATABASE_MAX_CONNECTIONS` | integer 2–200 | 20 | Pool size (one extra connection is used by the listener and one per running action for its advisory lock, within this pool). |
| `QUACK_DISCORD_TOKEN` | secret | required for `serve`, `commands sync` | Bot token. |
| `QUACK_DISCORD_APPLICATION_ID` | snowflake | required for `serve`, `commands sync` | Application ID (also the OAuth client ID). |
| `QUACK_DISCORD_CLIENT_SECRET` | secret | required for `serve` | OAuth client secret. |
| `QUACK_PUBLIC_URL` | URL, `http` or `https`, no path | required for `serve` | Dashboard links, OAuth redirect (`<url>/api/auth/callback`), CSRF origin, cookie `Secure`. |
| `QUACK_LISTEN_ADDR` | socket address | `0.0.0.0:8080` | HTTP listen address. |
| `QUACK_SESSION_SECRET` | base64, exactly 32 bytes | required for `serve` | Encrypts Discord tokens stored in sessions. Rotating it signs everyone out. |
| `QUACK_DASHBOARD_DIR` | path | `dashboard` | Directory with the built dashboard (`index.html`, `assets/`). |
| `QUACK_OTLP_ENDPOINT` | URL | unset | OTLP/HTTP base URL (`/v1/traces`, `/v1/metrics`, `/v1/logs` are appended). Unset: no export; logs still go to stdout. |
| `QUACK_SERVICE_NAME` | string | `quack` | `service.name`. |
| `QUACK_ENVIRONMENT` | string | `development` | `deployment.environment.name` resource attribute. |
| `QUACK_LOG_LEVEL` | `tracing` filter directive | `info` | Log and span level filter. |
| `QUACK_WORKER_CONCURRENCY` | integer 1–64 | 16 | Jobs running at once across all kinds. |
| `QUACK_DEV_GUILD_ID` | snowflake | unset | Register commands to this server only. |
| `QUACK_TRUSTED_PROXY_HOPS` | integer 0–5 | 0 | Proxies in front of Quack, for the client IP (§14.7). |

Everything else in this document is a constant in code.

### 17.8 Engineering rules

- `[workspace.lints]`: `unsafe_code = "forbid"`; clippy `all` and
  `pedantic` warned (CI uses `-D warnings`), with this allow list:
  `module_name_repetitions` (glossary names repeat by design),
  `missing_errors_doc` (every error is a typed enum documented on the
  type), `must_use_candidate` (noise on builders). Denied outside tests:
  `unwrap_used`, `expect_used`, `panic`, `dbg_macro`, `todo`,
  `unimplemented`. Startup code returns `anyhow::Result` instead of
  panicking.
- Errors: `thiserror` enums in libraries; `anyhow` only in the binary and
  tests. Every user-visible failure is a typed variant mapped to a §12
  message id.
- Every public item has a doc comment saying what it is for; every crate a
  crate-level comment. Modules over ~400 lines need a reason.
- `cargo fmt --check`, `cargo clippy --workspace --all-targets --locked
  -- -D warnings`, `cargo nextest run --workspace --locked`, and `cargo
  test --doc --workspace` pass before every commit.

---

## 18. Telemetry

Traces, metrics, and logs go over OTLP/HTTP (protobuf) using OpenTelemetry
0.33.1, `tracing` 0.1.44, and `tracing-opentelemetry` 0.34.0. Logs go
through `tracing` and carry the current trace and span IDs.

### 18.1 Rules

- **Cardinality.** Metric attributes are small enums only. Server, account,
  channel, case, appeal, action, message, and job IDs never appear on
  metrics; they go on spans and logs.
- **Catalog.** Every instrument is a field of `quack_telemetry::Metrics`,
  created once at startup with the name, unit, description, and buckets
  below; attribute values are Rust enums in `quack_telemetry::attr`, so names
  and values cannot drift.
- **Failure isolation.** A slow or absent collector never blocks moderation:
  exporters use bounded queues and drop on overflow.

### 18.2 Instruments

Types: C = counter, UDC = up-down counter, H = histogram, G = observable
gauge. Units: `s` seconds, `By` bytes, `{x}` a count of x. "DB task" = the
60-second gauge task (§16.6).

**Interactions**

| Name | Type | Unit | Attributes (allowed values) | Description | Recorded in |
| --- | --- | --- | --- | --- | --- |
| `quack.interaction.count` | C | `{interaction}` | `kind` (command, component, modal, autocomplete); `command` (case_add, case_view, case_list, case_void, case_history, appeals, rules, setup, help, add_case_message, add_case_member, or a custom-ID code from §11.5, `legacy`, `unknown`); `outcome` (ok, refused, error) | Interactions handled. | quack-bot router, after the reply |
| `quack.interaction.ack.duration` | H | `s` | `kind` | Receive to acknowledgement. Buckets B_FAST. | quack-bot, on ack |
| `quack.interaction.duration` | H | `s` | `kind`, `command` | Receive to final reply. Buckets B_REQ. | quack-bot |
| `quack.interaction.errors` | C | `{error}` | `kind`; `reason` (missing_capability, mfa_required, member_refused, not_found, conflict, validation, discord_unavailable, expired, legacy, ack_late, internal) | Interactions answered with an error. | quack-bot |

**Cases**

| Name | Type | Unit | Attributes | Description | Recorded in |
| --- | --- | --- | --- | --- | --- |
| `quack.case.created` | C | `{case}` | `source` (discord, dashboard, honeypot); `action_kind` (none, timeout, kick, ban) | Cases created. | engine `apply_rule`, after commit |
| `quack.case.create.duration` | H | `s` | `source` | `apply_rule` from start to commit. Buckets B_REQ. | engine |
| `quack.case.create.refused` | C | `{refusal}` | `reason` (MemberRefusal values, missing_capability, mfa_required, rule_archived, validation) | Case creations refused. | engine |
| `quack.case.level` | H | `{level}` | `source` | Chosen level index. Buckets B_LEVEL. | engine |
| `quack.case.voided` | C | `{case}` | `cause` (staff, appeal_accepted) | Cases voided. | engine `void_case` |

**Actions**

| Name | Type | Unit | Attributes | Description | Recorded in |
| --- | --- | --- | --- | --- | --- |
| `quack.action.attempts` | C | `{attempt}` | `kind` (timeout, kick, ban, remove_timeout, unban); `outcome` (done, no_change, refused, rate_limited, unknown, interrupted); `reason` (ActionFailure values, none) | Attempts settled. | engine `actions::run` |
| `quack.action.finished` | C | `{action}` | `kind`; `state` (done, failed, cancelled, skipped) | Actions reaching a terminal state. | engine |
| `quack.action.time_to_done` | H | `s` | `kind` | Case creation (or undo request) to `done`. Buckets B_LONG. | engine |
| `quack.action.retries` | C | `{retry}` | `kind` | Automatic retries scheduled. | engine |
| `quack.action.undo.skipped` | C | `{action}` | `kind` (remove_timeout, unban) | Undo actions left in place by the guard. | engine |
| `quack.action.failed.current` | G | `{action}` | `kind` | Actions in `failed`, not dismissed. | DB task |

**DMs**

| Name | Type | Unit | Attributes | Description | Recorded in |
| --- | --- | --- | --- | --- | --- |
| `quack.dm.outcomes` | C | `{dm}` | `kind` (case, appeal_update, ticket_closed); `state` (sent, failed, unknown, not_needed); `reason` (DmFailure values, none) | DMs settled. | engine `dms::send`, void |
| `quack.dm.send.duration` | H | `s` | `kind` | `sending` to settled. Buckets B_REQ. | engine |

**Appeals**

| Name | Type | Unit | Attributes | Description | Recorded in |
| --- | --- | --- | --- | --- | --- |
| `quack.appeal.submitted` | C | `{appeal}` | — | Appeals submitted. | engine |
| `quack.appeal.transitions` | C | `{transition}` | `decision` (request_information, information_provided, accept, reject, close, reopen, auto_close) | Appeal state changes. | engine |
| `quack.appeal.time_to_decision` | H | `s` | `decision` (accept, reject, close) | Submission to the first accept, reject, or close. Buckets B_DECISION. | engine |
| `quack.appeal.open` | G | `{appeal}` | `state` (pending, needs_information) | Open appeals. | DB task |

**Evidence**

| Name | Type | Unit | Attributes | Description | Recorded in |
| --- | --- | --- | --- | --- | --- |
| `quack.evidence.captures` | C | `{item}` | `outcome` (saved, failed); `reason` (EvidenceFailure values, none) | Evidence items settled. | engine `evidence::capture` |
| `quack.evidence.files_copied` | C | `{file}` | `outcome` (FileCopyState values except pending) | Files processed. | engine |
| `quack.evidence.bytes_copied` | C | `By` | — | Bytes uploaded to evidence channels. | engine |

**Moderation log and tracked messages**

| Name | Type | Unit | Attributes | Description | Recorded in |
| --- | --- | --- | --- | --- | --- |
| `quack.mod_log.entries` | C | `{entry}` | `kind` (ModLogKind values); `result` (ok, failed, refused) | Entries written. | quack-db `modlog::insert` |
| `quack.mod_log.posts` | C | `{post}` | `outcome` (posted, no_channel, channel_missing, not_staff_only, refused, retry, exhausted) | Log channel post attempts. | engine `modlog::post` |
| `quack.message.syncs` | C | `{sync}` | `kind` (MessageKind values); `outcome` (edited, posted, reposted, gone, retry, failed) | Tracked message syncs. | engine `messages::sync` |
| `quack.message.sync.lag` | H | `s` | `kind` | Bump to posted. Buckets B_LONG. | engine |

**Jobs**

| Name | Type | Unit | Attributes | Description | Recorded in |
| --- | --- | --- | --- | --- | --- |
| `quack.job.enqueued` | C | `{job}` | `kind` (JobKind values) | Jobs inserted or coalesced. | quack-db `jobs::enqueue` |
| `quack.job.started` | C | `{job}` | `kind` | Jobs claimed. | job runner |
| `quack.job.finished` | C | `{job}` | `kind`; `outcome` (done, retry, deferred, dead, rescheduled, timed_out) | Handler results. | job runner |
| `quack.job.duration` | H | `s` | `kind` | Handler run time. Buckets B_REQ. | job runner |
| `quack.job.queue_wait` | H | `s` | `kind` | `run_at` to claim. Buckets B_LONG. | job runner |
| `quack.job.lease_expired` | C | `{job}` | `kind` | Claims that found an expired lease. | job runner |
| `quack.job.ready` | G | `{job}` | `kind` | Ready jobs (`run_at ≤ now`, unlocked, not dead). | DB task |
| `quack.job.oldest_ready_age` | G | `s` | `kind` | Age of the oldest ready job. | DB task |
| `quack.job.dead` | G | `{job}` | `kind` | Dead jobs kept for inspection. | DB task |

**Discord REST**

| Name | Type | Unit | Attributes | Description | Recorded in |
| --- | --- | --- | --- | --- | --- |
| `quack.discord.requests` | C | `{request}` | `route` (Twilight route pattern, e.g. `PATCH /guilds/{guild_id}/members/{user_id}`); `method`; `status_class` (2xx, 3xx, 4xx, 5xx, none); `outcome` (done, refused, retry_after, unknown) | REST calls. | quack-discord client wrapper |
| `quack.discord.request.duration` | H | `s` | `route`, `method` | Call duration including rate-limit wait. Buckets B_REQ. | quack-discord |
| `quack.discord.rate_limit.hits` | C | `{hit}` | `route`; `scope` (route, global, shared) | 429 responses and limiter waits. | quack-discord |
| `quack.discord.rate_limit.wait` | H | `s` | `route` | Time spent waiting on the limiter. Buckets B_REQ. | quack-discord |

**Gateway and caches**

| Name | Type | Unit | Attributes | Description | Recorded in |
| --- | --- | --- | --- | --- | --- |
| `quack.gateway.events` | C | `{event}` | `event` (the §11.9 event names, `other`) | Dispatch events received. | quack-discord gateway loop |
| `quack.gateway.event.duration` | H | `s` | `event` | Handling time. Buckets B_FAST. | quack-bot |
| `quack.gateway.heartbeat.latency` | H | `s` | — | Heartbeat round trip. Buckets B_FAST. | gateway loop |
| `quack.gateway.connections` | C | `{connection}` | `kind` (connect, resume, reconnect, invalid_session) | Shard connection events. | gateway loop |
| `quack.gateway.connected` | G | `1` | — | 1 when the shard is connected and READY. | gateway loop |
| `quack.gateway.guilds` | G | `{guild}` | — | Servers in the guild cache. | gateway loop |
| `quack.gateway.queue.depth` | G | `{event}` | `queue` (events_0, events_1, events_2, events_3, activity_posts, interactions) | Items waiting. | quack-bot, activity log |
| `quack.gateway.queue.dropped` | C | `{event}` | `queue` | Items dropped because a queue was full. | quack-bot, activity log |
| `quack.cache.size` | G | `{entry}` | `cache` (guilds, members, rules, config, activity_messages, interactions, directory, account_guilds) | Entries held. | each cache |
| `quack.cache.lookups` | C | `{lookup}` | `cache`; `result` (hit, miss) | Lookups. | each cache |
| `quack.cache.evictions` | C | `{entry}` | `cache`; `cause` (capacity, expired, invalidated) | Evictions. | each cache |

**HTTP API**

| Name | Type | Unit | Attributes | Description | Recorded in |
| --- | --- | --- | --- | --- | --- |
| `http.server.request.duration` | H | `s` | `http.request.method`, `http.route`, `http.response.status_code`, `error.type` | Semconv; from the Tower layer. Buckets B_REQ. | quack-api |
| `http.server.active_requests` | UDC | `{request}` | `http.request.method` | Semconv. | quack-api |
| `quack.auth.sign_ins` | C | `{sign_in}` | `outcome` (ok, denied, failed) | OAuth callbacks. | quack-api |
| `quack.http.rate_limited` | C | `{request}` | `class` (sign_in, read, write, case_create, action_control, member_write) | 429 responses. | quack-api |
| `quack.sessions.active` | G | `{session}` | — | Unexpired sessions. | DB task |

**Database**

| Name | Type | Unit | Attributes | Description | Recorded in |
| --- | --- | --- | --- | --- | --- |
| `db.client.operation.duration` | H | `s` | `db.system.name` (postgresql), `db.operation.name` (SELECT, INSERT, UPDATE, DELETE, WITH, LISTEN), `db.collection.name` (table name) | Semconv. Buckets B_FAST. | quack-db query wrapper |
| `db.client.connection.count` | G | `{connection}` | `db.client.connection.state` (idle, used) | Pool connections. | quack-db |
| `db.client.connection.wait_time` | H | `s` | — | Pool acquire time. Buckets B_FAST. | quack-db |
| `quack.db.transaction.duration` | H | `s` | `name` (apply_rule, void_case, settle_action, settle_dm, decide_appeal, submit_appeal, patch_settings, edit_rule, enqueue, claim, settle_job, bootstrap, other) | Transaction begin to commit. Buckets B_FAST. | quack-db `Db::begin` guard |
| `quack.db.retries` | C | `{retry}` | `reason` (serialization_failure, deadlock) | Transactions retried (at most 3 times). | quack-db |
| `quack.db.errors` | C | `{error}` | `class` (unique_violation, foreign_key_violation, check_violation, serialization_failure, deadlock, statement_timeout, connection, other) | Database errors. | quack-db |

**Modules**

| Name | Type | Unit | Attributes | Description | Recorded in |
| --- | --- | --- | --- | --- | --- |
| `quack.ticket.opened` | C | `{ticket}` | `outcome` (open, open_failed) | Tickets that left `opening`. | tickets |
| `quack.ticket.closed` | C | `{ticket}` | `transcript` (complete, partial); `by` (member, staff, quack) | Tickets closed. | tickets |
| `quack.ticket.open` | G | `{ticket}` | — | Tickets in `open`. | DB task |
| `quack.ticket.transcript.size` | H | `By` | — | Transcript size. Buckets B_BYTES. | tickets |
| `quack.activity_log.events` | C | `{event}` | `event` (ActivityEvent values); `outcome` (posted, filtered, dropped, failed) | Server activity handled. | activity log |
| `quack.honeypot.hits` | C | `{hit}` | `outcome` (recorded, ignored_staff, ignored_bot, dropped, case_created, joined_burst, failed) | Honeypot posts. | honeypot |
| `quack.honeypot.cleanups` | C | `{message}` | `outcome` (done, retry, failed) | Bait deletions. | honeypot |

**Runtime and process**

| Name | Type | Unit | Attributes | Description | Recorded in |
| --- | --- | --- | --- | --- | --- |
| `quack.runtime.workers` | G | `{thread}` | — | Tokio worker threads. | telemetry, every 15 s |
| `quack.runtime.alive_tasks` | G | `{task}` | — | Alive Tokio tasks. | telemetry |
| `quack.runtime.global_queue_depth` | G | `{task}` | — | Tokio global queue depth. | telemetry |
| `quack.runtime.busy_time` | C | `s` | — | Sum of worker busy durations (busy ratio = rate / workers). | telemetry |
| `process.cpu.time` | C | `s` | `cpu.mode` (user, system) | From `/proc/self/stat`. | telemetry |
| `process.memory.usage` | G | `By` | — | Resident set size. | telemetry |
| `process.open_file_descriptor.count` | G | `{fd}` | — | Open file descriptors. | telemetry |
| `quack.build.info` | G | `1` | `version`, `commit` (one value per process) | Always 1. | telemetry |
| `quack.uptime` | G | `s` | — | Seconds since start. | telemetry |

### 18.3 Histogram buckets

| Set | Boundaries |
| --- | --- |
| B_FAST (s) | 0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1, 2.5 |
| B_REQ (s) | 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1, 2.5, 5, 10, 30 |
| B_LONG (s) | 0.1, 0.5, 1, 2, 5, 10, 30, 60, 300, 900, 3600, 14400, 86400 |
| B_DECISION (s) | 60, 300, 900, 3600, 14400, 43200, 86400, 259200, 604800, 2592000 |
| B_BYTES (By) | 1024, 16384, 131072, 1048576, 4194304, 8388608 |
| B_LEVEL | 1, 2, 3, 4, 5, 6, 7, 8, 9, 10 |

Configured with explicit-bucket views at meter provider creation.

### 18.4 Spans

Every unit of work is a span; each DB query and each Discord REST call is a
child span.

| Span name | Kind | Attributes | Events |
| --- | --- | --- | --- |
| `interaction {kind} {command}` | SERVER | `quack.guild_id`, `quack.actor_id`, `quack.interaction.id`, `quack.interaction.kind`, `quack.command`, `quack.outcome` | `acked` (`ack.type`, `ack.latency_ms`), `replied`, `refused` (`reason`) |
| `gateway {event}` | INTERNAL | `quack.guild_id`, `gateway.event`, `gateway.sequence` | — |
| `{method} {route}` (HTTP server semconv, e.g. `GET /api/guilds/{guild_id}/cases`) | SERVER | `http.request.method`, `http.route`, `http.response.status_code`, `url.scheme`, `user_agent.original`, `quack.account_id`, `quack.guild_id` | — |
| `job {kind}` | CONSUMER | `quack.job.id`, `quack.job.kind`, `quack.job.attempt`, `quack.job.max_attempts`, `quack.job.queue_wait_ms`, `quack.job.outcome`, `quack.guild_id`; a span **link** to the enqueuer | — |
| `{operation} {table}` (DB semconv, e.g. `UPDATE case_counters`) | CLIENT | `db.system.name`, `db.namespace`, `db.operation.name`, `db.collection.name`, `db.query.summary`, `db.response.status_code` (on error), `error.type` | — |
| `{method} {route pattern}` (HTTP client semconv, e.g. `PUT /guilds/{guild_id}/bans/{user_id}`) | CLIENT | `http.request.method`, `url.template`, `server.address` (discord.com), `http.response.status_code`, `discord.error_code`, `discord.rate_limit.wait_ms`, `quack.discord.outcome` | — |
| `use_case {name}` (e.g. `use_case apply_rule`) | INTERNAL | `quack.guild_id`, IDs relevant to the use case | domain events below |

Domain events (on the current span): `case.created` (`case.id`,
`case.number`, `level_index`, `action_kind`), `case.voided` (`case.id`,
`cause`), `action.attempt` (`action.id`, `kind`, `attempt`, `outcome`,
`http_status`, `discord_code`), `dm.outcome` (`dm.id`, `kind`, `state`,
`failure`), `appeal.transition` (`appeal.id`, `from`, `to`),
`evidence.captured` (`evidence.id`, `state`), `message.synced`
(`message_row.id`, `kind`, `outcome`), `mod_log.entry` (`entry.id`, `kind`,
`result`).

**Propagation into jobs.** `enqueue` stores the current span context as a
W3C `traceparent` in `jobs.trace_context`. The `job {kind}` span is a new
root (its own trace) with a span link to that context, so a retry hours
later stays a short trace while still pointing at the request that caused
it. HTTP requests accept an incoming `traceparent`; Discord interactions
start new traces.

### 18.5 Logs

- Emitted with `tracing`, exported over OTLP (`opentelemetry-appender-tracing`)
  and written to stdout: JSON lines when stdout is not a terminal, compact
  text otherwise. Every record carries `trace_id` and `span_id` when inside a
  span.
- Fields: `timestamp`, `level`, the Rust module path, `message`, and
  structured IDs (`guild_id`, `case_id`, `action_id`, `appeal_id`, `job_kind`, `job_id`,
  `account_id`) as span fields.
- Levels: `error` for failures needing an operator; `warn` for degraded but
  handled conditions (dropped events, exhausted retries, exporter failures);
  `info` for lifecycle (startup, shutdown, gateway connect); `debug` for
  per-request detail.
- **Never logged** (and never span attributes): the bot token, the client
  secret, the session secret, OAuth codes and states, Discord access tokens,
  session IDs, CSRF tokens, interaction tokens, message or evidence content,
  case reasons, context values, appeal text, transcripts, DM text, IP
  addresses, and Discord error bodies (only status and error code). Secrets
  are held in a `Secret<T>` newtype whose `Debug` and `Display` print
  `[redacted]`. Request logs use the route pattern, never the raw path or
  query.
- SDK and transport crates (`opentelemetry*`, `hyper`, `reqwest`, `h2`) are
  filtered out of the OTLP log bridge to avoid feedback loops; their
  warnings still reach stdout.

### 18.6 Resource and export

- Resource attributes: `service.name` (`QUACK_SERVICE_NAME`),
  `service.version` (crate version), `service.instance.id` (random UUID per
  process), `deployment.environment.name` (`QUACK_ENVIRONMENT`),
  `host.name`, `process.pid`, `vcs.ref.head.revision` (git commit, set at
  build time).
- Export: OTLP/HTTP protobuf to `QUACK_OTLP_ENDPOINT` with the blocking
  reqwest client built on the synchronous startup path. Spans: batch
  processor, queue 4,096, batch 512, every 5 s, export timeout 10 s.
  Metrics: periodic reader every 15 s, cumulative temporality. Logs: batch
  processor, queue 4,096, every 5 s.
- Sampling: parent-based, always on (moderation volume is low).
- Shutdown flushes all three providers within 5 seconds (§17.5).
- Export failures write one stderr warning per signal per minute.

### 18.7 Dashboards and alerts to build

Dashboards:

1. **Moderation**: cases by source and action kind; level distribution;
   actions by outcome and reason; time to done (p50/p95); DM outcomes by
   reason; appeals submitted, open, and time to decision.
2. **Reliability**: job ready depth and oldest-ready age by kind; job
   outcomes; lease expirations; dead jobs; failed actions waiting for staff;
   tracked message sync lag; log channel post outcomes.
3. **Discord**: REST requests by route and outcome; rate-limit hits and
   wait; gateway connected, reconnects, heartbeat latency; event rate and
   drops; interaction ack latency (p50/p99) and errors.
4. **API and database**: request rate, latency, and 5xx by route; sign-ins;
   rate-limit rejections; pool usage and wait; query and transaction
   latency; DB errors and retries.
5. **Runtime**: CPU, memory, file descriptors, Tokio busy time and queue
   depth, cache sizes and hit rates.

Alerts:

| Alert | Condition |
| --- | --- |
| Not ready | `/readyz` failing for 2 minutes. |
| Gateway down | `quack.gateway.connected` = 0 for 2 minutes. |
| Slow acknowledgements | p99 `quack.interaction.ack.duration` > 1.5 s for 10 minutes, or any `ack_late` errors. |
| Job backlog | `quack.job.oldest_ready_age{kind="run_action"}` > 120 s, or any kind > 600 s, for 5 minutes. |
| Dead jobs | `quack.job.dead` > 0 for `run_action`, `send_dm`, or `close_ticket`. |
| Failed actions | `quack.action.failed.current` grows by 10 or more in an hour. |
| DM couldn't confirm | `quack.dm.outcomes{state="unknown"}` > 5 in an hour. |
| Discord rate limits | Global rate-limit hits > 0 in 5 minutes. |
| Dropped events | `quack.gateway.queue.dropped` > 0 in 5 minutes. |
| API errors | 5xx ratio > 2% for 10 minutes. |
| Database | Pool wait p95 > 100 ms or DB errors > 10/min for 5 minutes. |

---

## 19. MySQL to PostgreSQL import

Go table names, column names, and state values appear verbatim in this
section because they are the source data.

`quack import-mysql` copies the Go system's MySQL database into an empty or
partly imported PostgreSQL database once, at cutover. It lives in
`quack-import`, the only crate with sqlx's `mysql` feature.

### 19.1 Preconditions

- The Go bot is stopped (no writers). The importer refuses to run when
  `quack_schema_migrations` in MySQL is not one of the accepted ledgers:
  versions `1–3`, `1–4`, or `1–5` with the names `baseline`,
  `audit_mirror_deliveries`, `launch_announcement`, `staff_roles`,
  `discord_user_mfa` in order. Missing later tables or columns (role lists,
  `discord_user_mfa`) read as empty.
- The PostgreSQL schema is migrated (`quack migrate`).
- The source account is read-only. The importer reads in one `REPEATABLE
  READ` snapshot (`START TRANSACTION WITH CONSISTENT SNAPSHOT`), with the
  session time zone set to `+00:00`; every `datetime(3)` is read as UTC.

### 19.2 Command and modes

```
quack import-mysql --source mysql://user:pass@host:3306/quack [--dry-run] [--report report.json]
```

- `--dry-run` reads everything, runs every transformation and validation,
  writes nothing to PostgreSQL except an `import_runs` row, and prints the
  report.
- Without `--dry-run` the import writes in one PostgreSQL transaction per
  server (plus one for global tables), so a failure leaves each server
  either fully imported or untouched.
- The report goes to stdout (human summary) and, with `--report`, to a JSON
  file; it is also stored in `import_runs.report`.

### 19.3 Identifier mapping

- **ULID → UUID.** Deterministic and order-preserving. A ULID is 128 bits:
  a 48-bit millisecond timestamp `T` and 80 random bits `R`. The UUID is
  `T (48 bits) ‖ 0b0111 (version 7) ‖ R[0..12) ‖ 0b10 (variant) ‖ R[12..74)`;
  bits `R[74..80)` are dropped. Ordering by `(T, R)` maps to ordering by
  `(T, R[0..74))`, so UUID order equals ULID order except for ULIDs that
  share their first 122 meaningful bits; the importer checks every mapped
  key is unique per table and aborts on a collision.
  `quack_core::ids::ulid_to_uuid` is this function; the runtime uses it to
  resolve legacy ULIDs in custom IDs and URLs (§11.5, §14.1).
- **Snowflake strings → `bigint`.** Trimmed decimal of 1–20 digits, value in
  `1..=i64::MAX`. Empty string and `quack-system` → NULL (Quack). Anything
  else → the row is reported and skipped (for references) or the import
  aborts (for primary identities like `guilds.discord_guild_id`).
- **Server.** The internal `guilds.id` ULID disappears; every child maps its
  `guild_id` ULID to the server's Discord ID through the `guilds` table.
- **Booleans** `tinyint(1)` must be 0 or 1. **Unsigned** values must fit
  the new column type (case numbers ≤ 2^63−1; versions ≤ 2^31−1); otherwise
  abort with the offending table and key.
- **Text** sentinels `''` → NULL for optional fields. Over-length text is
  cut to the new limit with "…" and counted in the report (`truncated`
  section), never silently.

### 19.4 Table mapping

| MySQL table | PostgreSQL | Transformation |
| --- | --- | --- |
| `guilds` | `guilds`, `case_counters` | `discord_guild_id` → `guild_id`; `name` cut to 100; `icon_url` → `icon_hash` (the hash segment of the CDN URL, else NULL); `owner_discord_user_id` → `owner_id`; `is_active` → `installed`; `created_at` → `first_joined_at`; `mfa_required = false` until the next `GUILD_CREATE`. `case_counters.last_number = max(cases.case_number)` for the server (0 if none). |
| `guild_settings` | `guild_settings` | CSV role lists → `bigint[]` (deduplicated, cut to 25); `audit_mirror_channel_discord_id` → `log_channel_id`; `appeal_queue_channel_discord_id` → `appeal_channel_id`; `managed_evidence_channel_discord_id` → `evidence_channel_id`; `appeal_rejoin_url` → `appeal_invite_url` (re-validated; invalid → NULL, reported); `notification_introduction` → `dm_intro` (cut to 1,000), `notification_footer` → `dm_footer` (cut to 500); `starter_policy_template_id` → `starter_rule_id`; `starter_policy_notice_acknowledged_at` → `starter_notice_acknowledged_at`. Dropped: `appeal_review_reason_required`, `starter_policy_notice_pending` (derived), `launch_announced_at`. `version = 1`. |
| `staff_members` | — | Dropped (attribution comes from Discord; counts reported). |
| `discord_user_mfa` | `discord_accounts` | Direct. Absent table → nothing. |
| `case_templates` + `case_template_context_fields` + `case_template_levels` + `case_template_level_actions` | `rules`, `rule_versions` | §19.5. |
| `cases` | `cases` | §19.6. |
| `case_action_executions` | `actions` | §19.7. |
| `case_action_attempts` | `action_attempts` | `attempt_number` → `number`; `started_at`, `finished_at`; status `succeeded` → `done`, `failed` → by `error_code`: suffix `rate_limited` → `rate_limited`; `network_error`, `discord_server_error`, `context_cancelled`, `discord_error` → `unknown`; `lease_expired` → `interrupted`; any other → `refused`. `running` with no finish → `interrupted`, `finished_at = started_at`. Payload JSON dropped; HTTP status and Discord code NULL. |
| `case_notifications` | `dms` (kind `case`) | `pending`, `prepared`, `claimed` → `pending` (sent after cutover); `sending` → `unknown`; `sent` → `sent` (`delivery_message_discord_id` → `message_id`; `sent_at` → `settled_at`); `failed` with `last_error_code = 'case_voided'` → `not_needed`; other `failed` → `failed` with `failure`: code ending `permission_or_hierarchy_denied` → `dms_closed`, `dm_prepare_failed` → `dm_channel_unavailable`, else `discord_refused`. Nonce: the new DM ID. Rendered text dropped. |
| `case_evidence_snapshots` | `evidence` | `capture_outcome`: `captured`, `uploaded` → `saved`; `deleted` → `failed`/`message_deleted`; `inaccessible` → `failed`/`quack_no_access`; `unavailable` → `failed`/`discord_error`. Uploaded synthetic snapshots (`uploaded`, the file ID as message ID) keep their IDs. `embeds_json` → summaries `{type, title, description, url}` (first 10). `content` cut to 4,000. Duplicate `(case, message)` rows: the earliest is kept, the rest are reported. |
| `case_evidence_attachments` | `evidence_files` | `position` by creation order (1–10; extras reported); `copy_outcome` `preserved` → `copied` (copy IDs from `preserved_*`), `metadata_only` → `link_only`. |
| `case_events` | — | Not imported: the case timeline is rebuilt from cases, actions, DMs, appeals, and the moderation log. Counts reported. |
| `case_publications` | `messages` (kind `case_receipt`) | `message_id`, `channel_id`, `case_id`; `wanted_revision = posted_revision = 1` (old receipts are not re-rendered; their old buttons resolve through §11.5). |
| `appeals` | `appeals` | Same states. `statement` (or legacy `content`, or joined `answers_json` values when `statement` is empty) cut to 4,000; `reviewed_by_discord_user_id` → `decided_by`; `reviewed_at` → `decided_at`; `decision_reason` cut to 2,000; `version` kept; `first_decided_at` = earliest accepted/rejected/closed event time. NULL `case_id` → skipped and reported. |
| `appeal_events` | `appeal_events` | `submitted` → `submitted`; `information_requested` → `information_requested`; `information_submitted` → `information_provided`; `reopened`, `accepted`, `rejected`, `closed` same. `body` → `text` (empty → the decision reason, else "(no text)"). `actor_type = staff` with empty actor → NULL. |
| `appeal_notifications` | `dms` (kind `appeal_update`) and `messages` (kind `appeal_post`) | `audience = member`: one DM per `event_id`, state mapping as for case notifications (`delivery_outcome_unknown` → `unknown`; `delivery_deferred` and `pending` → `pending`). `audience = staff`: the latest row per appeal with a `delivery_message_id` becomes the appeal's `appeal_post` (`wanted_revision = 2`, `posted_revision = 1` for `pending` or `needs_information` appeals so the new buttons replace the old ones; `1`/`1` otherwise). |
| `audit_log_entries` | `mod_log` | §19.8. |
| `audit_mirror_deliveries` | — | Not imported; old entries are never re-posted. Undelivered counts reported. |
| `module_configurations` (`tickets`) | `ticket_settings`, `messages` (`ticket_panel`) | `entry_channel_discord_id`, `queue_channel_discord_id`, `transcript_retention_days`, `enabled`. The panel (`entry_panel_channel_id`, `entry_panel_message_id`) becomes a tracked message with `wanted_revision = 2`, `posted_revision = 1` (re-rendered with new buttons at startup). |
| `module_configurations` (`general_logging`) | `activity_log_settings`, `activity_log_routes` | `channels` map: `message_edit` → `message_edited`, `message_delete` → `message_deleted`, `message_bulk_delete` → `messages_bulk_deleted`, `member_join` → `member_joined`, `member_leave` → `member_left`, `discord_ban` → `member_banned`, `discord_unban` → `member_unbanned`, `guild_change` → `server_changed`, `channel_change` → `channel_changed`. `include_attachment_metadata` → `include_file_names`, `include_embed_metadata` → `include_embed_types`. Dropped: `cache_entries_per_guild`, `max_delivery_attempts`. |
| `module_configurations` (`honeypots`) | `honeypot_settings`, `messages` (`honeypot_warning`) | `channel_discord_id`, `template_id` → `rule_id`, `warning_text` (the legacy default text → NULL), `enabled`; a non-empty `disabled_reason` → `enabled = false`, `disabled_reason = channel_deleted` when it says the channel was deleted, else NULL. The warning message becomes a tracked message (`wanted_revision = 2`, `posted_revision = 1`). |
| `tickets` | `tickets` | `open` → `open`; `resolved`, `cancelled` → `closed`; `short_id` = first 10 characters of the mapped UUID's u22; `thread_discord_channel_id` → `thread_id`; `resolved_by`/`resolved_at` → `closed_by`/`closed_at`; `entry_channel_id` from the settings; step columns set to `closed_at` for closed tickets. `log_channel_discord_id` + `log_message_discord_id` → a `ticket_post` tracked message (`1`/`1`). |
| `ticket_events` | — | Not imported (derived from ticket columns). |
| `ticket_member_states` | — | Not imported (the slot is derived, §13.2.2). |
| `ticket_message_journal` | `ticket_messages` | Direct; `author_name` NULL → "unknown"; `files = []`. |
| `ticket_transcripts` | `ticket_transcripts` | Direct; `complete = true`; rows already expired are skipped and counted. |
| `honeypot_triggers` | `honeypot_hits` | `created` → `case_created`; `failed`, `exempt` → `failed` (`failure` = old `failure_code` or `exempt`); `pending` → `pending` (a `honeypot_case` job is enqueued per member). |
| `honeypot_message_cleanups` | `honeypot_hits.cleaned_up_at` | `completed_at` copied to the matching hit; incomplete cleanups for created hits enqueue `honeypot_cleanup`. |
| `honeypot_warning_refreshes` | — | Not imported (the tracked message replaces it). |
| `v4_import_batches`, `v4_import_sources` | — | Not imported; each v4-imported case keeps its source ID in `cases.imported`. |
| `quack_schema_migrations` | `import_runs.report` | Recorded in the report only. |

### 19.5 Rules and their versions

For each `case_templates` row:

1. `rules`: `id` = mapped ULID; `guild_id`; `name` cut to 100 (when two
   non-archived rules in a server end up with the same lowercase name, the
   later one gets " (2)", " (3)" … and is reported); `current_version` =
   `version`; `archived_at`; `created_by`/`updated_by` from the Discord ID
   columns.
2. The **current version** is built from the child rows, validated with
   `Strictness::New`:
   - `name`, `description` (cut to 500), `reason` = `reason_template` (cut
     to 1,000), `appealable`, `counting_window_days` = `case_decay_days` (0
     → null).
   - `fields`: context fields by `position`; `field_type` `boolean` →
     `boolean`, `discord_message_link` → `message_link`, others same; label
     cut to 100.
   - `levels`: the default level (`is_default`, threshold 0) becomes
     `from_case_count = 1`; other levels use `trigger_case_count`. If a
     non-default level also has threshold 1, it replaces the default level
     (the Go code never chose the default in that case) and the dropped
     default is reported. Level names are dropped. `notify_user` →
     `send_dm`. Actions: `timeout_user {duration_seconds}` → `timeout`,
     `kick_user` → `kick`, `ban_user {delete_message_seconds}` → `ban`
     (missing → 0); `max_retries` dropped. More than 10 levels → abort for
     that server with the rule named.
   - A current definition that fails validation aborts that server's import
     with every problem listed.
3. **Older versions** referenced by cases (`cases.template_version <
   case_templates.version`) are reconstructed from those cases'
   `template_snapshot_json`: name, reason, appealable, decay, and fields from
   `template`/`context_fields`; levels = the distinct `selected_level`s seen
   across those cases (as above). They are stored with `reconstructed =
   true` and validated with `Strictness::Historical`. Versions no case used
   are not created (version numbers may have gaps). The report lists every
   reconstructed version and how many levels it knows.

### 19.6 Cases

| Column | Source |
| --- | --- |
| `id`, `number`, `member_id`, `created_at`, `updated_at` | mapped `id`, `case_number`, `target_discord_user_id`, timestamps |
| `source` | `dashboard`, `discord`, `honeypot` same; `v4_import` → `imported` |
| `moderator_id` | `moderator_discord_user_id` (NULL for honeypot and `quack-system`) |
| `rule_id`, `rule_version` | `template_id`, `template_version` (imported cases: NULL) |
| `case_count` | snapshot `selected_level.matched_case_count` |
| `level_index`, `level_from_case_count` | the snapshot's level located in the version's levels (`trigger_case_count` 0 → 1) |
| `action_kind`, `timeout_seconds`, `ban_delete_message_seconds` | snapshot `actions[0]` |
| `sends_dm` | snapshot `selected_level.notify_user` |
| `context` | `context_values_json` array → object `key → value` |
| `operation_key` | `idempotency_key` |
| `replaces_case_id` | `replaces_case_id` (checked: same server, the replaced case voided; otherwise dropped and reported) |
| `voided_at`, `voided_by`, `void_reason`, `void_cause` | `voided_*`; `void_cause = appeal_accepted` when an accepted appeal exists for the case, else `staff`; empty reason → "(no reason recorded)" |
| `imported` | v4 cases: `{v4_action, v4_moderator, v4_reason, v4_source_id}` from `metadata_json.v4` and `reason`; `sends_dm = false`, no action |

A non-imported case whose snapshot lacks `selected_level` or `template`
aborts that server's import with the case number listed (an operator must
decide). Dropped columns: `correlation_id`, `context_*` coordinates (their
messages are evidence rows), `metadata_json` (except v4), and
`template_snapshot_json` (after use).

### 19.7 Actions

| Column | Source |
| --- | --- |
| `id`, `case_id`, `guild_id`, `member_id` | mapped execution ID; the case and its member |
| `kind` | `timeout_user` → `timeout`, `kick_user` → `kick`, `ban_user` → `ban`, `remove_timeout` → `remove_timeout`, `unban_user` → `unban`; `send_dm` rows are skipped and counted |
| `undo_of_action_id` | `reversal_of_execution_id` |
| `timeout_ends_at` | for `done` timeouts: the last successful attempt's `response_payload_json.timeout_until`; otherwise `case.created_at + duration_seconds` |
| `delete_message_seconds` | `config_snapshot_json.delete_message_seconds` (0 when absent) |
| `requested_by` | `config_snapshot_json.requested_by` |
| `state` | `succeeded` → `done`; `pending`, `retrying`, `running` → `pending` with a `run_action` job (in-flight work resumes, §19.9); `cancelled` → `cancelled`; `failed` → see the next table |
| `attempts` | `attempt_count` |
| `dismissed_at`, `dismissed_by` | same |
| `done_at` | `finished_at` for `done` |
| `no_change` | response `reversal_noop = true` |

Failed executions, by `last_error_code`:

| Old code (suffix) | New state and reason |
| --- | --- |
| `permission_or_hierarchy_denied` | `failed`, `discord_refused`, HTTP 403 |
| `unknown_member_or_resource` | timeout: `failed`, `member_not_in_server`; others: `failed`, `discord_refused`, HTTP 404 |
| `validation_failed`, `discord_failure`, `invalid_action_config`, `unsupported_action` | `failed`, `discord_refused`, HTTP 400 |
| `guild_not_found` | `failed`, `quack_not_in_server` |
| `lease_expired_review_required`, `network_error`, `discord_server_error`, `rate_limited`, `context_cancelled`, `discord_error`, `discord_unavailable`, `guild_lookup_failed` | `failed`, `retries_exhausted` |
| `reversal_ownership_conflict`, `reversal_provenance_unavailable` | `skipped` when the §5.6 guard, run at import time, finds another valid case (`skipped_because_case_id`); otherwise `failed`, `retries_exhausted` |
| `reversal_authorization_unavailable`, `reversal_permission_denied` | `failed`, `retries_exhausted` |
| `case_voided` | `cancelled` |
| any other | `failed`, `retries_exhausted` |

### 19.8 Moderation log

`audit_log_entries` are append-only history, so every row is imported,
none re-posted:

| Old `action` | New `kind` |
| --- | --- |
| `case.create` (success) | `case_created` |
| `authorization.denied` | `case_create_refused` |
| `case.void`, `case.void.appeal` | `case_voided` (cause from the name) |
| `case.update` | `case_context_edited` (or `evidence_added` when metadata has `evidence_added`) |
| `case_action.succeeded` | `action_done` |
| `case_action.failed`, `case_action.recovered` | `action_failed` |
| `case_action.retry` | `action_retry_requested` |
| `case_action.dismiss` | `action_dismissed` |
| `case_action.reverse` | `action_undo_requested` |
| `case_notification.sent` / `.failed` | `dm_sent` / `dm_failed` |
| `appeal.submit` | `appeal_submitted` |
| `appeal.information_requested` | `appeal_information_requested` |
| `appeal.information.submit` | `appeal_information_provided` |
| `appeal.accepted`, `appeal.rejected`, `appeal.reopened` | `appeal_accepted`, `appeal_rejected`, `appeal_reopened` |
| `appeal.closed`, `appeal.close` | `appeal_closed` |
| `case_template.create`, `.update`, `.archive`, `.restore`, `.import` | `rule_created`, `rule_edited`, `rule_archived`, `rule_restored`, `rule_imported` |
| `case_template.bootstrap`, `guild.lifecycle.bootstrap` | `quack_joined` |
| `guild.lifecycle.leave` | `quack_left` |
| `guild_settings.update`, `guild_settings.starter_policy_notice.acknowledge` | `settings_changed` |
| `guild_settings.channel_reference.cleared`, `guild_settings.channel_references.repaired`, `audit_mirror.repaired` | `channel_setting_cleared` |
| `audit_mirror.failed` | `log_channel_failed` |
| `ticket.open`, `ticket.resolve` | `ticket_opened`, `ticket_closed` |
| `ticket.settings.update`, `honeypot.settings.update`, `general_logging.settings.update` | `module_settings_changed` |
| `honeypot.configuration.disabled` | `module_disabled` |
| `honeypot.trigger.failed` | `honeypot_case_failed` |
| anything else (reads, `evidence.capture`, `case_action.attempt`, `case_action.retrying`, `honeypot.trigger.detected`, `honeypot.case.created`, `case_template.export`, `v4_import.*`, mirror bookkeeping, unknown names) | `legacy` |

`result`: `success` → `ok`, `failure` → `failed`, `denied` → `refused`.
`source`: `imported` for all. `actor_id` from `actor_discord_user_id`. Typed
links filled from `resource_type`/`resource_id` and from metadata keys
`case_id`, `target_discord_user_id`/`member_discord_user_id`, `appeal_id`;
a link to a row that was not imported stays NULL. `detail` for mapped kinds
holds what the new kind needs when derivable, plus `{"legacy_action": …}`;
for `legacy` it is §9.5's redacted `{action, resource_type, resource_id,
failure_reason}`. `created_at` kept; `id` = mapped ULID.

### 19.9 In-flight work at cutover

The Go bot is stopped before the import; work it left unfinished is
resolved like this:

| Old state | New state |
| --- | --- |
| Execution `pending`, `retrying` | Action `pending` + `run_action` job: runs after cutover. |
| Execution `running` | Action `pending` + job; its open attempt becomes `interrupted`. Safe because actions are idempotent (§5.2), except that a timeout's end is now `case.created_at + duration` rather than the Go attempt's time. |
| Execution on a voided case, `pending`/`retrying` | `cancelled`. |
| Notification `pending`, `prepared`, `claimed` | DM `pending` + `send_dm` job: sent after cutover. For a kick or ban whose action is also pending, the action job waits for the DM (§6.3). |
| Notification `sending` | DM `unknown`. Never sent. |
| Appeal notification (member) `pending` or deferred `failed` | DM `pending` + job. `sending` → `unknown`. |
| Appeal staff post with `refresh_requested` | Appeal post re-rendered (`wanted_revision = 2`). |
| Audit mirror delivery not delivered | Dropped (reported). |
| Publication with `refresh_requested` | Not re-rendered; old buttons resolve through §11.5. |
| Honeypot trigger `pending` | If a case with the Go operation key `honeypot:<guild ULID>:<message ID>` exists, the hit becomes `case_created` with that case (and a cleanup job). Otherwise hit `pending` + `honeypot_case` job. |
| Ticket member slot reservation without a ticket | Dropped. |
| Ticket `open` whose thread no longer exists | `open`; the first `GUILD_CREATE` reconciliation (§13.1.5) finds the thread gone and closes the ticket. |

### 19.10 Validation report

`Report` (JSON; the summary prints the same sections):

| Section | Contents |
| --- | --- |
| `source` | MySQL server version, ledger rows, snapshot time, per-table row counts. |
| `written` | Per-table rows written (or that would be written, in dry-run), per server. |
| `skipped` | Per table: count and reason code, with up to 20 example keys each (IDs only, no content). Reason codes: `dm_action_row` (Go `send_dm` executions), `null_case_appeal`, `expired_transcript`, `duplicate_evidence`, `extra_attachment`, `invalid_snowflake`, `missing_parent`, `invalid_replacement`, `invalid_invite`. |
| `truncated` | Per column: count of values cut to the new limit. |
| `renamed_rules` | Rules renamed for name uniqueness (old name, new name). |
| `reconstructed_versions` | `(rule, version, known levels)`. |
| `dropped_default_levels` | Rules whose default level was replaced (§19.5). |
| `legacy_mod_log` | Count of `legacy` entries by old action name. |
| `in_flight` | Counts per row of §19.9. |
| `checks` | Relationship checks: every case's rule version exists; every action's case exists and matches server; replacement links are same-server and point at voided cases; appeal case and member match; counters ≥ max case number; mapped UUID uniqueness. Each `ok` or the failing keys. |
| `problems` | Errors that aborted a server, per server. |

The import exits 0 when every server imported and every check passed, 1
otherwise. Dry runs exit the same way.

### 19.11 Re-running

Mapped IDs are deterministic, every insert is `ON CONFLICT DO NOTHING`
(append-only `mod_log` included: inserts only), counters use
`GREATEST(last_number, $max)`, and jobs use their keys. A second run after a
complete import writes nothing and reports zero new rows; a run after a
partial failure completes the missing servers. Changing source data between
runs is not supported: when the source fingerprint (ledger plus per-table
row counts) differs from an earlier `apply` run's, the importer refuses;
to import a different source, drop and re-create the PostgreSQL database and
run `quack migrate` again.

### 19.12 Cutover runbook

1. **Rehearse.** Restore a production backup into a staging MySQL; run
   `quack import-mysql --dry-run` against an empty staging Postgres; fix
   every `problems` entry; then a real import; run `quack serve` against a
   staging Discord application and walk through §20.3's checklist.
2. **Announce** a maintenance window (expected under 15 minutes for
   current data sizes).
3. **Stop the Go bot** (`quack serve` and the dashboard server). Confirm no
   process holds the MySQL `quack_schema_migrations` lock or writes.
4. **Back up MySQL** (`mysqldump --single-transaction`) and verify the dump
   restores.
5. **Prepare Postgres**: create the database; `quack migrate`.
6. **Dry run** against production: `quack import-mysql --dry-run --report
   dry.json`; review `problems`, `checks`, `in_flight`.
7. **Import**: `quack import-mysql --report import.json`; it must exit 0.
8. **Start Quack** with the production configuration (same Discord
   application and token as Go). Startup syncs commands (removing Go-only
   commands), reconnects the gateway, re-renders appeal posts, ticket panels,
   and honeypot warnings, and runs in-flight jobs.
9. **Verify**: `/readyz` is ready; `quack.job.ready` drains; spot-check three
   servers (`/case view` of the newest case, `/case history`, the dashboard
   case page, a member's appeal page from an old DM link); confirm
   `quack.action.failed.current` and `quack.dm.outcomes` look normal.
10. **Keep MySQL** read-only for 30 days, then archive the dump.

**Rollback** (only before members act on the new system for long): stop
Quack; start the Go bot against the untouched MySQL. Work done by Quack
after cutover (cases, actions, DMs) exists only in Postgres and must be
replayed by hand from its moderation log; therefore roll back only within
the maintenance window.

---

## 20. Testing, development, operations, and decisions

### 20.1 Testing

CI runs, in order: `cargo fmt --check`; `cargo clippy --workspace
--all-targets --locked -- -D warnings`; `cargo nextest run --workspace
--locked`; `cargo test --doc --workspace --locked`; `cargo sqlx prepare
--workspace --check -- --all-targets`; the OpenAPI freshness test; in
`apps/dashboard`, `bun run api` (diff must be empty), `bun run check`, `bun
run test`, `bun run build`. Tests needing Docker (Postgres, MySQL) use
Testcontainers with pinned image digests and clean up their containers.

| Layer | What is tested | How |
| --- | --- | --- |
| `quack-core` | Rule validation (every problem code), escalation (proptest: monotonic level choice, first level at count 1, last level for large counts, window cutoff), capability derivation (owner, Administrator, Manage Server, moderator roles with and without deleted roles, fallback, 2FA), member checks in order, staff-channel check, every state machine (proptest: random event sequences never leave the transition tables; terminal states stay terminal), context validation per kind, message link parsing, `ulid_to_uuid` (order preserved, deterministic), u22 round trip. | Unit tests and proptest (regression seeds committed). |
| `quack-text` | Every message id and every listed variant, both surfaces (`Discord`, `Plain`); Markdown escaping; trimming to 4,000 characters; durations and plurals. | `insta` snapshots. |
| `quack-db` | Migrations apply to an empty database; every CHECK and trigger (mod_log append-only, rule_versions immutable); job queue: 8 concurrent claimers never claim the same row, lease expiry reclaim, fenced settle rejects stale tokens, coalescing and `rerun_requested`, dead-job revival. | `TestDb` (Postgres 18). |
| `quack-engine` | §20.1.1 scenarios. | `TestDb` + `FakeDiscord` (records calls, scripts outcomes per call, can "crash" a handler between steps). |
| `quack-discord` | Components V2 JSON for every `Msg` shape; custom ID encode/decode (proptest: round trip, ≤ 100 characters); legacy decode; REST outcome classification for every `RefusalKind` code, 429, 5xx, timeouts, connection reset; command definitions (snapshot) and the sync diff; captured interaction and gateway payloads decode. | `insta`, proptest, `httpmock` on loopback with Twilight's proxy setting. |
| `quack-api` | OAuth login and callback against a mocked Discord (state single use, nonce mismatch, error param); session expiry; CSRF and origin; rate limits; every error code's status and envelope; idempotency replay and reuse; member isolation (another member's case is 404); capability per route; static file rules; `openapi_is_current`. | `axum` `oneshot`, `TestDb`, `FakeDiscord`, `httpmock`. |
| Modules | Tickets: open adopts an existing thread; close resumes after a crash at each step; forced close on budget exhaustion; one live ticket per member. Server activity log: filtering, edit diffing, uncached delete batching, drops counted. Honeypot: burst joins one case, refusal keeps messages, staff and bots ignored, cleanup idempotent. | `TestDb` + `FakeDiscord`. |
| `quack-import` | A MySQL 8.4 container loaded with the Go schema (versions 3 and 5 ledgers) and fixtures covering every table and every mapping row in §19; dry run writes nothing; apply matches expected rows; re-run is a no-op; checks catch planted inconsistencies. | Testcontainers MySQL + `TestDb`. |

#### 20.1.1 Engine scenarios (each one test)

1. Two concurrent `apply_rule` calls for the same member and rule get
   consecutive case numbers and consecutive case counts.
2. The same operation key twice (sequential and concurrent) creates one case;
   a different request with the key is refused.
3. Each member refusal; Quack-caused refusals write `case_create_refused`,
   others write nothing.
4. Kick with DM: the action job is enqueued only when the DM settles; a
   failed DM still releases the kick.
5. Void with a `pending` action cancels it; with a `done` timeout queues
   `remove_timeout`; with a `running` ban, the ban settling `done` queues
   `unban` in the same transaction.
6. Crash after the attempt row is written: the next claim marks it
   `interrupted` and retries; after 8 unknown attempts the action fails with
   `retries_exhausted`.
7. Timeout end time is identical on every attempt; an attempt after the end
   fails with `timeout_window_passed`.
8. Undo guard: a newer valid case with a ban makes the undo `skipped` with
   that case's number.
9. Staff retry of an original action on a voided case is refused; retry of
   an undo works; dismiss hides from review; retry after dismiss is refused.
10. DM crash while `sending` becomes `unknown` and is never sent again.
11. Appeal: every legal transition, every illegal one (409), version
    mismatch, accept voids and queues undo, direct void closes an open
    appeal and sends one DM.
12. Tracked message bumped while its sync runs is synced again.
13. Evidence capture: deleted message, moderator without access, Quack
    without access, other server, file too large, unsupported type, copy
    failure, per-case caps.
14. Draft: double submit of a page, expired draft, rule edited mid-draft.
15. Settings: stale `expected_version` with role lists → 409; moderator role
    change without `manage_staff_roles` → 403.

### 20.2 Development

Requirements: Rust 1.100 (pinned in `rust-toolchain.toml` with rustfmt and
clippy), Docker, Bun, `sqlx-cli 0.9.0`, `cargo-nextest`, and a separate
development Discord application with both privileged intents enabled and
`http://localhost:3000/api/auth/callback` registered as a redirect.

`compose.yaml` at the repository root (project name `quack-dev`, so it never
touches other Compose projects or their containers):

| Service | Image | Ports | Profile |
| --- | --- | --- | --- |
| `postgres` | `postgres:18` (pinned digest), database `quack`, user `quack`, password `quack`, named volume | `55432:5432` | default |
| `otel` | `grafana/otel-lgtm` (pinned digest): OTLP receiver, Tempo, Prometheus, Loki, Grafana | `4318:4318`, `3001:3000` | `otel` |

`.env` (not committed; `.env.example` is):

```
QUACK_DATABASE_URL=postgres://quack:quack@localhost:55432/quack
QUACK_DISCORD_TOKEN=…
QUACK_DISCORD_APPLICATION_ID=…
QUACK_DISCORD_CLIENT_SECRET=…
QUACK_PUBLIC_URL=http://localhost:3000
QUACK_SESSION_SECRET=<openssl rand -base64 32>
QUACK_DEV_GUILD_ID=<your test server>
QUACK_DASHBOARD_DIR=apps/dashboard/dist
QUACK_OTLP_ENDPOINT=http://localhost:4318
```

| Command | What it does |
| --- | --- |
| `docker compose up -d postgres` | Start Postgres. |
| `docker compose --profile otel up -d` | Also start the OpenTelemetry viewer (Grafana on `http://localhost:3001`). |
| `cargo run -p quack -- migrate` | Apply migrations. |
| `cargo run -p quack -- serve` | Run Quack (API on `:8080`). |
| `cd apps/dashboard && bun install && bun run dev` | Dashboard dev server on `:3000`, proxying `/api` to `:8080`. |
| `cargo run -p quack -- commands sync` | Register commands without serving. |
| `cargo nextest run --workspace` | All tests (needs Docker). |
| `cargo nextest run -p quack-core` | Fast domain tests (no Docker). |
| `cargo sqlx prepare --workspace -- --all-targets` | Refresh `.sqlx/` after changing a query (needs the dev database). |
| `UPDATE_OPENAPI=1 cargo nextest run -p quack-api openapi_is_current` | Rewrite `contracts/http/openapi.json`. |
| `cd apps/dashboard && bun run api` | Regenerate the TypeScript client. |
| `cargo insta review` | Review changed text snapshots. |

To try the importer locally, restore a dump into a separate, uniquely named
MySQL container (`docker run --name quack-import-test -p 53306:3306 …
mysql:8.4`), import from it, and remove it afterwards; never point it at
someone else's running database.

Where things go: domain rules in `quack-core`; strings in `quack-text`;
queries and migrations in `quack-db`; use cases and jobs in `quack-engine`;
Discord specifics in `quack-discord`; HTTP in `quack-api`; interaction
routing in `quack-bot`; module behavior in its crate. A change to behavior,
configuration, or the API updates this document in the same commit.

### 20.3 Operations

**Deploying.**

- Image: multi-stage build with `cargo-chef` (cook, then `cargo build
  --release --locked -p quack` with `SQLX_OFFLINE=true`), the dashboard built
  with Bun in a separate stage, runtime `gcr.io/distroless/cc-debian13:nonroot`
  (pinned digest) containing `/app/quack` and `/app/dashboard`
  (`QUACK_DASHBOARD_DIR=/app/dashboard`), exposing 8080, entrypoint
  `quack serve`.
- Release: run `quack migrate` as a one-off step, then roll the single
  `quack serve` instance. Exactly one instance per Discord application (one
  gateway shard; two instances would double-handle events).
- Probes: liveness `GET /livez`, readiness `GET /readyz`. Termination grace
  period ≥ 35 seconds.
- Discord application: privileged intents Server Members and Message
  Content; OAuth redirect `<QUACK_PUBLIC_URL>/api/auth/callback`; install
  scopes `bot applications.commands`; bot permissions View Channel, Send
  Messages, Send Messages in Threads, Embed Links, Attach Files, Read
  Message History, Moderate Members, Kick Members, Ban Members, Manage
  Messages, Manage Channels, Manage Threads, Create Private Threads, View
  Audit Log. `GET /api/guilds/{guild_id}/health` reports what a server is
  missing.
- Backups: Postgres continuous archiving (point-in-time recovery) plus a
  daily `pg_dump`; restore drill every quarter.

**Health and monitoring.** Use the dashboards and alerts in §18.7.
`quack.job.ready`, `quack.job.oldest_ready_age`, and `quack.job.dead` show
background work; `quack.action.failed.current` shows actions waiting for
staff.

**Incidents.**

| Situation | What happens | What to do |
| --- | --- | --- |
| Postgres down | `/readyz` fails; interactions answer `error.internal`; jobs stop; nothing committed is lost. | Restore Postgres. Quack resumes on its own. |
| Discord down or rate limiting | Jobs retry within their budgets; actions may end `retries_exhausted`; log posts may be dropped after 5 attempts. | After recovery, staff retry failed actions from "Actions that need review". |
| Dead jobs | Rows with `dead_at` in `jobs`, visible in metrics. | Inspect `last_error`; fix the cause; re-enqueue by repeating the triggering operation (or `UPDATE jobs SET dead_at = NULL, attempts = 0, run_at = now() WHERE id = …`, the only sanctioned manual edit). |
| A stuck "in progress" | Cannot persist past the budgets in §16.5. | Check `quack.job.oldest_ready_age`; check the gateway and Discord status. |
| Leaked bot token or client secret | — | Rotate in the developer portal; update the environment; restart. |
| Leaked session secret | Discord tokens in sessions are exposed. | Rotate `QUACK_SESSION_SECRET` (signs everyone out) and `DELETE FROM sessions`. |
| Revoke one account's sessions | — | `DELETE FROM sessions WHERE account_id = …`. |
| Bad migration | `quack serve` refuses to start on a mismatch. | Never edit an applied migration; write a new one; restore from backup only when it cannot be fixed forward. |

Never hand-edit `mod_log` (the trigger refuses), `rule_versions` (same),
`actions`, `dms`, `appeals`, or `cases`: use staff controls, which re-check
permissions and keep the state machines intact.

**Before the first real release**, rehearse in a staging server with a
staging application: install (starter rule, notice, `/setup` creating each
channel, deleting a configured channel, leaving and rejoining);
permissions (owner, Administrator, Manage Server, Moderate Members before
and after moderator roles are set, a moderator role without Discord
moderation permissions, a rules manager, a server requiring 2FA with and
without confirmation, a former moderator, every member check); cases
(warning, timeout, kick, ban from Discord and the dashboard, private and
public receipts, multi-page form, evidence with and without an evidence
channel, the DM with DMs open and closed, void and undo, the undo guard
with two cases, retry and dismiss); appeals (from a DM after a ban, every
decision, the appeal post, DMs); each module; the log channel; `SIGTERM`
with jobs in flight, confirming nothing runs twice after restart; and the
full import runbook (§19.12) on a production copy.

### 20.4 Known gaps

1. **One process, one shard.** Quack supports up to 2,500 servers (Discord's
   single-shard limit) and must run as one instance.
2. **2FA is only as fresh as the last dashboard sign-in.** Quack cannot read
   2FA with the bot token.
3. **Staff channel overwrites are not re-synced** when staff roles change;
   new moderator roles must be added to existing staff channels by hand.
4. **Private ticket threads** are visible only to members added to them and
   to people with Manage Threads; a moderator without Manage Threads cannot
   open a ticket thread from the staff post.
5. **Server activity log is best effort.** Posts are in memory and lost on
   a crash or a full queue (counted).
6. **Log channel posts are dropped after about 4 minutes of Discord
   failures** (5 attempts inside the nonce window) to avoid duplicates; the
   dashboard always has the full log.
7. **A tracked message create whose result is unknown, retried after
   Discord's nonce window, can post twice.** Retries happen within 12
   seconds, so this needs a long stall.
8. **A kick or ban DM is sent before the action**; if the action then fails,
   the DM still says the member is being removed or banned.
9. **Discord-native moderation is invisible to the undo guard.** Voiding a
   case removes its timeout or ban even if a moderator separately applied
   one in Discord.
10. **Reconstructed rule versions are partial** (only the levels cases
    used).
11. **No live-server rehearsal yet** (§20.3).

### 20.5 Decisions for the owner

Each item is a choice between plausible behaviors. Overrule any by editing
this document first.

1. **Server key.** `guilds.guild_id` is the Discord server ID (`bigint`)
   instead of a UUID: it removes the internal-ID mapping layer and makes
   every tenant foreign key readable.
2. **Fixed retry budget.** 8 attempts over about 4.6 hours for every action
   replaces the per-level `max_retries` (0–10) setting; old values are
   dropped on import. Rationale: actions are now idempotent, so a knob adds
   nothing but configuration.
3. **Level names dropped.** Levels are shown as "level N (starts at case
   M)".
4. **Rule slugs dropped.** Names must be unique among a server's active
   rules (case-insensitive); the importer renames clashes and reports them.
5. **New and changed limits**: at most 10 levels per rule and 100 active
   rules per server (new, needed to bound caches and pickers); description
   500 characters; void reason 1,000 (was unbounded); DM introduction 1,000
   and footer 500 (were 2,000 bytes); decision reason 2,000 (kept); file
   copies up to 10 MiB (was 25 MiB; Discord's base upload limit is 10 MiB).
6. **Decision reasons are always required**; the per-server
   `appeal_review_reason_required` switch is dropped. Discord modals are
   prefilled with default text.
7. **Appeal reversal offers are dropped.** Accepting voids the case, which
   queues guarded undo actions; a skipped undo can be done by hand with the
   Remove timeout/Unban button.
8. **Undo actions re-check only Quack's own access** at run time, not the
   requesting moderator's current authority (checked when they voided or
   pressed the button).
9. **Timeouts on members with Administrator are refused before the case
   exists** (new member check 10), instead of failing afterwards.
10. **A timeout whose fixed end has passed fails** with
    `timeout_window_passed` rather than counting as done.
11. **Kick and ban DMs describe the decision** ("You're being banned")
    because they are sent before the action.
12. **New DM failure reason `dm_channel_unavailable`** for a DM channel that
    could not be opened after 3 tries.
13. **All staff reads are ephemeral**, including `/case view`, `/case
    list`, `/case history`, `/appeals`, and `/help` (some were public).
14. **Guided `/setup`** (one panel) replaces the subcommands `/setup
    appeals`, `audit`, `evidence`, `tickets`, `honeypot`, and `logging`.
15. **`/case add` has no `message_link` or `file` options** (the charter's
    option list). Evidence comes from the "Add case" message command, from
    `message_link` fields, and from Add evidence. File uploads are no longer
    possible from Discord except as attachments of a linked message.
16. **Edit context in Discord is limited to rules with at most 5 fields**;
    larger ones are edited on the dashboard.
17. **Prefilled-modal exception.** Edit context and re-opening a draft page
    do one primary-key read (300 ms deadline) before responding with the
    modal, because Discord needs the prefilled values in the first response.
18. **Message-link auto-fill.** A case started from a message fills empty
    `message_link` fields with that message's link.
19. **Modals open only for rules with a required field**; optional context
    is added afterwards (kept from Go).
20. **Activity log defaults**: message text, file names, and embed types on;
    per-event routing kept; cache size and delivery-attempt settings dropped
    (constants).
21. **Activity log posts are not durable jobs** (in-memory queue with drop
    counting), because they are high volume and best effort.
22. **Honeypot rule protection.** Archiving the honeypot's rule or making
    one of its fields required is refused, instead of the Go behavior of
    turning the honeypot off afterwards.
23. **Honeypot ignores every bot** (cases are never about bots); refused
    hits keep their message.
24. **Honeypot bait deletions** appear in the server activity log as
    ordinary deletions.
25. **Tickets**: one live ticket per member is a partial unique index (the
    slot table is gone); closing is a durable job that always finishes,
    with a journal-only transcript if needed. "Fix access" is kept and not
    logged.
26. **Moderation log scope**: reads and exports are not recorded; `dm_sent`
    and `evidence_added` are recorded but not posted; imported Go entries
    that have no new kind are kept as `legacy`.
27. **Log channel retries** stay inside Discord's nonce window (10 s to 2
    min, 5 attempts) instead of the Go schedule of 1 min to 6 h, trading
    possible loss in long outages for no duplicates.
28. **Not imported**: `case_events`, `ticket_events`, `staff_members`,
    `audit_mirror_deliveries`, `ticket_member_states`,
    `honeypot_warning_refreshes`, `v4_import_*` (timelines are rebuilt; all
    counts are reported).
29. **Sessions last as long as the Discord token (at most 7 days)** with no
    sliding renewal; `POST /auth/logout-all` is dropped (operators delete
    rows).
30. **Sessions and idempotency are hand-written on Postgres**;
    `tower-sessions` from the stack audit is not used.
31. **Dependencies beyond the stack audit's pins**: `aws-lc-rs` directly
    (SHA-256, AES-256-GCM, secure random; already in the graph through
    rustls), `tower-http` features `fs` and `compression-gzip`,
    `futures-util` (`BoxFuture`), `bytes`, `url`. Versions: the ones the
    lockfile already resolves.
32. **Configuration keys beyond the charter's list**:
    `QUACK_DATABASE_MAX_CONNECTIONS`, `QUACK_DASHBOARD_DIR`,
    `QUACK_ENVIRONMENT`, `QUACK_TRUSTED_PROXY_HOPS`.
33. **The `Discord` port's implementation lives in the binary**
    (`DiscordPort`), because `quack-discord` may not depend on
    `quack-engine` where the trait is defined.
34. **Module HTTP routes live in `quack-api`**, calling each module's
    service functions, so modules never depend on the API crate.
35. **Span names** follow OTel semantic conventions for HTTP server, HTTP
    client (Discord), and database spans (`GET /api/…`, `PUT /guilds/…`,
    `UPDATE cases`) instead of the `http …`, `discord …`, `db …` prefixes.
36. **Operation-key replays** return the current representation of the
    original result (the stored reference), with the original status code,
    rather than a byte copy of the first response.
37. **Settings PATCH** requires `expected_version` only when it changes role
    lists.
38. **Legacy Discord buttons**: old appeal buttons in DMs and old ticket
    panels keep working; old case buttons open the case card; everything
    else answers with `interaction.legacy_button`. Old receipts are not
    re-rendered; appeal posts, ticket panels, and honeypot warnings are.
39. **Dashboard paths** `/audit` → `/mod-log` and `/modules/logging` →
    `/modules/activity-log`, with redirects.
40. **Job order is FIFO per kind** with no per-server fairness (actions are
    serialized per member).
41. **Server health** (`GET /api/guilds/{guild_id}/health`) replaces the
    per-server ops status and is available to `manage_settings` (was owner
    and Administrator only).
42. **Members see evidence** on their own case page (text and files, never
    authors or links), reading the charter's "staff-only unless shown in the
    appeal view" as the member's case/appeal page.
43. **Members see the void reason** of their case (kept from Go).
44. **Receipt visibility kept from Go**: the member command "Add case" posts a
    public receipt; the message command's receipt is private.
45. **Ticket transcripts are capped at 8 MiB.**
46. **Statistics** default to the calendar month before `to` (kept) and are
    computed in one read-only snapshot.
