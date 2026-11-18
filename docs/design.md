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

> Rules managers define the server's moderation rules. Moderators apply those
> rules. Quack chooses and carries out the configured result.

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
   stuck. Effects are recorded as started before Discord is called, and a
   retry first reads Discord's current state, so an effect whose result was
   lost is not applied twice (the one exception, a duplicate staff post after
   a long outage, is listed in §20.4).
8. **One lock point per record.** Every change to a case, its action, DM,
   evidence, or appeal locks the case row first (§4.12). A background job
   writes only when the row is still in the state the job read, so a job
   that lost its lease cannot overwrite newer state (§16.1).

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
  receipt may be public, only when the moderator did not ask for it to be
  private, and then only with the public receipt's fixed fields (§11.7).
- Machine codes (Discord error numbers, HTTP statuses, enum values) never
  appear in a sentence. They are kept in structured fields for staff
  diagnostics (`Action.discord_code`, API `error.code`).

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

Every staff operation is authorized from current facts, before any staff
data is shown and before any write. Losing a role takes effect on the next
operation; earlier records stay attributed to whoever acted. "Current
facts" means:

- **Discord interactions** (commands, buttons, selects, modals): the
  actor's roles and permissions come from the interaction payload, which
  Discord computes at the moment of the click. Every component click and
  modal submit is authorized again; a custom ID never carries authority.
- **Dashboard requests**: the actor's member record comes from the member
  cache only while the gateway session is connected and the server has been
  loaded since the last (re)connect, because gateway events then keep it
  exact; otherwise from Discord REST. "Not a member" is never cached.
- **Server facts** (owner, roles and their positions and permissions,
  MFA level, channel overwrites) come from the guild cache under the same
  connected-and-loaded condition, else REST.
- **Settings** (staff role lists) are read inside the write's transaction.
- **2FA** is the `discord_accounts` row (§2.2).

Queued work (an action, a DM) is authorized when the staff operation that
created it commits; the job checks only Quack's own access when it runs
(§5.4).

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

`quack_core::permissions::check_member(facts: &ApplyFacts, purpose: CheckPurpose) -> Result<(), MemberRefusal>`
is pure. `CheckPurpose` picks which checks run: `ApplyRule` (1–8),
`ApplyHoneypotRule` (1, 3, 5, 6, 8; §2.4), or `RetryAction(kind)` (2, 4, 6,
7, and 3 for a timeout; §5.8). The caller gathers `ApplyFacts` from the `Discord` port under the
freshness rules above. Checks run in this order; the first failure wins.

| # | Check | Refusal (`MemberRefusal`) | Caused by Quack's access? |
| --- | --- | --- | --- |
| 1 | Quack is in the server. | `QuackNotInServer` | yes |
| 2 | The actor is in the server (skipped for Quack-initiated work). | `ActorNotInServer` | no |
| 3 | The member is in the server (only where the operation requires it, see §2.4). | `MemberNotInServer` | no |
| 4 | The member is not the actor. | `MemberIsSelf` | no |
| 5 | The member is not a bot account (this includes Quack). | `MemberIsBot` | no |
| 6 | The member is not the server owner. | `MemberIsOwner` | no |
| 7 | The member's highest role position is strictly below the actor's highest role position, unless the actor is the owner. Equal positions refuse (a Quack policy, stricter than Discord's own tie-break by role ID; kept from Go). | `MemberRoleTooHigh` | no |
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
| Apply a rule (`/case add`, "Add case", dashboard) | `moderate` + 2FA | Rule active (checked under lock, §4.2). Member checks 1–10; check 3 required. |
| Apply the honeypot rule (Quack) | none (Quack acts) | Member checks 1, 3, 5, 6, 8, 9, 10 with Quack as the actor. |
| View a case, list cases, member history, evidence | `moderate` + 2FA | none |
| Edit case context, add evidence | `moderate` + 2FA | none (allowed on valid and voided cases) |
| Void a case | `moderate` + 2FA | Case is valid. |
| Retry, dismiss, or undo an action | `moderate` + 2FA | The single eligibility function `quack_core::action::controls` (§5.8), evaluated inside the write's transaction and also used to decide which buttons to render. |
| Accept, reject, request information, close, reopen an appeal | `moderate` + 2FA | Appeal state allows the transition (§8.2); `expected_version` matches; the case is valid (an open appeal on a voided case cannot exist, §4.7). |
| Submit an appeal, reply with information | member self | §8.1 eligibility; the case is valid. |
| Rule create, edit, archive, restore, import, export | `manage_rules` + 2FA | §3 validation. |
| Read settings | `manage_settings` + 2FA | none |
| Change settings | `manage_settings` + 2FA | `manage_staff_roles` when `moderator_role_ids` changes; channel checks (§13.1.3). |
| Guided `/setup`, module settings, enable/disable | `manage_settings` + 2FA | Module enable checks (§13). |
| Server health | `manage_settings` + 2FA | none |
| Read the moderation log, statistics | `moderate` + 2FA | none |
| Open own ticket | member of the server | Tickets enabled; no ticket of theirs is opening, open, or closing. |
| Close a ticket | ticket's member, or `moderate` + 2FA | Ticket `open`. |
| Fix ticket access | `moderate` + 2FA | Ticket `open`. |

Every check that reads stored state (case valid, action failed, appeal
version, ticket open) runs inside the transaction that makes the change,
after the row lock of §4.12, never only before it.

---

## 3. Rules and escalation

### 3.1 Storage model

- `rules` holds a rule's identity: ID, server, `current_version`, the current
  name (denormalized for listing and uniqueness), `archived_at`, and who
  created and last changed it.
- `rule_versions` holds every version as one validated JSON document
  (`definition`), immutable once written (triggers reject UPDATE, DELETE,
  and TRUNCATE; a plain `TRUNCATE rule_versions` is already rejected by the
  foreign key from `rules` before the trigger runs, so only `TRUNCATE …
  CASCADE` reaches it).
- A case references `(rule_id, rule_version)` and stores only its decision
  (§4.4).

Editing a rule locks the rule row (`SELECT … FOR UPDATE`), checks
`expected_version` (mismatch → `409 rule_changed`), inserts version
`current_version + 1`, and updates `rules.current_version` and `rules.name`
in one transaction. Archive and restore change `archived_at` only, never the
version. There is no delete, draft, or enable switch: a saved rule that is
not archived is active immediately.

Case creation reads the rule row `FOR SHARE` inside its transaction
(§4.2), so an edit or archive and a case creation are ordered: a case
either uses the version that was current when it locked the rule, or waits
for the edit to commit and then uses the new version (or is refused because
the rule is now archived). Caches are only for display, pickers, and
building modals; they never decide which version a case uses.

Rule creation, import, and restore lock the server's `guild_settings` row
first, so the active-rule limit is checked against a count no concurrent
write can change. Archive and edit lock `honeypot_settings` (`FOR SHARE`)
before the rule, so the honeypot checks in §3.3 and §3.5 hold at commit.

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
| `fields` | array | Case context fields, in display order. A rule with no fields still takes free-text context: one optional `long_text` field `{"key": "details", "label": "Details"}` that `RuleDefinition::context_fields()` returns when `fields` is empty. It is never stored in `fields`. |
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
pub struct Level { pub from_case_count: u32, pub action: Option<LevelAction>, pub send_dm: bool }
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
| `levels[0].from_case_count` | Exactly 1. | `first_level_not_one` |
| `levels[i].from_case_count` | 2–1,000,000 for i > 0; strictly increasing. | `level_case_count`, `level_order` |
| timeout `duration_seconds` | 1–2,419,200 (28 days). | `timeout_duration` |
| ban `delete_message_seconds` | 0–604,800 (7 days). | `ban_delete_window` |
| honeypot rule | When the server's honeypot uses this rule, no field may be required. | `required_field_on_honeypot_rule` (409) |
| active rule count | At most 100 non-archived rules per server (create, import, restore). | `too_many_rules` (409) |

`validate` takes a `Strictness`:

- `Strictness::New` applies every row above. It is used for every
  definition written through Quack (create, edit, import of a rule file).
  Restore re-checks only the name and the active-rule limit.
- `Strictness::Imported { reconstructed }` is used only by the MySQL
  importer (§19.5). It checks structure (a name and reason that are not
  empty, 1 or more levels with strictly increasing `from_case_count`, valid
  action kinds, a timeout within Discord's 28 days, unique field keys) but
  not size limits, so legacy rules are imported as they were and reported.
  It requires a first level of 1 unless `reconstructed` is true (partial
  versions rebuilt from old cases, never applied to a new case).

A legacy rule that exceeds a size limit keeps working: cases use it as
imported. The first edit must bring it within the limits.

### 3.4 Choosing the level

`quack_core::escalation::select_level(def: &RuleDefinition, case_count: u32) -> Option<LevelChoice>`
is pure:

1. `case_count` is the number of **counted** cases plus one for the new case.
   A case is counted when it is in the same server, for the same member, for
   the same `rule_id` (any version), not voided, not `source = 'imported'`,
   and, when the current version has a counting window, created at or after
   `now - counting_window_days × 86,400 s`. The window always comes from the
   version being applied, so changing the window affects counting only.
2. The chosen level is the level with the highest `from_case_count` that is
   less than or equal to `case_count`. Because the first level of every
   version a case can use starts at 1, there is always one. Below every
   level (only a reconstructed imported version, never applied to a new
   case) the first level applies.
3. `LevelChoice { index: u8 /* 1-based */, from_case_count: u32, action, send_dm }`.
   The result is `None` only for a definition with no levels, which
   validation never lets through; callers treat `None` as an internal
   error.

Escalation never uses other rules, other servers, weights, or a moderator's
choice. Proptest covers: monotonic level choice in `case_count`; the first
level for `case_count = 1`; the last level for very large counts.

### 3.5 Archive and restore

- Archive sets `archived_at = now()`; the rule disappears from pickers and
  autocomplete; its cases and versions stay visible. Archive is refused
  with `409 rule_used_by_honeypot` while the honeypot module's settings point
  at the rule, whether or not the honeypot is on; the honeypot settings
  offer "Clear rule" (§13.4.1) to release it. A case creation that locks the
  rule after the archive commits is refused with `rule_archived`.
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

When Quack joins a server for the first time (bootstrap creates its
`guild_settings` row, §17.5), it creates, in the same transaction, this rule and stores its ID in
`guild_settings.starter_rule_id`:

```json
{
  "name": "General rule violation",
  "description": "A starter rule for general violations. Review it and change it for this server.",
  "reason": "General rule violation",
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

These are the Go starter's texts and levels, unchanged except "guild"
becomes "server". `created_by` is NULL (Quack). The dashboard shows a notice asking
administrators to review it until someone with `manage_settings`
acknowledges it (`starter_notice_acknowledged_at`). Rejoining a server never
creates a second starter rule.

### 3.8 The honeypot rule

The honeypot module (§13.4) applies one rule chosen in its settings. When an
administrator turns on the honeypot from `/setup` and the settings name no
rule, Quack creates this rule and saves it as the honeypot's rule in the
same transaction. Quack never adopts an existing rule by its name: when an
active rule is already called "Honeypot", the turn-on is refused with
`module.problem.honeypot_rule_missing` so the administrator chooses a rule.

```json
{
  "name": "Honeypot",
  "description": "Applied when a member posts in the honeypot channel.",
  "reason": "Posted in the honeypot channel despite the warning.",
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

All limits live in `quack_core::limits` as constants and are validated by
the application before every write. Database CHECK constraints mirror a
limit only where imported legacy data cannot exceed it (§15); elsewhere the
column admits legacy values, which are imported as they were and listed in
the import report (§19.10). "Characters" means Unicode scalar values after
trimming.

| Limit | Value | Where enforced |
| --- | --- | --- |
| Rule name | 1–100 characters | §3.3 |
| Rule description | 0–500 characters | §3.3 |
| Official reason | 1–1,000 characters | §3.3 |
| Context fields per rule | 0–10 | §3.3 |
| Field key | `^[a-z0-9][a-z0-9_-]{1,63}$` | §3.3 |
| Field label | 1–100 characters | §3.3 |
| Levels per rule | 1–10 | §3.3 |
| Level `from_case_count` | first = 1; others 2–1,000,000, strictly increasing | §3.3 |
| Timeout duration | 1–2,419,200 s | §3.3 |
| Ban message deletion window | 0–604,800 s | §3.3 |
| Counting window | null or 1–36,500 days | §3.3 |
| Active rules per server | 100 | §3.3 |
| Context `short_text` | 1–500 characters | §4.6 |
| Context `long_text` | 1–4,000 characters | §4.6 |
| Context `message_link` | a Discord message link; input at most 4,000 characters, stored canonical | §4.6 |
| Context `number` | a decimal with absolute value at most 1,000,000,000,000,000 and at most 6 decimal places, stored exactly | §4.6 |
| Free-text context (`details`, rules with no fields) | 1–4,000 characters | §4.6 |
| Void reason | 1–1,000 characters | §4.7 |
| Appeal statement, information reply | 1–4,000 characters | §8.1 |
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
| Rejoin invite | an invite link whose code is 2–64 of `[A-Za-z0-9_-]` | §13.1 |
| Honeypot warning text | 0–1,500 characters | §13.4 |
| Ticket transcript retention | 1–365 days (default 90) | §13.2 |
| Ticket transcript file | 8,388,608 bytes (8 MiB) | §13.2 |
| `Idempotency-Key` header | 8–200 printable ASCII characters | §14.1 |
| API page size | default 50, maximum 100 | §14.1 |
| Discord list page size | 10 rows | §11 |
| Case draft lifetime | 15 minutes | §11.6 |
| Discord nonce retry window | 60 seconds from the first attempt | §10.4 |
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
| `discord` | `/case add`, the "Add case" message command, the "Add case" member command | Operation key: interaction ID, or `draft:<draft_id>` for forms (§11.6). |
| `dashboard` | `POST /api/guilds/{guild_id}/cases` | Operation key: `Idempotency-Key` header. |
| `honeypot` | The honeypot module | Operation key `honeypot:<first message id>` (§13.4.2). `moderator_id` is NULL. |
| `imported` | The MySQL importer, for cases the Go system imported from Quack v4 | Never counted for escalation; no rule, no action, no DM, no appeal. |

### 4.2 Creating a case, end to end

`quack_engine::cases::apply_rule(&self, req: ApplyRule) -> Result<Applied, ApplyError>`
is the only way to create a case. `ApplyRule` carries: guild, actor
(`Principal` or `Quack`), member ID, rule ID, an optional pinned rule
version (a draft's version, or the version the dashboard form was built
from), context values, explicit evidence links and, for the message command,
the source message, `private` (receipt visibility), the receipt destination
(the interaction token for a private receipt, or the channel for a public
one), optional `replaces_case_id`, source, the operation key, and, for
honeypot cases, the honeypot configuration version the hits were admitted
under. The caller has already acknowledged the interaction or accepted the
HTTP request.

**Before the transaction** (no locks held; may call Discord):

1. **Authorize.** For staff sources, the `Principal` (built from current
   facts, §2) must hold `moderate` and pass the 2FA rule. Otherwise
   `Forbidden`. Nothing about the operation key is revealed before this.
2. **Replay check.** Look up the operation key (§4.3). Same request hash →
   `Applied::Replayed(case_id)`; the caller renders the existing case for
   this actor. Different hash → `ApplyError::OperationKeyReused`.
3. **Early checks** against the rules cache, so most mistakes are answered
   without a transaction: rule missing → `RuleNotFound`; archived →
   `RuleArchived`; context invalid for the cached version (§4.6) →
   `ContextInvalid`. These are repeated authoritatively in step 10.
4. **Evidence sources.** Collect, deduplicated by message ID: the source
   message (message command), every `message_link` context value, every
   Discord message link found inside a text answer (§4.6), and explicit
   links (dashboard). Explicit links and `message_link` values must point
   into this server (`other_server`); links found inside text that point
   elsewhere are ignored. More than 50 → `TooMuchEvidence`. When the case
   starts from a message, every empty `message_link` field is filled with
   that message's link first.
5. **Facts.** Ask the `Discord` port for `ApplyFacts`: server owner, MFA
   level, Quack's member and permissions, the actor's member (staff
   sources), and the member with their `joined_at` (or absent). Discord
   errors here → `DiscordUnavailable`; nothing is written.
6. **Member checks 1–8** (§2.3). A refusal caused by Quack's own access is
   written to the moderation log in its own short transaction
   (`case_create_refused`, result `refused`) and returned; for honeypot
   cases the module logs `honeypot_case_failed` instead (§13.4.2). Other
   refusals are returned without a log entry.

**Inside one transaction** (READ COMMITTED; locks in the §4.12 order):

7. Reserve the operation key: `INSERT INTO operation_keys … ON CONFLICT DO
   NOTHING RETURNING …`. No row returned means a concurrent request with the
   same key committed first: roll back and go to step 2.
8. Honeypot cases only: `SELECT … FROM honeypot_settings WHERE guild_id = $1
   FOR SHARE`; the module must still be enabled with the same `version`,
   channel, and rule as when the hits were admitted, else
   `HoneypotPolicyChanged` (§13.4.2).
9. `UPDATE case_counters SET last_number = last_number + 1 WHERE guild_id = $1 RETURNING last_number`.
   This row lock serializes case creation per server until commit, so the
   count in step 11 sees every earlier committed case. Past the highest
   case number the counter's `CHECK` rejects the update
   (`quack_db::guilds::next_case_number` returns `DbError::Rejected`), and
   the engine answers `CaseNumbersExhausted` (§4.4).
10. `SELECT current_version, archived_at FROM rules WHERE guild_id = $1 AND id = $2 FOR SHARE`,
    then load that version's definition. Missing → `RuleNotFound`;
    archived → `RuleArchived`; a pinned version that differs from
    `current_version` → `RuleChanged`. Validate the context against this
    version (pure; skipped when it is the version step 3 used).
11. Count counted cases (§3.4) with the `cases_escalation` index; add one.
12. `select_level` (§3.4).
13. If the level has an action, run member checks 9 and 10 against the
    facts from step 5. A refusal rolls back and is returned; a check 9
    refusal (Quack's access) is also logged as in step 6.
14. If `replaces_case_id` is set: the referenced case must be in the same
    server, voided, and not already replaced (`cases_replaces` unique
    index). Otherwise `ReplacementInvalid`.
15. Insert the `cases` row with the decision (§4.4), `number` from step 9,
    `member_joined_at` from step 5, the scoped operation key and request
    hash, `context_revision = 1`, and `created_at = now()` captured once in
    Rust and reused below.
16. If the level has an action, insert an `actions` row (state `pending`).
    For a timeout, `timeout_ends_at = created_at + duration_seconds`,
    truncated to whole seconds (§5.2).
17. If the level sends a DM, insert a `dms` row (kind `case`, state
    `pending`).
18. Insert one `evidence` row per evidence source (state `saving`).
19. Insert the receipt's `messages` row (§11.7): private → kind
    `case_receipt`, `private = true`, the interaction token and its expiry;
    public → kind `case_receipt`, the channel, `message_id` NULL, and the
    interaction token, used only to answer the moderator privately once the
    receipt is posted or refused (§10.3). The dashboard creates no receipt.
20. Insert the moderation log entry `case_created` (§9) and, when the log
    channel is set, its `post_mod_log` job.
21. Enqueue jobs (§16): `send_dm` for the DM; `run_action` for the action
    (for a kick or ban whose level sends a DM, `run_at = created_at + 120 s`,
    moved earlier when the DM settles, §6.3); one `capture_evidence` per
    evidence row; `sync_message` for the receipt. Each enqueue notifies its
    job kind; PostgreSQL delivers the notifications on commit.
22. Commit. Every statement runs with `SET LOCAL statement_timeout = '2s'`
    and `SET LOCAL lock_timeout = '2s'`, and the whole transaction with a
    3-second deadline in Rust. A deadline before COMMIT is sent rolls back
    (`error.internal`); a deadline while COMMIT is outstanding is
    `error.unconfirmed`, and repeating the same operation key returns the
    result.

**After the transaction:**

23. Return `Applied::Created(CaseCreated { case, action, dm })`. For Discord
    callers the receipt's `sync_message` job, which the commit's
    notification wakes at once, is the only writer of the interaction's
    reply from here on (§10.3); the caller writes nothing more, so the two
    never race. The API returns the case.
24. Metrics: `quack.cases.created`, `quack.case.create.duration`,
    `quack.case.level`; refusals count `quack.cases.refused`. Span event
    `case.created`.

A failure in steps 7–22 (other than the handled ones) rolls back everything:
no case number is consumed, no job exists, nothing reaches Discord.

### 4.3 Operation keys

Every state-changing command carries an operation key, stored in
`operation_keys` in the same transaction as the write it protects. A key is
scoped to the server, the account acting, and the operation, so different
people and different operations never collide.

| Origin | `guild_id` | `account_id` | `operation` | `key` |
| --- | --- | --- | --- | --- |
| Slash command, button, select, modal | the server, or 0 in DMs | the clicker | command name or custom-ID code (`case_add`, `vdm`, `aam`, …) | the interaction ID (decimal) |
| `/case add` form | the server | the moderator | `case_add` | `draft:<draft_id>` (stable across pages, double clicks, and retries) |
| Dashboard write | the server, or 0 for `/api/me` routes | the signed-in account | `<METHOD> <route pattern>`, e.g. `POST /api/guilds/{guild_id}/cases` | the `Idempotency-Key` header |
| Honeypot | the server | 0 (Quack) | `honeypot_case` | `honeypot:<first message id>` |

- **Order.** Authenticate, resolve the target, and authorize the actor
  against its current state first; only then look up the key. A replay is
  answered only to someone allowed to see the result now.
- `request_hash` is SHA-256 of a canonical JSON encoding of the resolved
  request: path IDs (resolved, e.g. a case number becomes the case UUID),
  query parameters, and the validated, typed body for HTTP; decoded options,
  custom-ID arguments, and modal values for Discord.
- A replay with the same hash re-renders the **current** state of the
  stored result reference (`{"case_id": …}`, `{"action_id": …}`) for this
  actor with the original status code (HTTP replays add
  `Idempotency-Replayed: true`). A different hash returns `409
  idempotency_key_reused`.
- Two concurrent requests with one key: the second blocks on the primary
  key until the first commits, then replays its result.
- Two calls, both inside the write's transaction:
  `quack_db::operation_keys::reserve(tx, key, request_hash) -> bool`
  inserts the row with a placeholder result (`response_status = 0`,
  `response = '{}'`; `false` means another request committed the key
  first: roll back and replay), and `complete(tx, key, status, response)`
  fills in the real status and result reference before commit. A
  placeholder is therefore never visible to another transaction;
  `lookup` reads a stored operation.
- Rows expire after 24 hours (housekeeping deletes them). Case creation is
  also protected forever: `cases.operation_key` holds
  `<account_id>:<operation>:<key>` (unique per server) with
  `cases.request_hash`, so a replay after expiry still finds the case, and
  a different request with the old key is still `409`. Other operations are
  protected only for 24 hours; repeating one after that runs it again, which
  is harmless because each re-checks its preconditions (a second void is
  `409 case_already_voided`).
- An HTTP request whose write transaction has begun is allowed to finish
  even past the request deadline (§14.8). If the client gave up, repeating
  the same key returns the stored result. The dashboard reuses one key for
  every retry of one submit.
- Reads never use operation keys.

### 4.4 Case numbers and the decision

- `case_counters(guild_id, last_number)` is created with the server and
  only ever increments. Numbers start at 1, never repeat, and are not
  reused after voids. The importer sets `last_number` to the highest
  imported number. Numbers stop at 2^53 − 1 (`limits::MAX_CASE_NUMBER`,
  exact in JavaScript); past that, case creation is refused with
  `409 case_numbers_exhausted` (`error.case_numbers_exhausted`, §12.3,
  §14.6). Numbers are never reused.
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

The case also records `member_joined_at`, the member's Discord `joined_at`
when the case was created; a kick is only ever carried out against that
membership (§5.4).

The decision never changes. Corrections are a void plus a replacement case.

### 4.5 Case states

A case is **valid** or **voided** (`voided_at IS NULL` means valid).

| From | Event | To | Effects |
| --- | --- | --- | --- |
| (none) | Commit of §4.2 | valid | §4.2 steps 15–21. |
| valid | Staff void (§4.7) | voided | §4.7. |
| valid | Appeal accepted (§8.2) | voided | §4.7 with cause `appeal_accepted`. |
| voided | Any void | voided | Refused: `409 case_already_voided`. A replay of the same operation key returns the original result instead. |

There is no un-void.

### 4.6 Case context validation

`quack_core::case::validate_context(fields: &[Field], guild: GuildId, submitted: &[(String, ContextInput)]) -> Result<ContextValues, Vec<ContextProblem>>`,
where `fields` is `RuleDefinition::context_fields()` (the rule's fields, or
the single free-text `details` field when it has none, §3.2). Each
submitted answer is a `ContextInput`: `Text(String)` from a Discord modal
or select; from the API, `JsonNumber(String)` — the original text of a JSON
number, e.g. `2.50` — or `Json(Value)` for anything else.
`ContextInput::from_json_text(raw)` builds one from the answer's raw JSON
text (the API reads `context` as `key → RawValue`), so a JSON number is
never parsed through a binary float; passing a parsed `serde_json` number
as `Json` would lose decimals. Unknown keys → `unknown_field`. Duplicate
keys → `duplicate_field`. An omitted, empty, or whitespace-only optional value is
stored as absent.

| Kind | Accepted input | Stored as | Problems |
| --- | --- | --- | --- |
| `short_text` | String, 1–500 characters after trimming. | JSON string | `required`, `too_long`, `not_text` (API: a JSON value that is not a string) |
| `long_text` | String, 1–4,000 characters after trimming. | JSON string | `required`, `too_long`, `not_text` |
| `boolean` | API: JSON `true`/`false`. Discord: the select values `yes`/`no`. | JSON bool | `required`, `not_yes_no` |
| `number` | API: a JSON number or a string; Discord: text. Grammar: optional `-`, digits, optional `.` and digits; no exponent, no `+`, no spaces inside. Absolute value ≤ 10^15, ≤ 6 decimal places. Parsed from the original text into `quack_core::case::Decimal` (an `i128` count of millionths), never through a binary float. | Canonical decimal string (no trailing zeros, no leading `+`), e.g. `"2.5"`; the API returns it as a string | `required`, `not_a_number`, `number_out_of_range` |
| `message_link` | `https://discord.com/channels/<guild>/<channel>/<message>` (also `ptb.`, `canary.`, `www.` hosts and `discordapp.com`), each ID a positive decimal below 2^63, `guild` equal to this server, input at most 4,000 characters. Query and fragment are dropped. | Canonical `https://discord.com/channels/…` string | `required`, `not_a_message_link`, `other_server` |

`false` and `0` are answers, not absence. Validation reports every problem
at once; the message for each is in §12.5.

**Links inside text.** `quack_core::case::find_message_links(text)` returns
the Discord message links inside a text answer (HTTPS URLs with the hosts
above, trailing punctuation trimmed). They become evidence sources (§4.2
step 4, §4.9) when they point into this server; the text itself is stored
unchanged.

### 4.7 Voiding

`quack_engine::cases::void_case(principal, case_ref, reason, op_key)`:

1. Authorize (`moderate` + 2FA). Reason 1–1,000 characters.
2. One transaction (locks in the §4.12 order):
   1. Reserve the operation key.
   2. `SELECT … FROM cases WHERE guild_id = $1 AND id = $2 FOR UPDATE`.
      Voided already → `409 case_already_voided`.
   3. Set `voided_at = now()`, `voided_by`, `void_reason`, `void_cause`
      (`staff`, or `appeal_accepted` with the fixed reason
      `case.void.reason.appeal_accepted`, §8.2).
   4. Lock the original action (`FOR UPDATE`). By state:
      - `pending` → `cancelled`. Its job finds the state terminal and
        finishes. An original action never starts a new attempt on a voided
        case (§5.4 checks validity under the same case lock).
      - `running` → unchanged. The attempt in flight settles normally; if it
        does not end `done`, the action becomes `cancelled` (§5.3).
      - `done`, `failed`, `cancelled` → unchanged.
   5. **Undo.** If the original is a timeout or ban, no undo exists yet, at
      least one of its attempts may have reached Discord (§5.2), and, for a
      timeout, `timeout_ends_at` is in the future: insert the undo action
      (`remove_timeout` or `unban`, state `pending`, `requested_by` = the
      voider) and its `run_action` job. This covers an action that is done,
      one whose attempt is still in flight, and one whose earlier attempt's
      result was never learned. The undo runs only after any attempt in
      flight for the member has settled (§5.4), and it changes Discord only
      when Discord still shows this case's timeout or ban (§5.6). A kick is
      never undone.
   6. The case DM in state `pending` → `not_needed`. A DM in `sending` or a
      terminal state is left alone.
   7. An appeal in `pending` or `needs_information` → `closed` with appeal
      event `closed` (actor NULL, `closed_by_void = true`) and its
      `appeal_update` DM — unless this void is the appeal's own acceptance.
   8. Bump the case receipt and the appeal post (§10).
   9. Moderation log `case_voided` (detail: cause, cancelled action IDs,
      undo action IDs).
3. Metric `quack.cases.voided{quack.void.cause}`.

Undo actions are never cancelled by a later void: once queued, they run.

### 4.8 Replacement links

A wrong case is fixed by voiding it and creating a new case with
`replaces_case_id` pointing at it (§4.2 step 14). One voided case has at
most one replacement (`cases_replaces` unique index). A case never replaces
itself. The replacement may use a different rule or member. Staff views show
the link in both directions ("Replaced by case #13", "Replaces case #12").
Member views show a link only when both cases are about that member, so a
correction that moved a case to another member never reveals the other
member's case.

### 4.9 Edits

The decision never changes. Two edits exist, both on valid or voided cases:

- **Edit context.** Replaces the case context with new values validated
  against the case's own rule version (not the current one). The request
  carries the `context_revision` the editor was opened at; the transaction
  locks the case and compares it. A mismatch is `409 context_changed` and
  nothing is saved; otherwise `context_revision` increments. New message
  links (from `message_link` values and from links inside text answers) that
  are not yet evidence get evidence rows and `capture_evidence` jobs,
  subject to the per-case cap, in the same transaction. Existing evidence is
  never removed. Moderation log `case_context_edited` (detail: changed field
  keys). The receipt is bumped.
- **Add evidence.** Adds 1–10 message links (§7.1) under the case lock. A
  link already saved or being saved is reported as such; a link whose
  earlier capture failed is captured again (state back to `saving`, new
  job). Moderation log `evidence_added` (detail: count). The receipt is
  bumped.

### 4.10 Reading cases

Staff read by number (`/case view`, `GET …/cases/{case_ref}` where
`case_ref` is a number, a UUID, or a legacy ULID mapped as in §19.3).
Members read their own case by UUID or legacy ULID only: case numbers are
per server, and member routes have no server in the path.

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
| Case context | yes, every field | every field except `message_link` values, which show as `member.context.link_saved` |
| Evidence text, embeds summary, files | yes | yes: text, embed type and title, file names, files through the member file route |
| Evidence author, original message link, channel, embed URLs | yes | no |
| Moderator | yes | no |
| Source (`discord`, `dashboard`, `honeypot`, `imported`) | yes | no |
| Replacement links | yes | yes, case numbers only, and only when both cases are about this member (§4.8) |
| Appeal (state, statement, member replies, decision reasons) | yes | yes |
| Who decided the appeal | yes | no |
| Staff timeline (moderation log entries linked to the case) | yes | no |
| Member timeline (§4.11) | yes | yes |

Member reads never reveal whether a case exists in another member's name:
a wrong owner and a missing case are both `404 not_found`. Text that staff
or members typed (reasons, context, statements) is shown as written and can
itself mention people.

### 4.11 Member timeline

Built from immutable rows, so history never changes when an action is
retried or dismissed: the case row (added, voided with its reason), the
`action_attempts` rows (each attempt that ended `done` or `no_change`, and
each action's final failure from the moderation log entry `action_failed`),
the settled DM (`dms.settled_at`), and the appeal events. Items are rendered
with member-facing texts (§12.6, §12.7, §12.9) that never name staff.
Oldest first; the API returns the newest 100 items and
`timeline_truncated` when there are more.

### 4.12 Locks

Every transaction that changes moderation state takes its locks in this
order, skipping the ones it does not need, so transactions never deadlock:

1. `operation_keys` (the new row; blocks only a concurrent request with the
   same key)
2. `guild_settings` (rule creation, import, restore; settings writes)
3. Module settings rows (`honeypot_settings`, `ticket_settings`,
   `activity_log_settings`)
4. `case_counters` (case creation)
5. `rules` (`FOR SHARE` for case creation; `FOR UPDATE` for edit, archive,
   restore)
6. The member advisory lock `pg_advisory_xact_lock` (action starts only,
   §5.4)
7. `cases` (`FOR UPDATE`; when one transaction locks several, in ascending
   `id` order)
8. The case's children: `actions`, `dms`, `appeals`, `evidence`
9. `messages` (bumps)
10. `jobs` (enqueue upserts, and a handler's own fence and settle, which
    are always its transaction's last statements, §16.1)

The case row is the single lock point for a case: creating or settling an
action attempt, sending a case DM, voiding, every appeal transition, context
edits, and evidence admission all lock it first, then re-check the state
they depend on. All locks are transaction-scoped; nothing holds a lock while
calling Discord. A transaction that fails with a serialization failure or
deadlock is retried up to 3 times when it touches only the database.

---

## 5. Actions

An action is one Discord change Quack makes for a case. A case has at most
one **original** action (timeout, kick, or ban) and at most one **undo**
action per original (remove timeout for a timeout, unban for a ban). A kick
has no undo.

### 5.1 Kinds

Every attempt first reads the Discord state it is about to change, inside
the member-serialized section (§5.4), and acts only when the change is
still needed. Reads use REST, never a cache.

| `action_kind` | Purpose | Reads first | Then | Permission |
| --- | --- | --- | --- | --- |
| `timeout` | original | `GET /guilds/{g}/members/{m}` | Absent → `failed` (`member_not_in_server`). Administrator → `failed` (`member_is_administrator`). A current timeout ending at or after `timeout_ends_at` → `done` (`no_change`): Quack never shortens a timeout. Otherwise `PATCH /guilds/{g}/members/{m}` `communication_disabled_until = timeout_ends_at`. | Moderate Members |
| `kick` | original | `GET /guilds/{g}/members/{m}` | Absent, or a `joined_at` later than `cases.member_joined_at` (they left and rejoined, which is a new membership; for an imported case, whose join time is unknown, later than the case's `created_at`) → `done` (`no_change`, "already left the server"). Otherwise `DELETE /guilds/{g}/members/{m}`; 404 Unknown Member → `done` (`no_change`). | Kick Members |
| `ban` | original | `GET /guilds/{g}/bans/{m}` | A ban exists → `done`; `no_change` unless its reason carries this case's marker (then an earlier attempt of this action placed it). Otherwise `PUT /guilds/{g}/bans/{m}` with `delete_message_seconds`. Message deletion therefore happens at most once. | Ban Members |
| `remove_timeout` | undo of `timeout` | the guard, then `GET /guilds/{g}/members/{m}` | §5.6. | Moderate Members |
| `unban` | undo of `ban` | the guard, then `GET /guilds/{g}/bans/{m}` | §5.6. | Ban Members |

Every mutating call sends `X-Audit-Log-Reason`. For originals it is the
**case marker** `Quack case #<n>: <rule name>`, cut to 512 characters; for
undos, `Quack case #<n> voided`. Discord stores the ban's reason, so
`quack_core::action::parse_case_marker(reason) -> Option<u64>` (the number
after a leading `Quack case #`) tells which case placed a ban.

### 5.2 Idempotency and "may have applied"

- `timeout_ends_at = cases.created_at + timeout_seconds`, truncated to whole
  seconds, fixed at creation, and sent unchanged on every attempt. Discord's
  `communication_disabled_until` is compared at whole-second precision.
- A kick acts only on the membership the case was about (§4.4).
- A ban is placed only when no ban exists; an unban or timeout removal acts
  only on a ban or timeout Quack can show is this case's (§5.6).
- Therefore an attempt whose result is unknown (crash, deadline, 5xx,
  connection reset) is simply retried: the retry reads Discord first. No
  action is parked for review because its outcome was uncertain.

An attempt **may have applied** when it is still open (`finished_at` NULL)
or ended `done`, `unknown`, or `interrupted`.
`quack_core::action::may_have_applied(attempts: &[Option<AttemptOutcome>]) -> bool`
is this rule (`None` is an open attempt); the void (§4.7) and the failure
reason (§5.5) use it.

### 5.3 States

| `action_state` | Outcome shown | Meaning |
| --- | --- | --- |
| `pending` | in progress | Waiting for its first attempt or for the next retry (`next_attempt_at`). |
| `running` | in progress | An attempt has started (its `action_attempts` row is open) and has not settled. |
| `done` | done | Discord confirmed the change, or the desired state already held (`no_change`). |
| `failed` | failed | A definite refusal, or the retry budget ran out. `failure_reason` says which. |
| `cancelled` | cancelled | An original action that will not run (again) because its case was voided. Originals only. |
| `skipped` | left in place | An undo that did not change Discord; `skip_reason` says why (§5.6). Undos only. |

Transition table (`quack_core::action::transition(state, event)`; any pair
not listed is rejected as `InvalidTransition`). "Start" and "settle" are the
two transactions of §5.4.

| From | Event | To | Side effects (same transaction) |
| --- | --- | --- | --- |
| (none) | Case created with an action / undo requested | `pending` | Insert the `run_action` job. |
| `pending` | Start: original whose case is voided | `cancelled` | Bump receipt. |
| `pending` | Start: undo guard finds another valid case's action in force (§5.6) | `skipped` | `skip_reason = other_case_applies`, `skipped_because_case_id`; log `action_undo_skipped`; bump. |
| `pending` | Start: timeout end has passed | `failed` | `failure_reason = timeout_window_passed`; log; bump. |
| `pending` | Start: Quack-side pre-check refuses (§5.4) | `failed` | `failure_reason`; log `action_failed`; bump. |
| `pending` | Start: checks pass | `running` | Insert the open `action_attempts` row; `attempts + 1`; `last_attempt_at`. |
| `pending` | Case voided (original) | `cancelled` | §4.7. |
| `running` | Settle: attempt `done`, or `no_change` that is not the undo's "changed outside this case" result | `done` | `done_at`, `no_change`; log `action_done`; bump. |
| `running` | Settle: undo found the timeout or ban changed outside this case (recorded as attempt `no_change`) | `skipped` | `skip_reason = changed_outside_case`; log `action_undo_skipped`; bump. |
| `running` | Settle: attempt `refused` | `failed` | `failure_reason` from §5.5 with its details; log `action_failed`; bump. |
| `running` | Settle: original not `done`, case now voided | `cancelled` | Bump. |
| `running` | Settle: attempt `not_sent`, `unknown`, or `interrupted`, attempts left | `pending` | `next_attempt_at` (§5.7). |
| `running` | Settle: same, no attempts left | `failed` | `failure_reason = unconfirmed` when any attempt in this round may have applied, else `retries_exhausted`; log; bump. |
| `failed` | Staff retry (§5.8) | `pending` | Clear every failure field (`failure_reason`, `failure_permission`, `failure_http_status`, `failure_discord_code`); `attempts = 0` (new round); new job; log `action_retry_requested`; bump. |
| `failed` | Staff dismiss | `failed` | `dismissed_at`, `dismissed_by`; log `action_dismissed`; bump. |

`done`, `cancelled`, and `skipped` are terminal. `failed` is terminal unless
staff retry it. The start rows are tried in the order listed (the order of
§5.4 step 1.8), and so are the settle rows. A "changed outside this case"
result on an original, or with any outcome other than `no_change`, is an
`InvalidTransition`. A `refused` attempt becomes `failed` even when the
case was voided meanwhile.

### 5.4 Running an attempt

The `run_action` job handler (`quack_engine::actions::run`). Facts that
need Discord (the guild's owner, Quack's roles and permissions, the
member's cached roles) are gathered before any transaction.

1. **Start** (one short transaction, locks in the §4.12 order):
   1. `SELECT pg_advisory_xact_lock(hashtextextended('member:' || $guild || ':' || $member, 0))`.
      The lock lasts until this transaction ends, so nothing leaks if the
      handler stops.
   2. Lock the case, then the action (`FOR UPDATE`). (The job's fence runs
      as this transaction's last statement, §16.1.)
   3. Terminal state → finish the job (`Done`).
   4. An open attempt row means an earlier run stopped mid-attempt: settle
      it as `interrupted` through the normal settle rows (§5.3), then
      continue only if the action is now `pending` and due.
   5. `pending` with `next_attempt_at` in the future → `Retry` at that time.
   6. Another action of this member is `running` → `Defer(2 s)`. Together
      with step 1 this serializes all attempts for one (server, member): an
      undo never runs while an original's attempt is in flight.
   7. Kick or ban whose case DM is still `pending` or `sending`, less than
      120 s after the case was created → `Defer` until then (§6.3).
   8. Apply the start rows of §5.3: voided case (original), the undo guard
      (§5.6), the timeout end (`now() >= timeout_ends_at`), and the
      Quack-side pre-check: Quack in the server (`quack_not_in_server`),
      Quack holds the permission (`quack_missing_permission`), the member is
      not the owner (`member_is_owner`), and, when the member is in the
      server, their highest role is below Quack's (`quack_role_too_low`; not
      checked for `unban`).
   9. Otherwise open the attempt (`running`) and commit. The commit is the
      logical start: a void that commits later finds the action `running`.
2. **Read and act** (no locks; each Discord call has a 10-second deadline):
   the reads and calls of §5.1.
3. **Settle** (one short transaction): lock the case, then the action;
   check `state = 'running'` and that the open attempt is the one this run
   started (its number); the job's fence runs last. Any mismatch rolls back:
   the attempt was already settled by whoever holds the job now. Otherwise
   finish the attempt row, apply the settle row of §5.3, write the log
   entry, bump the receipt, and settle the job, all in this transaction.

A timeout `PATCH` that Discord refuses after `timeout_ends_at` has passed is
recorded as `timeout_window_passed`, not `discord_refused`.

Metrics: `quack.action.attempts{quack.action.kind,quack.action.attempt.outcome,quack.reason}`;
on reaching a terminal state `quack.actions.finished`; on `done`,
`quack.action.completion.duration` (from case creation, or from the undo
request); on a scheduled retry, `quack.action.retries`; on `skipped`,
`quack.action.undo.skipped`. Span event `action.attempt`.

### 5.5 Attempt outcomes and failure reasons

| Result of the attempt | `attempt_outcome` | Next |
| --- | --- | --- |
| The change call returned `Done` | `done` | `done` |
| The read showed the desired state already holds, or Unknown Member / Unknown Ban on the change call where §5.1 says so | `no_change` | `done` |
| The undo read showed a timeout or ban that is not this case's (§5.6) | `no_change` | `skipped` (`changed_outside_case`) |
| `Refused(r)` on a read or the change call (other than the cases above) | `refused` | `failed` with the reason below |
| A read failed, or `RetryAfter(d)` before the change call was sent: Discord changed nothing | `not_sent` | retry at `max(d, backoff)` |
| `Unknown(c)` on the change call | `unknown` | retry with backoff |
| Found open by the next run (§5.4 step 1.4) | `interrupted` | retry with backoff |

`action_failure` (typed enum `ActionFailure`; message ids in §12.6):

| Value | When |
| --- | --- |
| `quack_not_in_server` | Pre-check, or Discord 404 Unknown Guild (10004). |
| `quack_missing_permission` | Pre-check. Carries the permission. |
| `quack_role_too_low` | Pre-check. |
| `member_is_owner` | Pre-check. |
| `member_not_in_server` | Timeout: the read found the member absent. |
| `member_is_administrator` | Timeout: the member has Administrator, and Discord never times out administrators. |
| `timeout_window_passed` | The timeout's end passed before Discord applied it (§5.4). |
| `discord_refused` | Any other 4xx except 429, including 403 Missing Permissions (50013) after the pre-check passed (stale facts). Carries HTTP status and Discord error code. |
| `retries_exhausted` | The round's budget ran out and no attempt may have applied (every one was `not_sent`): Discord definitely did not apply it. |
| `unconfirmed` | The round's budget ran out and at least one attempt may have applied, or the importer found an attempt in flight (§19.9). Quack can't tell whether Discord applied it. |

### 5.6 The undo guard and provenance

An undo changes Discord only when Discord still shows this case's effect and
no other valid case wants it. Two checks, in order:

1. **Guard** (start transaction, under the member and case locks):

   ```sql
   SELECT c.id, c.number
   FROM actions a JOIN cases c ON c.guild_id = a.guild_id AND c.id = a.case_id
   WHERE a.guild_id = $1 AND a.member_id = $2
     AND a.kind = $3                        -- 'timeout' for remove_timeout, 'ban' for unban
     AND a.case_id <> $4                    -- the undo's own case
     AND c.voided_at IS NULL
     AND a.state = 'done'
     AND (a.kind <> 'timeout' OR a.timeout_ends_at > now())
   ORDER BY c.number DESC LIMIT 1;
   ```

   A row means another valid case's timeout or ban is in force: the undo
   becomes `skipped` (`other_case_applies`, `skipped_because_case_id`), shown
   as "left in place because case #N still applies". Only `done` actions
   count: a competing action that has not started will apply after the
   undo, and one in flight makes the undo wait (§5.4 step 1.6). Uses index
   `actions_member`.
2. **Provenance** (after start, from the REST read):
   - `remove_timeout`: no current timeout, or one that has ended → `done`
     (`no_change`). A timeout whose end differs from this case's
     `timeout_ends_at` → `skipped` (`changed_outside_case`): someone set a
     different timeout, so Quack leaves it. Equal → `PATCH …
     communication_disabled_until = null`.
   - `unban`: no ban (404 Unknown Ban) → `done` (`no_change`). A ban whose
     reason's case marker names a **voided** case of this member in this
     server that has a ban action → `DELETE /guilds/{g}/bans/{m}`.
     Otherwise (no marker, or a valid case) → `skipped`
     (`changed_outside_case`). Any voided case's marker is accepted, not
     only this case's, because Discord does not document whether banning an
     already-banned member replaces the stored reason; with the guard, a
     ban placed by Quack for a case that no longer counts is Quack's to
     remove.

A skipped undo is terminal. Nothing is lost: if the case that still applied
is voided later, its own undo removes the timeout or ban, because that case
now owns what Discord shows. A `changed_outside_case` undo is reported so a
moderator can check the member in Discord.

### 5.7 Retry policy

- Budget: 8 attempts per round (one first try plus seven retries). A staff
  retry starts a new round with a fresh budget; `action_attempts.number`
  keeps counting across rounds.
- Backoff after attempt n (n = 1…7): 5 s, 15 s, 1 min, 5 min, 15 min,
  1 h, 3 h (`RETRY_BACKOFF`; `MAX_ATTEMPTS = 8`). Total 4 h 21 min 20 s.
- `RetryAfter(d)`: the next attempt waits `max(d, backoff)`.
- A `Defer` (member busy, waiting for a DM) does not consume an attempt.

### 5.8 Staff controls

`quack_core::action::controls(action: &ActionView, case_valid: bool, now: OffsetDateTime) -> Controls`
decides which controls an action offers. `ActionView` is the action as the
write reads it under lock (kind, state, dismissed, failure reason,
`timeout_ends_at`, and for an original whether an undo exists);
`Controls { retry, dismiss, undo }` holds `Result<(), DomainError>` for
retry and dismiss and `Result<ActionKind, DomainError>` (the undo kind)
for undo, each `Err` being the refusal a write gets. Card and log-channel buttons are
rendered from it, and the write re-evaluates it inside its transaction
after locking the case and the action.

| Control | Allowed when | Actor checks (fresh facts) | Effect |
| --- | --- | --- | --- |
| Retry | `failed`, not dismissed. Original: case valid, reason not `timeout_window_passed`, and for a timeout `timeout_ends_at` in the future. Undo: always (its case is voided by definition). | Original: member checks 2, 4, 6, 7 with the retrying moderator as actor (and 3 for a timeout). Remove timeout: check 7 when the member is in the server. Unban: none (the member is usually absent). | New round: `pending` + job. |
| Dismiss | `failed`, not dismissed. | none | Sets `dismissed_at`; the action leaves the failed-actions list; history kept. |
| Undo (remove timeout / unban) | Original `done`, kind timeout or ban, no undo exists. Timeout: end in the future. | Remove timeout: check 7 when the member is in the server. | New undo action `pending` + job, `requested_by` = staff. §5.6 still applies. |

Refusals (each a typed outcome): `404 action_not_found` (no such action in
this server), `409 action_not_retryable`, `409 action_not_dismissable`, `409
case_voided` (retry of an original on a voided case), `422
timeout_window_passed`, `409 action_not_undoable`, and the member refusal
for a failed actor check.

Buttons carry the action ID and are labelled with its kind ("Retry ban",
"Dismiss unban"), so a card with a failed original and a failed undo is
never ambiguous. Staff find every failed action on the dashboard's failed
actions page (`/guilds/{guild_id}/failures`); the staff card of a case
with a failed action links there, and the log channel's `action_failed`
post carries a Retry button (§9.4).

### 5.9 What a void does at each action state

| Original action state | On void |
| --- | --- |
| `pending`, no attempt may have applied | `cancelled`. Never starts. |
| `pending` after an attempt that may have applied | `cancelled`; for a timeout or ban, the undo is queued. |
| `running` | Left to settle: `done`, or `cancelled` otherwise. For a timeout or ban, the undo is queued now and runs after the attempt settles. |
| `done` (ban; or timeout with its end in the future) | Undo queued. |
| `done` (timeout already ended; or kick) | Nothing. A kick can't be undone. |
| `failed` | Stays `failed`; retry is refused because the case is voided. For a timeout or ban, the undo is queued when an attempt may have applied. |
| `cancelled` | Nothing. |

| Undo action state | On void of its own case | On void of another case |
| --- | --- | --- |
| any | Unchanged: undo actions are never cancelled. | Unchanged; the guard re-evaluates when it runs. |

---

## 6. DMs

A DM is one direct message from Quack to a member. Each DM is a `dms` row
with its own outcome and is sent **at most once**: a DM whose result Quack
never learned is reported as "couldn't confirm" and never sent again.

### 6.1 Kinds

| `dm_kind` | Created when | At most | Content |
| --- | --- | --- | --- |
| `case` | A case is created at a level with `send_dm` | one per case | `dm.case` (§12.8), from the case row |
| `appeal_update` | A staff decision on an appeal (request information, accept, reject, close, reopen), or Quack closing the appeal because its case was voided | one per appeal event | `dm.appeal.*` (§12.8), from that appeal event |
| `ticket_closed` | A ticket finishes closing | one per ticket | `dm.ticket_closed` (§12.13) with the transcript file |

A case whose level has no DM has no `dms` row; every view reports its DM as
"not needed" (`dm.state.not_needed.level`).

### 6.2 States

| `dm_state` | Outcome shown | Meaning |
| --- | --- | --- |
| `pending` | in progress | Not yet sent. |
| `sending` | in progress | Quack opened the DM channel and recorded `first_attempt_at` before its first create call. |
| `sent` | done | Discord returned the message. `channel_id` and `message_id` set. |
| `failed` | failed | Discord definitely refused before any create call with an unknown result, or the DM channel could not be opened. `failure` set. |
| `unknown` | couldn't confirm | A create call's result was never learned and the nonce window (§10.4) has ended. Terminal: never sent again. |
| `not_needed` | not needed | The case was voided before sending started. |

| From | Event | To | Side effects (same transaction) |
| --- | --- | --- | --- |
| (none) | Created (§6.1) | `pending` | `send_dm` job. |
| `pending` | Case voided (case DMs only) | `not_needed` | Bump receipt. |
| `pending` | Start: DM channel opened; for a case DM, the case is valid (checked under the case lock) | `sending` | `channel_id`, `first_attempt_at`. Committed before the first create call. |
| `pending` | Opening the DM channel refused | `failed` | `failure`; log `dm_failed`; bump; wake a waiting kick or ban. |
| `pending` | Budget spent while still `pending` (never sent) | `failed` | `failure = discord_unavailable`; log; bump; wake. |
| `sending` | Create `Done` | `sent` | `message_id`, `settled_at`; log `dm_sent`; bump; wake. |
| `sending` | Create `Refused`, and no create call of this DM had an unknown result | `failed` | `failure`; log `dm_failed`; bump; wake. |
| `sending` | Create `RetryAfter` that does not fit in the window, and no create call had an unknown result | `pending` | Nothing was sent: the job retries after the delay. |
| `sending` | The window ended after a create call with an unknown result, or a create was refused after one; or a run finds the DM `sending` after the window | `unknown` | Log `dm_failed` (`unknown`); bump; wake. |

"Wake a waiting kick or ban" means: if this is a case DM and the case's
original action is a kick or ban in `pending`, move its `run_action` job's
`run_at` to now (§6.3). Every terminal transition, including the job's
finalization (§16.1), does this in its own transaction.

### 6.3 Ordering with kick and ban

For kick and ban the DM goes **before** the action so the member can still
receive it (after a kick or ban they often share no server with Quack). The
case transaction enqueues the action's job with `run_at = created_at +
120 s`; the DM's terminal transition moves it to now. The action's start
(§5.4) waits while the DM is `pending` or `sending`, but never past those
120 seconds, so a slow or stuck DM delays a kick or ban by at most two
minutes. A DM failure never blocks the action. For timeouts and warnings
both jobs are enqueued at once and run independently.

Every DM describes the case's **decision** ("The moderators decided to ban
you from the server"), not the action's later outcome, so it is correct
whichever order the two finish in and whether or not the action succeeds.
The member's case page shows the action's actual outcome.

### 6.4 Sending

The `send_dm` handler (`quack_engine::dms::send`):

1. Load the row. Terminal state → `Done`.
2. `pending`: `open_dm(member_id)` (`POST /users/@me/channels`, which is
   idempotent). `Refused` → `failed` with the reason from §6.5. `Unknown` or
   `RetryAfter` → retry the job (budget 3 attempts, backoff 5 s, 30 s).
   Then the start transaction: lock the case (case DMs), then the DM; case
   DM on a voided case → `not_needed`; otherwise `sending` with `channel_id`
   and `first_attempt_at = now()`; the job's fence (§16.1); commit.
3. `sending`: if `now() >= first_attempt_at + 60 s` (the window, §10.4) →
   `unknown`. Otherwise send.
4. Render the DM from its own record (the case row for `case`, the appeal
   event for `appeal_update`, the ticket and transcript for
   `ticket_closed`) and call `send(channel, msg, files, nonce)` with
   `enforce_nonce = true`. The nonce is the DM ID as 22 characters of
   base64url and never changes, so a repeat inside the window returns the
   first message instead of creating a second one.
5. Results, while inside the window: `Done` → `sent`. `Unknown` → remember
   that a create may have succeeded, wait 2 s, repeat step 4. `RetryAfter(d)`
   → wait `d` and repeat if that still fits in the window. `Refused` →
   `failed` when no create so far was unknown, else `unknown`. When the
   window would be exceeded: `unknown` if any create was unknown, else back
   to `pending` with the job retried after `d`.
6. Settle per §6.2 in one transaction with the job (§16.1).

The handler's deadline (§16.5) is longer than the window, so a run never
stops mid-window on its own. A run that crashes leaves the DM `sending`; the
next run resumes it with the same nonce while the window lasts and settles
`unknown` after it.

Metrics: `quack.dm.outcomes{quack.dm.kind,quack.dm.outcome,quack.reason}`,
`quack.dm.send.duration{quack.dm.kind}` (from opening the DM channel to the
settle). Span event `dm.outcome`.

### 6.5 Failure reasons

`dm_failure` (`DmFailure`):

| Value | Discord answer | Sentence fragment (§12.7) |
| --- | --- | --- |
| `dms_closed` | 403 Cannot send messages to this user (50007) | "because Discord didn't let Quack message them (their DMs may be closed, or they may have blocked Quack)" |
| `not_in_shared_server` | 50278 (no mutual servers) | "because they no longer share a server with Quack" |
| `discord_unavailable` | The DM channel could not be opened, or Discord kept rate limiting, within the budget, and nothing was sent | "because Discord wasn't available" |
| `discord_refused` | Any other 4xx except 429 | "because Discord refused the message" |

### 6.6 What a case DM contains

In order (exact text in §12.8): the server name; the rule name; the official
reason as a quote; the server's DM introduction (if set); every case context
field except `message_link` fields (booleans as Yes/No); "What happens" with
the decided action (timeout end time as a Discord timestamp); the case
number and time; appeal instructions with an **Appeal** button and an
**Open in dashboard** link when the case is appealable, or "This case can't
be appealed."; the server's DM footer (if set). It never contains evidence,
the moderator's identity, or internal details. The member's case page on
the dashboard shows the same facts, plus the action's outcome.

---

## 7. Evidence

Evidence is saved message snapshots plus their files, attached to a case.
Capture happens after the case is committed, as jobs, so creating a case
never waits on Discord fetches or file copies.

### 7.1 Sources and limits

| Source | Added by |
| --- | --- |
| The message an "Add case" message command was used on | The case transaction |
| `message_link` context values, and message links inside text answers (§4.6) | The case transaction, and context edits |
| Links given when creating a case on the dashboard | The case transaction |
| "Add evidence" (Discord button, dashboard) | Its own transaction, 1–10 links per call |

- An explicit link must point into this server (§4.6 `message_link` rules);
  otherwise `other_server`.
- One evidence row per `(case_id, message_id)`. Adding a message that is
  `saving` or `saved` again reports "already saved"; adding one whose
  capture `failed` captures it again (state back to `saving`, new job).
- At most 50 evidence rows per case, counted under the case lock in the
  transaction that adds them. A request that would exceed it is refused
  before any write (`422 evidence_limit_reached`, also for case creation
  with more than 50 sources).
- Each row starts `saving` with a `capture_evidence` job keyed
  `capture_evidence:<evidence_id>`. Views show "Saving evidence…" until it
  settles.

### 7.2 The capture job

`quack_engine::evidence::capture`:

1. Load the row; terminal state → `Done`.
2. If `added_by` is a staff member (not Quack), check from current facts
   (§2) that they can read the channel: View Channel and Read Message
   History, computed with overwrites; threads use the parent channel;
   private threads also need thread membership (`GET
   /channels/{thread}/thread-members/{user}`; 404 means not a member) or
   Manage Threads. No access → `failed` (`moderator_no_access`). Facts
   unavailable → retry; access is never assumed.
3. `fetch_message(channel_id, message_id)`. `Refused` 404 Unknown Message or
   Unknown Channel → `failed` (`message_deleted`); 403 → `failed`
   (`quack_no_access`); the message's server differs → `failed`
   (`other_server`); `Unknown`/`RetryAfter` → retry (budget 4, backoff 10 s,
   1 min, 5 min). Budget exhausted → `failed` (`discord_error`).
4. **Save the snapshot** (one transaction: lock the case, then the
   evidence row): author ID, content (first 4,000 characters), created and
   edited times, up to 10 embeds as `{type, title, description, url}`
   summaries (title ≤ 256, description ≤ 1,000 characters), and the
   message's files. A file is admitted while the message has fewer than 10
   admitted files and the case fewer than 100; every admitted file gets an
   `evidence_files` row (filename, content type or NULL, size, original URL,
   its Discord attachment ID); the rest are only counted in
   `evidence.files_not_saved`. Each file's first state is decided here:
   `link_only` (no evidence channel), `too_large`, `unsupported_type`, or
   `pending` (to copy).
5. **Copy** each `pending` file (§7.3), one file per transaction, so
   progress survives a crash. When the run's time slice ends (§16.1) with
   files left, return `Continue`.
6. When every file has left `pending`: `saved`, `saved_at`; bump the
   receipt. Metric `quack.evidence.captures{quack.outcome}`.

At exhaustion (§16.1 finalization) every file still `pending` becomes
`copy_failed`, and the row becomes `saved` if its snapshot was stored, else
`failed` (`discord_error`). "Snapshot stored" is
`evidence.message_created_at IS NOT NULL`: step 4 always sets it (every
Discord message has a creation time) and nothing else does, so step 4's
update is guarded on `message_created_at IS NULL` (stored once) and the
finalizer's on `IS NOT NULL`.

`evidence_state`: `saving` → `saved` | `failed`. A `saved` row may still have
individual files that could not be copied; each file has its own state.

`evidence_failure`: `message_deleted`, `moderator_no_access`,
`quack_no_access`, `other_server`, `discord_error`.

### 7.3 Copying files

When `guild_settings.evidence_channel_id` is set, the job copies each file:

- Eligible: size ≤ 10,485,760 bytes and a content type (lowercased, without
  parameters) starting with `image/`, `video/`, `audio/`, or equal to
  `text/plain` or `application/pdf`. A file with no content type is not
  eligible.
- Before uploading, re-check the evidence channel is staff-only (§13.1.3)
  from current facts. Not staff-only → `copy_failed` for every pending file
  of this item and `copy_skipped_not_staff_only = true` on the item (shown
  as `evidence.channel_not_staff_only`).
- Download only from `cdn.discordapp.com` or `media.discordapp.net` over
  HTTPS, at most 5 redirects within those hosts, 30-second deadline, stop at
  10,485,760 bytes.
- Upload as a plain message (no Components V2, no text) with one attachment
  `SPOILER_<filename>`, following §10.4: record `copy_started_at`, nonce =
  the file ID (22 characters), `enforce_nonce`. On `Done` store
  `copy_channel_id`, `copy_message_id`, `copy_attachment_id` → `copied`.
  Each file has a budget of 3 upload tries; a definite refusal or a spent
  budget → `copy_failed`.
- Metrics: `quack.evidence.files` (copies made), `quack.evidence.bytes`.

`file_copy_state`:

| Value | Meaning |
| --- | --- |
| `pending` | Not yet processed. |
| `copied` | A copy exists in the evidence channel. |
| `link_only` | No evidence channel is set; only the original link is kept. |
| `too_large` | Larger than 10 MiB. |
| `unsupported_type` | Not an eligible content type. |
| `copy_failed` | Download or upload failed, or the channel is not staff-only. |

### 7.4 Who sees evidence

- Staff see everything, including authors and original message links.
- The member sees their own case's evidence on the dashboard: message text,
  embed types and titles, file names, and files through
  `GET /api/me/cases/{case_id}/evidence/{file_id}`. They never see authors,
  original message links, embed URLs, or channel names.
- File routes answer `302` to a fresh Discord CDN URL. Quack finds it by
  fetching a message and locating the attachment by its ID in the response:
  the copy message when the file is `copied`, else (or when the copy is
  gone) the source message. When neither message has the attachment any
  more → `404 file_unavailable`. The CDN URL contains a channel ID (never a
  channel name); that is accepted.
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

The submit transaction locks the case first and checks these there, so a
submission and a void are ordered: an appeal is never created on a voided
case. The statement answers one fixed question (§12.9 `appeal.form.*`) and
is 1–4,000 characters. Ineligible → `409 appeal_exists`, `409 case_voided`,
or `422 appeal_not_allowed`.

### 8.2 States

| `appeal_state` | Meaning |
| --- | --- |
| `pending` | Waiting for staff. |
| `needs_information` | Staff asked the member for more information. |
| `accepted` | Staff accepted; the case is voided. Terminal. |
| `rejected` | Staff rejected; the case stands. |
| `closed` | Staff closed it, or Quack closed it because the case was voided (`closed_by_void`, terminal). |

Every transition is one transaction that locks the case, then the appeal
(§4.12), and re-checks the case's validity and the appeal's state and
`version` there.

| From | Event (`appeal_event_kind`) | Actor | To | Effects (same transaction) |
| --- | --- | --- | --- | --- |
| (none) | `submitted` | member | `pending` | Appeal row (version 1); event; the appeal's `appeal_post` row (§8.3); log `appeal_submitted`; bump receipt. |
| `pending` | `information_requested` | staff, reason | `needs_information` | Event; `appeal_update` DM; bump post; log. |
| `needs_information` | `information_provided` | member, text 1–4,000, the `version` they saw | `pending` | Event; bump post; log. |
| `pending` | `accepted` | staff, reason | `accepted` | Event (with the staff reason); void the case (§4.7, cause `appeal_accepted`, void reason `case.void.reason.appeal_accepted`); DM; bump post and receipt; log `appeal_accepted` and `case_voided`. |
| `pending` | `rejected` | staff, reason | `rejected` | Event; DM; bump post; log. |
| `pending`, `needs_information` | `closed` | staff, reason | `closed` | Event; DM; bump post; log. |
| `pending`, `needs_information` | `closed` | Quack, inside a direct void (§4.7) | `closed` | Event with `closed_by_void = true` and text `appeal.closed_by_void`; DM; bump post; log `appeal_closed` (actor NULL). |
| `rejected`, `closed` (not by void) | `reopened` | staff, reason; the case must be valid | `needs_information` | Event; DM (asks for information); bump post; log. |
| any other | any | | unchanged | `409 appeal_changed`, or `409 case_voided` when the case was voided. |

- Every staff decision requires a reason of 1–2,000 characters, stored on
  its event. In Discord the Accept and Reject buttons open a modal
  prefilled with default text (`appeal.reason.default.accept`,
  `appeal.reason.default.reject`).
- Every transition carries `expected_version`, staff and member alike; the
  row's `version` increments on each transition. A mismatch is `409
  appeal_changed`, so an old reply can never answer a newer information
  request.
- `decided_by`, `decided_at`, `decision_reason` mirror the latest staff
  decision for lists; the events are the history.
- Metrics: `quack.appeal.transitions{quack.appeal.transition}` (submissions
  included), `quack.appeal.decision.duration{quack.appeal.transition}`
  (submission to the first accept, reject, or close), `quack.appeals.open`
  (gauge). Span event `appeal.transition`.

### 8.3 The appeal post

Each appeal has exactly one tracked staff message (kind `appeal_post`,
§10), created with the appeal. Its channel is the server's
`appeal_channel_id`, or NULL while no appeal channel is set.

- Setting an appeal channel (settings or `/setup`) binds, in the same
  transaction, the post of every `pending` or `needs_information` appeal
  that has no channel or is `gone` to the new channel and bumps it.
- Changing the appeal channel moves the posts of open appeals: the old
  location is kept as a stale coordinate (§10.1) and its message is edited to
  `appeal.post.moved` without buttons. Posts of decided appeals stay where
  they are.
- Clearing the appeal channel leaves existing posts in place.

Content: `appeal.post.*` (§12.9): member, case number and rule, state,
statement (first 1,500 characters; the rest on the dashboard), the latest
decision and its reason, and buttons **Accept**, **Reject** (only while
`pending`), and **Open appeal** (dashboard link). Every button re-checks the
clicker's capability and the appeal version and answers privately.

### 8.4 Appeal DMs

Each staff decision (and Quack's automatic close) sends exactly one
`appeal_update` DM (§6), rendered from its own appeal event: the decision,
that event's reason as a quote, what happens next, a link to the member's
case page (`/guilds/{guild_id}/cases/{case_id}/appeal`), and for accepted
appeals of cases with a kick or ban, the server's rejoin invite when set
(read when the DM is sent). Because each DM renders its own event and shows
the event's time, two quick decisions produce two accurate DMs whatever
order they arrive in. DMs never name the staff member.

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
listed, so tokens and message content cannot enter by construction. The
typed links of an entry are all taken from one loaded record (the case, the
appeal, the action), never assembled from separate inputs, so they always
agree.

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
| `action_failed` | failed | case, member, action | `{case_number, kind, reason, permission\|null, http_status\|null, discord_code\|null, attempts}` (`reason` is an `ActionFailure`, including `unconfirmed`) | yes |
| `action_retry_requested` | ok | case, member, action | `{case_number, kind}` | yes |
| `action_dismissed` | ok | case, member, action | `{case_number, kind}` | yes |
| `action_undo_requested` | ok | case, member, action | `{case_number, kind}` (the undo's kind) | yes |
| `action_undo_skipped` | ok | case, member, action | `{case_number, kind, skip_reason: "other_case_applies"\|"changed_outside_case", still_applies_case_number\|null}` | yes |
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
| `module_disabled` | ok | — | `{module, reason: null\|"channel_deleted"\|"quack_left"}` | yes (not when `quack_left`) |
| `module_settings_changed` | ok | — | `{module, fields: [string]}` | yes |
| `quack_joined` | ok | rule (starter, first join only) | `{first_join}` | yes |
| `quack_left` | ok | — | `{}` | no |
| `channel_setting_cleared` | ok | — | `{setting: "log_channel"\|"appeal_channel"\|"evidence_channel"\|"ticket_entry_channel"\|"ticket_queue_channel"\|"honeypot_channel"\|"activity_log_route"}` | yes |
| `log_channel_failed` | failed | — | `{entry_id, reason: "not_staff_only"\|"discord_refused"\|"retries_exhausted", discord_code\|null}` (the first entry that could not be posted in this outage) | no |
| `ticket_opened` | ok | member, ticket | `{}` | yes |
| `ticket_closed` | ok | member, ticket | `{transcript_complete}` | yes |
| `honeypot_case_failed` | failed | member, rule | `{refusal}` (a `MemberRefusal`, `"rule_unavailable"`, `"policy_changed"`, or `"discord_unavailable"`) | yes |
| `legacy` | as imported | as imported | `{action, resource_type, resource_id, failure_reason}` (every imported Go row, §19.8) | no |

`actor_id` is the staff member for staff operations, the member for
`appeal_submitted`, `appeal_information_provided`, `ticket_opened` (and
`ticket_closed` when the member closed it), and NULL for Quack.

### 9.3 Append-only enforcement

Two triggers on `mod_log` call the shared `reject_change()` function (§15.3,
§15.6), so UPDATE, DELETE, and TRUNCATE fail with `insufficient_privilege`
(`mod_log is append-only`), even for the table owner:

```sql
CREATE TRIGGER mod_log_no_change BEFORE UPDATE OR DELETE ON mod_log
  FOR EACH ROW EXECUTE FUNCTION reject_change();
CREATE TRIGGER mod_log_no_truncate BEFORE TRUNCATE ON mod_log
  FOR EACH STATEMENT EXECUTE FUNCTION reject_change();
```

Entries are written in the same transaction as the change they describe.
Bot-caused refusals (`case_create_refused`) are written in their own
transaction because the change rolled back.

### 9.4 Posting to the log channel

- When an entry of a posted kind commits and `guild_settings.log_channel_id`
  is set, the same transaction inserts a `post_mod_log` job keyed
  `post_mod_log:<entry_id>` whose payload pins that channel. Changing the
  log channel later neither re-posts old entries nor sends a queued entry
  somewhere else: a job whose pinned channel is no longer the log channel
  finishes without posting.
- The job renders the entry (§12.10). Facts come from immutable records:
  the entry's `detail`, the case's rule version, the void reason, the appeal
  event. Only display names (members, the rule's current name in links) are
  read live.
- Before creating the message the job re-checks from current facts that the
  channel is staff-only (§13.1.3), then creates it following §10.4 with
  nonce = the entry ID in base64url (22 characters).
- Outcomes: `Done` → finished. 404 Unknown Channel → finished without
  posting; clear `log_channel_id` only if it still holds that channel
  (compare-and-clear) and log `channel_setting_cleared`. Not staff-only, 403,
  or another definite refusal → finished without posting; the setting is
  kept. `Unknown`, `RetryAfter`, 5xx → retry: budget 6, backoff 10 s, 30 s,
  2 min, 10 min, 1 h. After the nonce window a retry may create a second
  copy of the same post (§20.4). A `Done` sets
  `guild_settings.log_channel_posted_at = now()` in the job's settle
  transaction (no `version` bump, no `NOTIFY`). Every finish without a
  post writes `log_channel_failed` once per outage: only when the server's
  newest `log_channel_failed` entry
  (`quack_db::modlog::last_log_channel_failure`) is absent or older than
  `log_channel_posted_at`. With no successful post yet, the first failure
  is written and later ones are not until a post succeeds. The column is
  added by migration `0002_log_channel_posted_at.sql` (§15.10).
- An `action_failed` post carries a **Retry** button (`1:ar:<action>`) when
  `quack_core::action::controls` (§5.8) allows a retry at posting time.
  The post is not edited later; a click re-checks everything and answers
  privately.
- Metric `quack.modlog.posts{quack.outcome}` (`done` posted, `skipped`
  moved or no channel, `failed`).

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
posts, and the honeypot warning. Each is a `messages` row, and its
`sync_message` job is the only thing that writes it to Discord.

### 10.1 The model

| Column | Meaning |
| --- | --- |
| `kind` | `case_receipt`, `appeal_post`, `ticket_panel`, `ticket_post`, `honeypot_warning`. |
| `case_id`, `appeal_id`, `ticket_id` | The owner: exactly the one column its kind needs (case receipt → case, appeal post → appeal, ticket post → ticket); none for the one-per-server kinds (`ticket_panel`, `honeypot_warning`). Real foreign keys. |
| `channel_id`, `message_id` | Where it lives. `channel_id` is NULL only for an appeal post waiting for an appeal channel (§8.3); `message_id` is NULL until posted. |
| `generation` | Incremented whenever the message must be posted anew: a move, a repost after deletion, or reactivation. Write-backs are compared against it. |
| `create_started_at` | When the current create began (§10.4); NULL when no create is in progress. |
| `wanted_revision` | Incremented in the same transaction as any change the message shows (≥ 1). |
| `posted_revision` | The highest revision whose rendering is on Discord (0 to `wanted_revision`). |
| `wanted_at` | When `wanted_revision` last changed, for the sync lag metric. |
| `private`, `interaction_token`, `token_expires_at` | A private receipt is the interaction's ephemeral original response. It is only ever edited through its token (valid 15 minutes minus 60 seconds); there is no other way to write it. A public receipt keeps the moderator's token only for the fallback in §10.3. |
| `state` | `active` or `gone`. |
| `stale` | Earlier locations still to be cleaned up after moves: a JSON list of `{channel_id, message_id}`, deduplicated, at most 5; appending a sixth drops the oldest (that message is left as it was). |
| `attachment_id` | The file Quack attached (the ticket transcript), kept on later edits. |
| `last_error` | Machine code of the last failed sync, e.g. `http_403_50001`; never shown in Discord or on the dashboard. |

Uniqueness: one row per owner and kind; one row per `(guild_id, kind)` for
the one-per-server kinds.

### 10.2 Bumping

`quack_db::messages::bump(tx, guild, Owner)` where `Owner` is `Case(id)`,
`Appeal(id)`, `Ticket(id)`, or `Server(kind)`:

```sql
-- Owner::Case (likewise appeal_id for Appeal, ticket_id for Ticket)
UPDATE messages SET wanted_revision = wanted_revision + 1, wanted_at = now()
WHERE guild_id = $1 AND case_id = $2 AND kind = 'case_receipt' AND state = 'active'
RETURNING id;
-- Owner::Server(kind)
UPDATE messages SET wanted_revision = wanted_revision + 1, wanted_at = now()
WHERE guild_id = $1 AND kind = $2 AND case_id IS NULL AND appeal_id IS NULL
  AND ticket_id IS NULL AND state = 'active'
RETURNING id;
```

followed by enqueueing `sync_message` keyed `sync_message:<id>` (§16.3).
What bumps what:

| Change | Bumps |
| --- | --- |
| Action state change, DM settles, evidence settles, context edit, void, appeal submitted or decided, replacement created | the case's `case_receipt` |
| Appeal transition, appeal channel set or changed | the appeal's `appeal_post` |
| Ticket opened, closed, access fixed, transcript saved | the ticket's `ticket_post` |
| Ticket settings changed, tickets enabled or disabled | the server's `ticket_panel` |
| Honeypot case created, honeypot settings changed, honeypot enabled or disabled, an edit of the honeypot's rule | the server's `honeypot_warning` |

A settings transaction that chooses a destination for a `gone` row (or an
appeal post with no channel) reactivates it in the same transaction:
`state = 'active'`, the new `channel_id`, `message_id = NULL`,
`generation + 1`, and a bump.

### 10.3 The sync job

`quack_engine::messages::sync`, keyed per row, so one row never has two
syncs running:

1. **Read** (short transaction): lock the row (the job's fence runs last in every transaction, §16.1). `gone`
   → `Done`. `posted_revision = wanted_revision` and `stale` empty → `Done`
   (already current). Note `rev`, `generation`, `channel_id`, `message_id`.
   If a create is needed and `create_started_at` is NULL, set it to now.
2. **Render** from current data at send time (§12), never from the
   revision, so a later render always shows the latest state.
3. **Write:**
   - Private receipt: token valid → edit the original response. Token
     expired → `gone`. Nothing else is ever done with a private receipt.
   - Public receipt, first post: after the create succeeds and while the
     token is valid, also edit the moderator's original response to
     `case.receipt.posted`.
   - `channel_id` NULL → `Done` (an appeal post waiting for a channel).
   - Appeal and ticket posts: before any create or edit, re-check from
     current facts that the channel is still staff-only (§13.1.3). Not
     staff-only → write nothing, `last_error = "not_staff_only"`, finish; the
     next bump tries again.
   - `message_id` set → edit with bot credentials, keeping
     `attachment_id`.
   - `message_id` NULL → create it (§10.4) with nonce
     `<row id u22><generation, base 36>` (at most 25 characters): stable
     across revisions, new for each generation.
4. **Record** (short transaction): lock the row, then
   compare-and-set on the generation read in step 1:

   ```sql
   UPDATE messages
   SET posted_revision = GREATEST(posted_revision, $rev),
       message_id = COALESCE(message_id, $created_id), create_started_at = NULL,
       last_error = NULL, updated_at = now()
   WHERE id = $1 AND generation = $gen;
   ```

   No row updated means the message was moved while this sync ran: a
   message it created is appended to `stale` instead, so it is cleaned up.
5. **Clean up** each `stale` location: edit it to `ticket.panel.moved` or
   `appeal.post.moved` (without buttons), or delete it (honeypot warning,
   public receipts, ticket posts), treating 404 as done; remove it from the
   list.
6. If `wanted_revision > rev` now, repeat from step 1 (at most 3 loops per
   run; then `Continue`).

When the message or channel is gone (404 Unknown Message 10008, Unknown
Channel 10003, 403 Missing Access 50001):

| Kind | Action |
| --- | --- |
| `case_receipt` | Public receipt whose create was refused: while the token is valid it becomes a private receipt (`private = true`) and the moderator's ephemeral reply is replaced by the card with `case.receipt.post_failed`; otherwise `gone`. A posted receipt that disappears → `gone`. The case is unaffected. |
| `appeal_post` | Post again in the current `appeal_channel_id` (`generation + 1`); none set → wait with `channel_id` NULL. |
| `ticket_panel` | Post again in the current entry channel; channel reconciliation (§13.1.5) turns tickets off when the channel was deleted. |
| `ticket_post` | Post again in the current queue channel; none → `gone`. |
| `honeypot_warning` | Post again in the honeypot channel; reconciliation turns the honeypot off when the channel was deleted. |

A `MESSAGE_DELETE` for a tracked message (looked up by `(channel_id,
message_id)`) does the same in its own transaction, so a deleted panel,
post, or warning is posted again at once.

**Moved messages.** When settings move a tracked message to another channel
(the ticket entry channel, the honeypot channel, the appeal channel for
open appeals), the settings transaction appends the old location to
`stale`, sets the new `channel_id`, `message_id = NULL`, `generation + 1`,
and bumps. The sync posts in the new channel, then cleans up the stale
locations.

Other outcomes: `Unknown`, `RetryAfter`, 5xx → retry with backoff 2 s,
10 s, 1 min, 5 min, 30 min (6 attempts); creates follow §10.4. Exhausted →
`last_error` set and the job finishes; the next bump tries again. A definite
4xx not listed → `last_error`, finish.

A worker that outlives its lease could land an older edit after a newer
one; this needs a Discord response slower than the margin between the
handler deadline and the lease (§16.1), and the next bump corrects it.

Metrics: `quack.message.syncs{quack.message.kind,quack.outcome}`; on
success `quack.message.sync.lag{quack.message.kind}` (seconds from
`wanted_at` to now).

### 10.4 Creating messages safely

Discord's Create Message takes a `nonce` (at most 25 characters) and
`enforce_nonce`. Discord's documentation says that with `enforce_nonce` the
nonce "will be checked for uniqueness in the past few minutes" for the same
author, returning the earlier message instead of creating a new one, and
gives no exact window. Quack assumes **60 seconds**
(`quack_core::limits::NONCE_WINDOW`). Every message create follows one rule:

1. Before the first create call, record when the create started
   (`dms.first_attempt_at`, `messages.create_started_at`,
   `evidence_files.copy_started_at`, `tickets.welcome_started_at`) and fix
   its nonce, derived from the record's ID.
2. While less than 60 seconds have passed since that start, a create whose
   result was unknown may be repeated with the same nonce and
   `enforce_nonce = true`.
3. After that, the earlier create's outcome is unknown:
   - **Member DMs** settle `unknown`. Terminal; never resent (§6).
   - **Everything else** (public receipts, appeal posts, ticket posts,
     ticket panels, honeypot warnings, log channel posts, evidence copies,
     ticket welcomes) is created again with a new start time. A crash or
     lost answer at exactly the wrong moment followed by a long Discord
     outage can therefore produce one duplicate post (§20.4).

Channels and threads have no nonce, so they are made findable instead and
looked up before every create: ticket threads by their full name
(§13.2.3), setup channels by name, creation time, and Quack's own overwrite
(§13.1.4).

---

## 11. Discord surface

### 11.1 Commands

All commands are guild-install only (`integration_types = [0]`,
`contexts = [0]`). Descriptions and option descriptions are catalog
messages (§12.1). No command has a default member permission except
`/setup` (Manage Server, `0x20`).

| Command | Type | Options (name, type, required, constraints) | Capability | Reply |
| --- | --- | --- | --- | --- |
| `/case add` | slash subcommand | `member` USER, required; `rule` STRING, required, autocomplete, max 100; `private` BOOLEAN, optional | `moderate` | Private: the private receipt (the staff card, ephemeral). Otherwise an ephemeral confirmation, and Quack posts the public receipt in the channel. |
| `/case view` | slash subcommand | `number` INTEGER, required, min 1 | `moderate` | Ephemeral case card (§11.7). |
| `/case list` | slash subcommand | `member` USER, optional; `rule` STRING, optional, autocomplete (includes archived rules) | `moderate` | Ephemeral list, 10 per page, newest first. |
| `/case void` | slash subcommand | `number` INTEGER, required, min 1; `reason` STRING, required, 1–1,000 | `moderate` | Ephemeral result (§4.7). |
| `/case history` | slash subcommand | `member` USER, required | `moderate` | Ephemeral member history: totals and 10 cases per page. |
| `/appeals` | slash | none | `moderate` | Ephemeral list of `pending` appeals, oldest first, 10 per page, each with an **Open** button. |
| `/rules` | slash | none | `moderate` or `manage_rules` | Ephemeral list of active rules (name, levels summary) with a dashboard link per rule; 10 per page. |
| `/setup` | slash | none | `manage_settings` | Ephemeral guided setup panel (§11.8). |
| `/help` | slash | `topic` STRING, optional, choices `cases`, `rules`, `appeals`, `setup` | none | Ephemeral help text (§12.12). |
| Add case | message command | (the message) | `moderate` | Ephemeral rule picker, then the private receipt. The source message becomes evidence. |
| Add case | member command | (the member) | `moderate` | Ephemeral rule picker, then an ephemeral confirmation, and Quack posts the public receipt in the channel. |

Autocomplete for `rule` options answers from the rules cache only (§17.6):
case-insensitive substring match on the name and the description, at most
25 choices, the name (cut to 100 characters) as label, the rule UUID as
value. `/case add` offers active rules; `/case list` also offers archived
ones. It returns no choices when the interaction member lacks the
capability by the roles and permissions in the interaction payload and the
cached settings; 2FA is checked when the command runs. (Rule names are not
secret: members see them on their own cases.) A `rule` value that is not a
rule UUID (typed text that was not picked) is matched against active rule
names, case-insensitively and exactly; no match → `error.rule_not_found`.

### 11.2 Command registration

`quack commands sync` and `quack serve` (before connecting the gateway)
run the same function:

1. Build the definitions from `quack_discord::commands::definitions()`.
2. Scope: `QUACK_DEV_GUILD_ID` set → that server's commands; otherwise
   global commands.
3. `GET` the remote list. Canonicalize both sides (drop IDs, versions, and
   default fields; sort keys; keep option order).
4. Equal → keep the remote list. Different → one bulk overwrite
   (`PUT /applications/{app}/commands` or the guild variant) with every
   definition, which also removes Go-era commands (`/template …`, `/case
   failures|retry|dismiss|reverse|evidence|user`, "Add case for member",
   `/ui-preview`).
5. Store the command IDs from the list Discord returned (the `GET`, or the
   `PUT`'s response), keyed by `(type, name)`, for command mentions in text
   (`</case view:ID>`). Mentions are rendered only from typed catalog
   references, never from text a person typed.

A sync failure stops `serve` with a clear log line; `commands sync` exits 1.

### 11.3 Acknowledgement rules

Every interaction is acknowledged before any database or Discord REST call,
with the documented exception below. Work happens after the ack. The
capability is always checked before staff data is shown: from the
interaction payload and the caches when the response must be immediate, and
fully (with 2FA) before any write.

| Interaction | Initial response | Visibility of the final answer |
| --- | --- | --- |
| `/case add` when the rule (from the rules cache) has a required field, and the payload shows the `moderate` capability | Modal page 1, built from the rules cache (no stored values). | §11.6. |
| `/case add` otherwise | Deferred channel message, **ephemeral**. | Ephemeral. A public receipt is a separate message Quack posts (§11.7). |
| `/case view`, `/case list`, `/case void`, `/case history`, `/appeals`, `/rules`, `/setup` | Deferred channel message, ephemeral. | Ephemeral. |
| `/help` | Immediate message (type 4), ephemeral. Static text. | Ephemeral. |
| Message or member command "Add case" | Deferred channel message, ephemeral. | Ephemeral; a public receipt is a separate message (§11.7). |
| Autocomplete | Type 8 result from memory. | — |
| Button that opens a modal with no stored values (Void, Accept, Reject, Appeal, Continue) | Modal (custom ID, caches, or static text only). Void, Accept, Reject, and Continue refuse privately, with no modal, when the payload shows no `moderate`. | — |
| **Exception:** a modal prefilled with stored values: Edit context, Try again on a draft page, Set rejoin invite, Edit warning, and Add evidence (its title names the case number, which the `ea` custom ID doesn't carry) | Authorize from the payload, the caches, and the actor's 2FA row, and read the one row the modal shows, in a single query bound to the actor and server, within 300 ms; then the modal. Not authorized → ephemeral refusal and no modal. Deadline missed → ephemeral `interaction.busy`. | — |
| Button or select on an ephemeral message | Deferred update (type 6). | Edits that ephemeral message. |
| Button on a shared message (public receipt, log channel post, appeal post, ticket post, ticket panel) | Deferred channel message, ephemeral (type 5, flag 64). | New ephemeral message; the shared message changes only through tracked-message sync. |
| Modal submit | Deferred update when the modal came from an ephemeral message; otherwise deferred channel message, ephemeral. | Ephemeral. |

- The ack must go out within 2.5 seconds of the interaction's snowflake
  timestamp. `quack.interaction.ack.duration` measures receive-to-ack.
- Initial callbacks of type 5 and 6 carry only the ephemeral flag. The
  Components V2 flag is set on the edit or follow-up that carries the
  content.
- After the ack, the handler answers by editing the original response
  (`PATCH /webhooks/{app}/{token}/messages/@original`) or with a follow-up.
- In DMs the ephemeral flag is omitted (Discord rejects it there).
- Duplicate deliveries (same interaction ID within 15 minutes) are dropped
  by an in-memory set before the ack. Writes are protected again by
  operation keys (§4.3).
- Unknown or legacy custom IDs: see §11.5.

### 11.4 What is ephemeral

Everything staff-facing is ephemeral, including errors and refusals. The
only messages Quack posts publicly in server channels are: the **public
receipt** (§11.7: only its fixed fields); the ticket panel; the ticket
thread welcome; the honeypot warning; and posts in staff-only channels
(appeal posts, ticket posts, log channel posts, evidence copies, server
activity log posts). Every button on a public message re-checks the
clicker's capability and answers privately.

### 11.5 Custom IDs

Format: `1:<code>[:<arg>]…`, at most 100 characters. `1` is the scheme
version. UUIDs are 22 characters of unpadded base64url (`u22`). Snowflakes
and integers are decimal. `quack_discord::custom_id::CustomId` is an enum
with one variant per row; `encode` and `decode` are property-tested
(round trip, ≤ 100 characters for maximal arguments). A custom ID carries
no authority: every handler authorizes the clicker.

| Code | Component | Arguments | Behaviour |
| --- | --- | --- | --- |
| `cv` | Button "Open case" | case u22 | Show the case card. |
| `cvs` | Select "Open a case" (lists and history) | none; value: case u22 | Show the chosen case card. |
| `ce` | Button "Edit context" (staff card only) | case u22 | Exception read (§11.3): open the edit-context modal prefilled with the current context. More than 5 context fields: answer `case.context.too_many_fields` with a dashboard link instead. |
| `cem` | Modal | case u22, context revision | Save context (§4.9). |
| `ea` | Button "Add evidence" | case u22 | Open the add-evidence modal (one paragraph input, one link per line). |
| `eam` | Modal | case u22 | Add evidence (§7.1). |
| `ev` | Button "View evidence", pager | case u22, item number (1-based) | Show one evidence item. |
| `vd` | Button "Void case" | case u22 | Open the void modal (reason). |
| `vdm` | Modal | case u22 | Void (§4.7). |
| `hi` | Button "Member's case history", pager | member snowflake, page | Member history page. |
| `cl` | Pager | page, member snowflake or `0`, rule u22 or `0` | Case list page. |
| `ar` | Button "Retry {action}" | action u22 | Retry (§5.8). |
| `ad` | Button "Dismiss failed {action}" | action u22 | Dismiss (§5.8). |
| `au` | Button "Remove timeout" / "Unban" | action u22 (the original) | Ask to confirm. |
| `auc` | Button "Yes, remove timeout" / "Yes, unban" | action u22 | Undo (§5.8). |
| `rp` | Select (rule picker) | draft u22 | Value: rule u22. Set the draft's rule (§11.6). |
| `rpp` | Pager | draft u22, page | Rule picker page (25 rules per page). |
| `cd` | Modal (case form page) | draft u22, rule u22, rule version, page | Save the page (§11.6). |
| `cn` | Button "Continue" / "Fill in the form" | draft u22, rule u22, rule version, page | Open that page's modal (no stored values). |
| `cr` | Button "Try again" (a page with problems) | draft u22, rule u22, rule version, page | Exception read: re-open the same page prefilled with the values the moderator sent. |
| `cs` | Button "Try again" (saving failed) | draft u22 | Apply the completed draft again (§11.6). |
| `ap` | Button "Appeal case" (DM) | case u22 | Open the appeal modal. |
| `apm` | Modal | case u22 | Submit the appeal (§8.1). |
| `aa` | Button "Accept" | appeal u22, version | Open the accept modal (prefilled default reason). |
| `aam` | Modal | appeal u22, version | Accept. |
| `aj` | Button "Reject" | appeal u22, version | Open the reject modal. |
| `ajm` | Modal | appeal u22, version | Reject. |
| `ao` | Button "Open appeal for #{number}" | appeal u22 | Show the appeal card: the appeal post's content plus Accept and Reject while `pending`, and Open appeal (dashboard link). Other decisions are made on the dashboard. |
| `al` | Pager | page | `/appeals` page. |
| `rl` | Pager | page | `/rules` page. |
| `su` | Setup controls | area, operation | §11.8. |
| `to` | Button "Open a ticket" | none | Open a ticket (§13.2). |
| `tc` | Button "Close ticket" | ticket u22 | Close the ticket. |
| `tf` | Button "Restore private access" | ticket u22 | Fix ticket access. |

Longest form: `1:cd:<22>:<22>:<10>:<2>` = 64 characters.

**Legacy IDs** (messages posted by the Go version stay in Discord). Each
supported Go shape is decoded exactly; the ULID is mapped with §19.3 and
must name a record of this server.

| Legacy ID | Handled as |
| --- | --- |
| `appeal:submit:v1:<case ULID>` | `ap` for the case. |
| `appeal:accept:v1:<appeal ULID>`, `appeal:reject:v1:<appeal ULID>` | `ao` for the appeal (the card shows the current buttons). |
| `ticket:open:v1:` | `to`. |
| `ticket:close:v1:<ticket ULID>` | `tc` for the ticket. |
| `ticket:repair:v1:<ticket ULID>` | `tf` for the ticket. |
| `case:view:v1:<case ULID>`, `case:evidence:v1:<case ULID>`, `case:edit_context:v1:<case ULID>`, `case:void:v1:<case ULID>` | `cv` for the case. |
| `case:retry:v1:<execution ULID>`, `case:dismiss:v1:<execution ULID>` | `cv` for the case of the mapped action (view only). |
| `case:user_detail:v1:<member snowflake>` | `hi` page 1. |
| anything else, or a ULID that names nothing in this server | Ephemeral `interaction.legacy_button`. |

An undecodable `1:` ID answers `interaction.expired_button`.

### 11.6 Multi-page `/case add` drafts

A Discord modal holds at most 5 inputs. Rules with up to 10 fields use up to
2 pages. Drafts live in `case_drafts` so restarts and double clicks are
harmless. A draft belongs to one moderator and one server, holds the rule
version it was started with, and expires 15 minutes after it was created.

**Saving a page** (`cd` submit; defer ephemerally first). One transaction:

1. Authorize the moderator (fresh payload, 2FA).
2. `INSERT INTO case_drafts … ON CONFLICT (id) DO NOTHING` (the first page's
   submit creates the draft; the modal itself never needed a row), then
   lock it `FOR UPDATE`. It must belong to this moderator and server and not
   be expired (`case.draft.expired`).
3. The draft's `page` is past this page → this page was already saved (a
   double click): answer `case.draft.page_already_saved` with the button for
   the draft's current page, or the case if the draft was already used.
4. Store the submitted values for this page. Validate them (§4.6 against the
   draft's version). Problems → keep the page, commit, and answer
   `case.draft.page_invalid` with **Try again** (`cr`), which re-opens the
   page prefilled with what was sent. Valid → `page + 1`; commit.
5. More pages → `case.draft.page_saved` with **Continue** (`cn`).
6. All pages saved → apply the rule (§4.2) with operation key
   `draft:<id>` and the draft's version pinned. The case transaction deletes
   the draft. When applying fails without a case (`error.internal`,
   `error.discord_unavailable`, `error.unconfirmed`), the completed draft
   stays and the answer has **Try again** (`cs`), which applies it again;
   the operation key makes that safe even if the first try committed.
   `RuleChanged` → `case.draft.rule_changed` and the draft is deleted.

**From `/case add`:** when the rule needs input, the initial response is
modal page 1 with a fresh draft ID (UUIDv7) in its custom ID; the first
submit creates the row. The `cd` custom ID doesn't carry the member or the
receipt choice, so `quack-bot` remembers them in memory (by draft ID, 15
minutes, bounded) when it opens the modal; a first-page submit after a
restart finds nothing and answers `case.draft.expired`. Only page 1 may
create the row, and a form whose case was applied is marked used, so a
resubmitted page finds the case through `draft:<id>` instead of starting
a second draft. Otherwise the command defers and applies the rule
at once with operation key = the interaction ID.

**From the "Add case" message or member command:**

1. Defer ephemerally; authorize; insert a draft (member, the source message
   for the message command, `private = true` for the message command and
   `false` for the member command, no rule yet).
2. Show the rule picker: a string select of active rules (25 per page,
   pager `rpp`), or `case.picker.no_rules` when there are none. With exactly
   one active rule, continue at step 3 with it.
3. On select (`rp`, deferred update): set the draft's rule and version
   (`UPDATE … WHERE id = $1 AND actor_id = $2 AND rule_id IS NULL`), then
   fill empty `message_link` fields from the source message. Required fields
   left → answer `case.picker.fill_form` with **Fill in the form** (`cn`,
   page 1). Otherwise apply the rule with operation key `draft:<id>`.
4. Message command: the private receipt replaces the picker. Member command:
   the picker message becomes `case.receipt.posted`, and Quack posts the
   public receipt.

Expired drafts are deleted by housekeeping (§16.6).

### 11.7 The case card and receipts

The **staff card** renders every `case.card.*` line (§12.4). It is only
ever shown ephemerally: `/case view`, Open case, the history and list
selects, and the **private receipt** (the card as the interaction's original
response, kept current through the interaction token for 15 minutes, then
dropped; `/case view` shows it later). A private receipt is never posted any
other way.

The **public receipt** is a separate message Quack posts in the channel
with bot credentials (§10.4) and keeps current by editing. It shows only
these fixed fields, from `quack-text`'s `ReceiptView` projection:

- the case number and rule name, with "· Voided" once voided;
- the member;
- the official reason;
- the action line: the §12.6 action sentence without its failure reason or
  retry suffix (`Detail::Receipt` keeps only the first sentence, so an
  action Quack couldn't confirm shows "Quack couldn't confirm whether …"
  without the staff advice after it);
- for a failed or unconfirmed action, `case.receipt.staff_hint`, which
  points staff at Open case without saying what went wrong.

It never shows case context, evidence, the moderator, the void reason, the
appeal, the DM outcome, or any failure detail, and its only button is
**Open case** (`cv`), which authorizes the clicker and answers with the
staff card privately. When Quack can't post in the channel (a definite
refusal), the case stands and the moderator is told privately
(`case.receipt.post_failed`).

Staff card buttons, rendered from current state (each is re-authorized and
re-checked when clicked); at most 3 rows of 5:

| Button | Shown when |
| --- | --- |
| Retry {action} | `controls(action).retry`, per action (§5.8); first, so a failure's controls are easy to find |
| Dismiss failed {action} | `controls(action).dismiss`, per action |
| Edit context | always |
| Add evidence | always |
| View evidence | the case has evidence |
| Member's case history | always |
| Void case | case valid |
| Remove timeout / Unban | `controls(original).undo` |
| View failed actions (link to `/guilds/{guild_id}/failures`) | an action of the case is failed and not dismissed |
| Open case on dashboard | always (link) |

The card does not page: long context and evidence lists are cut (§11.11)
and continue on the dashboard.

### 11.8 Guided `/setup`

One ephemeral panel with one button per area. Each area shows its current
state and controls; every change is saved immediately through the same
engine calls as the dashboard and re-renders the panel with a result line.
Every click re-checks `manage_settings` + 2FA.

| Area (`su` area code) | State shown | Controls (`su` operation) |
| --- | --- | --- |
| Home (`home`) | One line per area below. | One button per area; "Open server settings" dashboard link. |
| Log channel (`log`) | Channel or "Not set". | Channel select (text channels) `ch`; "Create #moderation-log" `new`; "Stop posting log entries" `clr`. |
| Appeals (`ap`) | Appeal channel; rejoin invite set or not. | Channel select `ch`; "Create #appeals" `new`; "Stop posting appeals" `clr`; "Set rejoin invite" `inv` (modal `invm`, exception read); "Remove invite" `clrinv`. |
| Evidence copies (`ev`) | Channel or "Off". | Channel select `ch`; "Create #evidence" `new`; "Stop copying files" `clr`. |
| Tickets (`tk`) | On/off; entry and queue channels. | Entry channel select `entry`; queue channel select `queue`; "Create channels" `new` (creates `#support` and `#ticket-log`); "Turn on" `on`; "Turn off" `off`. |
| Honeypot (`hp`) | On/off; channel; rule; warning. | Channel select `ch`; "Create #honeypot" `new`; rule select `rule` (active rules without required fields, from the rules cache); "Clear rule" `clrrule` (only while off); "Edit warning" `warn` (modal `warnm`, exception read); "Turn on" `on`; "Turn off" `off`. |
| Server activity log (`al`) | On/off; channel (or "Several channels" when routes differ). | Channel select `ch` (routes every event to it); "Create #server-log" `new`; "Turn on" `on`; "Turn off" `off`. |

Channel selects offer text channels only. A chosen staff destination must
pass §13.1.3; created channels get the overwrites of §13.1.4.

### 11.9 Gateway intents and events

Intents (fixed at connect): `GUILDS`, `GUILD_MEMBERS` (privileged),
`GUILD_MODERATION`, `GUILD_MESSAGES`, `MESSAGE_CONTENT` (privileged). Both
privileged intents must be enabled in the developer portal, and an
application in 100 or more servers needs Discord's approval for them. If
Discord rejects the identify for disallowed intents (close code 4014),
`serve` stops with a log line naming the intents.

Quack runs one shard (`ShardId::ONE`). At startup it reads `GET
/gateway/bot`; when Discord recommends more than one shard (about 2,500
servers), `serve` stops with a clear error (§20.4).

| Event | Handling |
| --- | --- |
| `READY` | Mark the gateway connected; record Quack's own account ID; clear the member cache; mark every server unloaded until its `GUILD_CREATE`. Installed servers missing from READY's server list are treated as left (as `GUILD_DELETE`). Every `open` ticket gets `journal_gaps = true` when this READY follows an invalid session (events may have been missed). |
| `RESUMED` | Mark connected. Nothing was missed. |
| `GUILD_CREATE` | Update the guild cache; seed the member cache from the payload's members; bootstrap the server (§17.5); reconcile channel settings (§13.1.5); warm the rules and config caches; mark the server loaded. |
| `GUILD_UPDATE` | Update `guilds` (name, icon, owner, MFA level) when changed; server activity `server_changed`. |
| `GUILD_DELETE` | `unavailable = true` → mark the server unloaded (outage). Otherwise Quack left: §17.5 "Leaving a server". |
| `GUILD_ROLE_CREATE/UPDATE/DELETE` | Guild cache only. Deleted roles are ignored when reading staff roles. |
| `CHANNEL_CREATE/UPDATE` | Guild cache; server activity `channel_changed`. |
| `CHANNEL_DELETE` | Guild cache; clear settings that point at the channel (§13.1.5); server activity `channel_changed`. |
| `THREAD_DELETE` | A ticket's thread (`opening` or `open`) → close the ticket (§13.2.4). |
| `GUILD_MEMBER_ADD` | Member cache; server activity `member_joined`. |
| `GUILD_MEMBER_UPDATE` | Member cache. |
| `GUILD_MEMBER_REMOVE` | Member cache removal; server activity `member_left`. |
| `MESSAGE_CREATE` | Server activity message cache; ticket journal; honeypot hit. |
| `MESSAGE_UPDATE` | Server activity `message_edited`. |
| `MESSAGE_DELETE`, `MESSAGE_DELETE_BULK` | Server activity `message_deleted` / `messages_bulk_deleted`; a deleted tracked message is posted again (§10.3). |
| `GUILD_AUDIT_LOG_ENTRY_CREATE` | Ban (22) and unban (23) by someone other than Quack → server activity `member_banned` / `member_unbanned`. Needs View Audit Log; without it these two events are missing, and server health and the module's settings say so. |
| `INTERACTION_CREATE` | Interaction router (§11.3). |

Dispatch: the shard loop only updates caches and hands events off.
Interactions go to a task pool (at most 64 concurrent handlers). Other
events go to 4 queues of 1,024 events, chosen by server ID, so one server's
events stay in order. A full queue drops the event and counts
`quack.queue.dropped{quack.queue.name}`; a dropped event in a ticket thread
also sets that ticket's `journal_gaps`.

### 11.10 Caches

| Cache | Contents | Bound | Freshness | Size metric (`quack.cache.size{quack.cache.name}`) |
| --- | --- | --- | --- | --- |
| Discord guild cache (`twilight-cache-inmemory`, resource types `GUILD`, `CHANNEL`, `ROLE`, `USER_CURRENT`) | Server owner, MFA level, name, icon, channels with overwrites and parents, roles with positions and permissions | The servers Quack is in (one shard) | Gateway events; trusted for a server only while connected and loaded (§2) | `guilds` |
| Member cache (`quack-discord`) | (server, account) → roles, joined time, timeout end, bot flag, username, global name, avatar | 200,000 entries, LRU | Event-fed entries are exact while connected and loaded; REST-fed entries expire after 5 min; removed on `GUILD_MEMBER_REMOVE`; "not a member" is never cached | `members` |
| Rules cache (`quack-engine`) | Server → every rule's name, description, archived flag, and, for active rules, the current definition | One server's rules | Reloaded for a server on `NOTIFY quack_rules` | `rules` |
| Config cache (`quack-engine`) | Server → `guild_settings` and module settings | One per installed server | Reloaded on `NOTIFY quack_settings` | `config` |
| Server activity message cache (`quack-activity-log`) | (server, channel, message) → author, text (already redacted, §13.3.2), file names and sizes, embed types | 50,000 entries and 64 MiB estimated | 24 h; LRU | `activity_messages` |
| Interaction dedupe set (`quack-bot`) | Interaction IDs | 100,000 | 15 min | `interactions` |
| Directory cache (`quack-api`) | (server, account) → display data for the dashboard | 10,000 | 10 min | `directory` |
| Account servers cache (`quack-api`) | Session → the account's Discord server list | 1,000 | 60 s | `account_guilds` |

Every cache counts `quack.cache.lookups{quack.cache.name,quack.cache.result}`
and `quack.cache.evictions{quack.cache.name}`. Twilight's message, member,
presence, voice, emoji, sticker, and integration caches are off.

### 11.11 Rendering

`quack-text` returns a neutral `Msg`; `quack_discord::render` turns it into
a Components V2 message (flag `IS_COMPONENTS_V2`, no `content`, no embeds):

| `Msg` part | Components V2 |
| --- | --- |
| `Block::Heading(text)` | Text Display `### text`, on one line (line breaks in names become spaces) |
| `Block::Text(text)` | Text Display |
| `Block::Quote(text)` | Text Display, every line prefixed `> ` |
| `Block::Field { label, value }` | Text Display `**label:** value`; the label is escaped like typed text (rule fields name it), and a label ending in `?` gets no colon and its value on the next line |
| `Block::Subtext(text)` | Text Display `-# text` |
| `Block::Divider` | Separator |
| `Block::File(name)` | File component `attachment://<name>` (Discord shows a V2 message's attachments only through File components) |
| `Row::Buttons(Vec<Button>)` | Action Row, at most 5 buttons |
| `Row::Select(Select)` | Action Row with one select |
| `Msg::tone` | Container accent: none (neutral), green `0x3BA55C` (done), amber `0xFAA61A` (in progress or warning), red `0xED4245` (failed) |

- Everything sits in one Container. Each run of consecutive text blocks
  (Heading, Text, Quote, Field, Subtext) becomes **one** Text Display, its
  blocks joined by line breaks; a Divider or File ends the run. The budget
  (`quack_text::budget`) is at most 40 components counted with nesting
  (the Container, each Text Display run, Separator, File, Action Row,
  button, and select) and at most 4,000 characters of text counted after
  Markdown escaping. `budget::fit` enforces the text budget before
  rendering: it removes Subtext blocks, then cuts the longest Quote or Text
  block (with "…", never below 200 characters), then drops trailing Field,
  Text, and Quote blocks, never headings or rows, and adds the message's
  own note saying where the full details are (`Msg::when_trimmed`, such as
  `dm.case.trimmed`), else `render.trimmed` ("Quack shortened this message
  to fit in Discord."). Messages put the lines that must survive (case
  number, action, appeal instructions) first. Variable
  lists are paged or cut before rendering (10 list rows, 20 rows in an
  activity bulk-delete post with "and N more"), so the component budget
  always holds. A test renders maximal inputs for every message and checks
  Discord's limits (`budget::check_msg`, `budget::check_modal`).
- Dynamic text (names, reasons, context, statements, evidence text) is
  Markdown-escaped. Mentions render but never ping (`allowed_mentions`
  empty), except the ticket welcome, which pings its member.
- Button labels ≤ 80 characters, select option labels and descriptions
  ≤ 100, modal titles and Label labels ≤ 45. A modal always has at least one
  input; a rule with no fields has the free-text `details` input (§3.2).
- Discord timestamps use `<t:unix:f>` (full) and `<t:unix:R>` (relative).
- Evidence copies are plain messages with one attachment and no
  components (§7.3).

---

## 12. Message catalog

Every user-facing string, keyed by a stable message id. `quack-text`
implements the catalog with one function per message (a whole message such
as `quack_text::case::card` renders every `case.card` line), returning a
`Msg`, `Text`, `Modal`, or `String`, and has one `insta` snapshot per
message with every listed variant (the snapshot is named after the id, or
after the group id such as `case.card`). Functions take typed facts and
match every state, outcome, failure, and refusal exhaustively. The
dashboard renders API fields that carry these texts (`message`, `text`,
`outcome_text`, `state_text`) as given.

### 12.0 Notation

| Placeholder | Renders as (Discord) | Renders as (dashboard / API text) |
| --- | --- | --- |
| `{member}`, `{actor}`, `{author}` | Mention `<@id>` (never pings) | Display name (server nickname, else global name, else username), else "a member (ID {id})" (capitalized at the start of a sentence) |
| `{actor}` when NULL | "Quack" | "Quack" |
| `{case}` | `**[case #N](<dashboard case URL>)**` in log posts; `case #N` elsewhere | "case #N" |
| `{rule}` | Rule name in bold | Rule name |
| `{channel}`, `{thread}` | Channel mention `<#id>` | `#name`, else "an unknown channel (ID {id})" |
| `{role}` | Role mention `<@&id>` | `@name`, else "an unnamed role (ID {id})" |
| `{server}` | Server name in bold | Server name |
| `/command` | Clickable command mention `</case view:id>` when the command's ID is known, else `/case view` in code | "/case view" |
| `{ends_at}` | `<t:unix:f> (<t:unix:R>)` | "17 Nov 2026 at 14:05 UTC" |
| `{time}` | `<t:unix:R>` | "on 17 Nov 2026 at 14:05 UTC" |
| `{datetime}` | `<t:unix:f>` | "17 Nov 2026 at 14:05 UTC" |
| `{date}` | `<t:unix:d>` | "17 Nov 2026" |
| `{duration}` | `quack_text::fmt::duration`: the largest whole unit and, when not zero, the next smaller unit, spelled out: "45 seconds", "1 minute", "1 hour 30 minutes", "1 day 1 hour", "7 days", "28 days"; zero is "0 seconds" | same |
| `{last}` | `quack_text::fmt::last`, used after "the last": a single "1 unit" drops the number ("the last day", "the last hour"); anything else is `{duration}` ("the last 7 days", "the last 1 hour 30 minutes") | same |
| `{permission}` | Discord's English UI name: Timeout Members (the API's `MODERATE_MEMBERS`), Kick Members, Ban Members, Administrator, Manage Server, Manage Channels, Manage Messages, Manage Threads, View Channel, Send Messages, Send Messages in Threads, Read Message History, Attach Files, Embed Links, Create Public Threads, Create Private Threads, View Audit Log | same |
| `{permissions}` | `{permission}` names joined as "A", "A and B", or "A, B, and C" | same |

Plural variants are listed as "one: … / other: …" (and "zero: …" where zero
has its own words). Counts of things use thousands separators ("1,234").
Lists in a sentence are joined as "a and b" or "a, b, and c" (with "or"
where noted). A line starting with `>` is a quote block; `###` a heading;
`-#` subtext; `**Label:** value` a field. Text a person typed is escaped
on Discord so it can't become Markdown, a mention, or a link; the server's
own Markdown (DM introduction and footer, honeypot warning) keeps its
formatting but can't mention anyone. Consecutive text blocks of one message
render as one Text Display, one block per line. A message over Discord's
text budget drops subtext, cuts its longest quote or paragraph, then drops
trailing fields and paragraphs, and ends with the message's own note (where
the full details are) or `render.trimmed`. A field whose label ends in `?`
renders as `**Label?**` with its value on the next line.

### 12.1 Commands

| Id | Text |
| --- | --- |
| `command.case` | Add, find, and void moderation cases. |
| `command.case.add` | Add a case by applying a rule to a member. |
| `command.case.add.member` | The member this case is about. |
| `command.case.add.rule` | The rule to apply. |
| `command.case.add.private` | Show the receipt only to you. |
| `command.case.view` | Open a case by number and see its action and DM outcomes. |
| `command.case.number` | The case number shown on its receipt. |
| `command.case.list` | List recent cases. |
| `command.case.list.member` | Only show cases about this member. |
| `command.case.list.rule` | Only show cases under this rule. |
| `command.case.void` | Keep a case in the history and stop it from counting. |
| `command.case.void.reason` | Why you're voiding the case. The member can see this reason. |
| `command.case.history` | Show a member's case history. |
| `command.case.history.member` | The member whose cases to show. |
| `command.appeals` | Review appeals waiting for a decision. |
| `command.rules` | List active rules and open them on the dashboard. |
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
| `word.action.remove_timeout` | remove timeout |
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
| `button.open_in_dashboard` | Open on dashboard (generic; messages about one thing name it, such as `case.button.dashboard`) |
| `button.failed_actions` | View failed actions |
| `select.open_case` | Open a case (placeholder; each option is "#{number} · {rule name}", at most 25) |
| `render.trimmed` | Quack shortened this message to fit in Discord. |
| `level.summary.item` | at {from} {case / cases}: {outcome} ("at 3 cases: timeout for 1 hour"; "from case 3" would read as case #3) |
| `level.outcome.warning` | warning |
| `level.outcome.timeout` | timeout for {duration} |
| `level.outcome.kick` | kick |
| `level.outcome.ban` | ban |
| `level.outcome.ban_delete` | ban and delete the last {last} of messages |

`{action}` inside a sentence is `word.action.*`; sentences about an undo
name what it does instead ("removing the timeout", "the failed attempt to
unban the member", §12.6). Button labels
(`quack_text::word::retry_label` and `dismiss_label`) are "Retry
{action}" and "Dismiss failed {action}", except that a remove timeout is
"Retry removing timeout" and "Dismiss failed timeout removal".

### 12.3 Errors and refusals

Shown ephemerally in Discord; the same texts are the API `message` for the
matching error code (§14.6).

| Id | Text |
| --- | --- |
| `error.missing_capability.moderate` | Only moderators can do this. Ask a server administrator to check the moderator roles in the Quack dashboard's server settings. |
| `error.missing_capability.manage_rules` | To manage rules, you need Manage Server in Discord or a rules manager role in Quack. Ask an administrator to check your roles in Server Settings → Roles, or the rules manager roles in the Quack dashboard's server settings. |
| `error.missing_capability.manage_settings` | To change Quack's settings, you need Manage Server in Discord. Ask an administrator to check your roles in Server Settings → Roles. |
| `error.missing_capability.manage_staff_roles` | Only the server owner or a member with Administrator can change Quack's moderator roles. Ask one of them to change them in the Quack dashboard's server settings. |
| `error.mfa_required` | This server requires two-factor authentication for moderation. In Discord, open User Settings → My Account and turn on two-factor authentication. Then sign in to the Quack dashboard with Discord and try again. |
| `error.actor_not_in_server` | You're not in this server anymore. |
| `error.quack_not_in_server` | `access.quack_not_in_server` |
| `error.discord_unavailable` | Quack couldn't reach Discord, so nothing changed. Try again in a moment. |
| `error.internal` | Something went wrong inside Quack, so nothing changed. Try again in a moment. (Only for failures that changed nothing, such as a rollback; an outstanding commit is `error.unconfirmed`.) |
| `error.unconfirmed` | Quack couldn't confirm whether that was saved. Open it again to check before you try again. |
| `error.render_failed` | Case #{number} is saved. Quack couldn't show it here. Run /case view and enter {number} as the number. |
| `error.case_not_found` | Case #{number} wasn't found in this server. Check the number or use /case list. |
| `error.case_not_found_dashboard` | Case #{number} wasn't found in this server. Check the number or find the case in the case list. (dashboard pages) |
| `error.action_not_found` | Quack couldn't find that action in this server. Open the case again to see its actions. |
| `error.context_changed` | Someone else changed this case's context. Open it again to see their changes before you save. |
| `error.rule_changed` | The rule {rule} changed while you were adding the case. Run /case add again to use the new version. |
| `error.rule_not_found` | That rule doesn't exist anymore. Choose another rule. |
| `error.rule_archived` | The rule {rule} is archived, so it can't be applied. Choose an active rule. |
| `error.case_already_voided` | Case #{number} is already voided and no longer counts. |
| `error.case_voided` | Case #{number} is voided. Quack won't retry its {timeout / kick / ban}. (an undo: "Quack won't retry this action.") |
| `error.action_not_retryable` | {The timeout / The kick / The ban / Removing the timeout / Unbanning the member} can no longer be retried. Open the case to see its current outcome. |
| `error.action_not_dismissable` | {The timeout / kick / ban failure; The failed attempt to remove the timeout / unban the member} can no longer be dismissed. Open the case to see its current outcome. |
| `error.action_not_undoable` | kick: A kick can't be undone. Open the case to see what happened. / timeout, ban: This {timeout / ban} can't be undone from the case now. Open the case to see its current outcome. / an undo: This action can't be undone from the case. Open the case to see its current outcome. |
| `error.timeout_window_passed` | The timeout's end time has passed, so Quack won't retry it. Check {member}'s case history before adding another case. |
| `error.appeal_not_found` | Quack couldn't find this appeal. Open the case, or use /appeals to see appeals waiting for a decision. |
| `error.appeal_not_found_dashboard` | Quack couldn't find this appeal. Open the case, or go back to the appeals list. (dashboard pages) |
| `error.appeal_changed` | This appeal changed since you opened it. Open it again to see the latest update. |
| `error.evidence_limit_reached` | Case #{number} would have more than 50 evidence items. Add fewer message links. |
| `error.too_many_links` | Add at most 10 message links at a time. |
| `error.no_links` | No evidence was added. Paste at least one message link and try again. |
| `error.reason_empty` | Add a reason before you save. The member can see it. (void, appeal decisions) |
| `error.reason_too_long` | This reason is too long. Shorten it to {max} characters or fewer. |
| `error.appeal_text_empty` | Write what you want the moderators to know before you send it. (appeal, added information) |
| `error.appeal_text_too_long` | Your message is too long. Shorten it to {max} characters or fewer. |
| `error.replacement_invalid` | Case #{number} can't be replaced. Choose a voided case in this server that doesn't already have a replacement. |
| `error.honeypot_policy_changed` | The honeypot settings changed, so Quack didn't add a case for this post. |
| `error.case_numbers_exhausted` | No case was added. This server has reached the highest case number Quack can store. |
| `interaction.busy` | Quack is busy right now. Try again in a moment. |
| `interaction.expired_button` | This button no longer works. Run the command again, or use the dashboard. |
| `interaction.legacy_button` | This button is from an older version of Quack and no longer works. Run the command again, or use the dashboard. |

`error.action_not_found`, `error.context_changed`,
`error.case_numbers_exhausted`, and `error.honeypot_policy_changed` are
text functions of their own for the engine's own refusals (§14.6: API codes
`action_not_found`, `context_changed`, `case_numbers_exhausted`; the
honeypot job records `policy_changed` on the hit); they are not
`quack_core::DomainError` variants.

**Domain errors and their facts.** A `quack_core::DomainError` carries no
names or numbers, so callers pass `quack_text::error::ErrorFacts` with what
they know: the scene (staff, adding a case, editing a rule, or a member's
own case or appeal), the case number, the rule's name, the action's kind,
and the member. `quack_text::error::for_domain` picks the text (snapshot
`error.for_domain`, every error in every scene with all facts and with
none):

| Domain error | Facts used | Text | Fallback when a fact is absent |
| --- | --- | --- | --- |
| missing capability | — | `error.missing_capability.*` | — |
| MFA required | — | `error.mfa_required` | — |
| member refused | member, action | `refusal.*` | see below |
| case not found | scene, case number | member scene: `api.not_found`; else `error.case_not_found` | Quack couldn't find that case in this server. |
| case already voided | case number | `error.case_already_voided` | This case is already voided and no longer counts. |
| case voided | scene, case number, action | member scene: `appeal.submit.voided`; else `error.case_voided` (needs both number and action) | member scene: This case was voided and no longer counts. You can still review it on the dashboard. Else: This case is voided. Quack won't retry its action. |
| replacement invalid | case number | `error.replacement_invalid` | That case can't be replaced. Choose a voided case in this server that doesn't already have a replacement. |
| rule not found | — | `error.rule_not_found` | — |
| rule archived | rule | `error.rule_archived` | That rule is archived, so it can't be applied. Choose an active rule. |
| rule changed | scene, rule | adding a case: `error.rule_changed`; any other scene: `api.rule_changed` | adding a case: The rule changed while you were adding the case. Run /case add again to use the new version. |
| rule name taken | rule | `api.rule_name_taken` | Another active rule already has this name. Choose a different name. |
| too many rules | — | `api.too_many_rules` | — |
| rule used by honeypot | — | `api.rule_used_by_honeypot` | — |
| required field on honeypot rule | — | `api.required_field_on_honeypot_rule` | — |
| action not retryable | action | `error.action_not_retryable` | This action can no longer be retried. Open the case to see its current outcome. |
| action not dismissable | action | `error.action_not_dismissable` | This failure can no longer be dismissed. Open the case to see its current outcome. |
| action not undoable | action | `error.action_not_undoable` | This action can't be undone from the case. Open the case to see its current outcome. |
| timeout window passed | member | `error.timeout_window_passed` | The timeout's end time has passed, so Quack won't retry it. |
| appeal not found | scene | member scene: `api.not_found`; else `error.appeal_not_found` | — |
| appeal changed | scene | member scene: `appeal.information.outdated`; else `error.appeal_changed` | — |
| appeal exists | case number | `appeal.submit.exists` | `api.appeal_exists` |
| appeal not allowed | case number | `appeal.submit.not_allowed` | `api.appeal_not_allowed` |
| evidence limit reached | case number | `error.evidence_limit_reached` | This case would have more than 50 evidence items. Add fewer message links. |
| too many links | — | `error.too_many_links` | — |
| settings changed | — | `api.settings_changed` | — |
| ticket not open | — | `ticket.close.not_open` | — |

Member refusals when applying a rule (`refusal.*`; the first sentence is
always "No case was added."):

| Id | Text |
| --- | --- |
| `refusal.quack_not_in_server` | No case was added. `access.quack_not_in_server` |
| `refusal.actor_not_in_server` | No case was added. You're not in this server anymore. |
| `refusal.member_not_in_server` | No case was added. {member} isn't in this server. Choose a current member. |
| `refusal.member_is_self` | No case was added. You can't add a case about yourself. Ask another moderator. |
| `refusal.member_is_bot` | No case was added. {member} is a bot, and cases are only for people. |
| `refusal.member_is_owner` | No case was added. {member} owns this server. Quack doesn't add cases about the server owner. |
| `refusal.member_role_too_high` | No case was added. `access.your_role_too_low` |
| `refusal.quack_role_too_low` | No case was added. `access.quack_role_too_low` Then run /case add again. |
| `refusal.quack_missing_permission` | No case was added. This rule calls for a {timeout / kick / ban}. `access.quack_role_needs` (staff) Then run /case add again. |
| `refusal.member_is_administrator` | No case was added. This rule calls for a timeout, and Discord doesn't allow timeouts for members with Administrator. |

The action named is the level's, else the one the missing permission is
for (Timeout Members, Kick Members, Ban Members). When the member isn't
known (an API error raised before the member was), `{member}` reads "The
member" at the start of a sentence and "the member" inside one. The log
channel's `log.case_create_refused` and `log.honeypot_case_failed` reuse
these reasons in the third person (§12.10).

Permission and role problems (`access.*`, `quack_text::access`): one
sentence each, shared by refusals, action failures, setup, modules,
evidence, and the log channel, with Discord's English UI names and the
settings page that fixes it. Quack's role and the reader's own role are
always named apart. "Staff" readers are asked to go to an administrator;
"administrator" readers (setup) are told what to change.

| Id | Text |
| --- | --- |
| `access.quack_role_needs` | staff: Quack needs {permissions}. Ask an administrator to grant {it / them} to Quack's role in Server Settings → Roles. / administrator: In Server Settings → Roles, give Quack's role {permissions}. |
| `access.quack_channel_needs` | staff: Quack needs {permissions} in {channel}. Ask an administrator to allow {it / them} for Quack in the channel's Edit Channel → Permissions. / administrator: In {channel} → Edit Channel → Permissions, allow {permissions} for Quack. |
| `access.quack_role_too_low` | Quack's highest role must be above {member}'s highest role. Ask an administrator to move Quack's role up in Server Settings → Roles. |
| `access.your_role_too_low` | Your highest role must be above {member}'s highest role. Ask a moderator whose highest role is above {member}'s. |
| `access.quack_not_in_server` | Quack is no longer in this server. Ask an administrator to add Quack again. |

### 12.4 Case card and receipt

The staff card (and the private receipt, which is the card) uses these
lines, in this order; lines whose condition is false are omitted. What
staff act on comes before the case context, so trimming a long card drops
context first; a trimmed card ends with `case.card.trimmed`.

| Id | Text | Condition |
| --- | --- | --- |
| `case.card.heading` | ### Case #{number} · {rule} | valid |
| `case.card.heading_voided` | ### Case #{number} · {rule} · Voided | voided |
| `case.card.member` | {member} · level {level_index} (from a case count of {from}) · case count {case_count} | not imported |
| `case.card.member_imported` | {member} · imported from Quack v4 | imported |
| `case.card.reason` | > {reason} | always |
| `case.card.action` | **Action:** {action sentence, §12.6} | not imported |
| `case.card.undo` | **Undo:** {undo sentence, §12.6} | an undo exists, not imported |
| `case.card.controls.retry_dismiss` | To try again, select {retry label}. To take it off the failed actions list, select {dismiss label}. | per action, both buttons shown |
| `case.card.controls.retry` | To try again, select {retry label}. | per action, only Retry shown |
| `case.card.controls.dismiss` | To take this failure off the failed actions list, select {dismiss label}. The case keeps its history. | per action, only Dismiss shown |
| `case.card.dm` | **DM:** {DM sentence, §12.7} | not imported |
| `case.card.voided` | Case #{number} was voided. It no longer counts. {actor} voided it {time}. | voided |
| `case.card.void_reason` | > {void_reason} | voided |
| `case.card.replaces` | Replaces case #{other}. | replaces another |
| `case.card.replaced_by` | Replaced by case #{other}. | has a replacement |
| `case.card.appeal.pending` | **Appeal:** Waiting for a decision. | appeal pending |
| `case.card.appeal.needs_information` | **Appeal:** Waiting for {member} to reply. | |
| `case.card.appeal.accepted` | **Appeal:** Accepted. | |
| `case.card.appeal.rejected` | **Appeal:** Rejected. The case still counts. | |
| `case.card.appeal.closed` | **Appeal:** Closed. | |
| `case.card.evidence.saving` | **Evidence:** Saving evidence… | any item `saving` |
| `case.card.evidence.saved` | **Evidence:** one: Quack saved 1 evidence item. / other: Quack saved {count} evidence items. | all items settled, none failed |
| `case.card.evidence.partial` | **Evidence:** Quack saved {saved} evidence {item / items}. It couldn't save {failed}. Select View evidence to see why. (none saved: "Quack couldn't save {failed} evidence {item / items}. Select View evidence to see why.") | none saving, some failed |
| `case.card.context_heading` | **Case context** | the case has context |
| `case.card.context_line` | **{label}:** {value} (text cut to 300 characters with "…"; booleans Yes/No; numbers as written; a message link as a link to the message) | per answered field |
| `case.card.imported_action` | -# The original Quack v4 action was {v4_action}. Quack took no action for this case. | imported |
| `case.card.footer` | -# Added by {moderator} {time} | moderator set |
| `case.card.footer_honeypot` | -# Added by Quack {time} after a post in the honeypot channel | honeypot |
| `case.card.footer_imported` | -# Imported {time}. Original moderator: {v4_moderator} ("unknown" when v4 didn't record one) | imported |
| `case.card.trimmed` | Open this case on the dashboard for the full details. | the card was shortened to fit |

An imported case's card has no action, undo, or DM line: Quack took no
action for it, and `case.card.imported_action` says what v4 did.

The public receipt (§11.7) uses only:

| Id | Text | Condition |
| --- | --- | --- |
| `case.receipt.heading` | ### Case #{number} · {rule} | valid |
| `case.receipt.heading_voided` | ### Case #{number} · {rule} · Voided | voided |
| `case.receipt.member` | {member} | always |
| `case.receipt.reason` | > {reason} | always |
| `case.receipt.action` | **Action:** {action sentence, §12.6, without `{reason}`, the check, the retry time, or the dismissed suffix} | always |
| `case.receipt.staff_hint` | -# Staff: select Open case to see the outcome privately. | the action failed or couldn't be confirmed |

For an unconfirmed action the receipt shows only the first sentence of
`action.unconfirmed` ("Quack couldn't confirm whether Discord banned
{member}."), without the staff instruction to check.

Buttons (labels), in this order, so a failed action's controls come
first: `case.button.retry` "Retry {action}" ("Retry removing timeout"),
`case.button.dismiss` "Dismiss failed {action}" ("Dismiss failed timeout
removal"; "Dismiss ban" alone would read as removing the ban), per action
that allows them; `case.button.edit_context` "Edit context",
`case.button.add_evidence` "Add evidence", `case.button.view_evidence`
"View evidence" (when the case has evidence), `case.button.history`
"Member's case history", `case.button.void` "Void case" (when not voided),
`case.button.remove_timeout` "Remove timeout", `case.button.unban`
"Unban", `button.failed_actions` (when an action needs a decision),
`case.button.dashboard` "Open case on dashboard". The public receipt's only button
is `button.open_case`.

| Id | Text |
| --- | --- |
| `case.receipt.posted` | Case #{number} is saved. Quack posted the receipt in this channel. |
| `case.receipt.post_failed` | Case #{number} is saved. Quack couldn't post the receipt in this channel, so here it is privately. |
| `case.receipt.replayed` | Case #{number} is already saved. Run /case view and enter {number} as the number to see it. |
| `case.saved_unconfirmed` | Quack couldn't confirm whether the case was saved. Select Try again. If the case was already saved, Quack will open it. (The button reuses the operation key, §11.6.) |

### 12.5 Adding a case: picker, form, drafts, context problems

| Id | Text |
| --- | --- |
| `case.picker.prompt` | Choose a rule for the case about {member}. (with the rule select, 25 rules per page, each option the rule's name and description, and `button.previous` / `button.next` when there are more pages) |
| `case.picker.placeholder` | Choose a rule |
| `case.picker.no_rules` | This server has no active rules yet. Ask a rules manager to create one on the dashboard. |
| `case.picker.fill_form` | The rule {rule} needs some answers. Fill in the form to finish the case about {member}. |
| `case.form.title` | {rule} ({page}/{pages}), the rule name cut so the title fits 45 characters; a one-page form is titled {rule} alone, with no "(1/1)" |
| `case.form.boolean_placeholder` | Choose Yes or No |
| `case.form.number_placeholder` | A number, like 3 or 2.5 |
| `case.form.link_placeholder` | https://discord.com/channels/… |
| `case.draft.page_saved` | Page {page} of {pages} is saved. Select Continue to finish adding the case. |
| `case.draft.button.continue` | Continue |
| `case.draft.button.fill_form` | Fill in the form |
| `case.draft.button.try_again` | Try again |
| `case.draft.page_invalid` | Some answers need fixing, so this page isn't saved yet:\n{problems, one per line}\nSelect Try again to fix your answers. |
| `case.draft.page_already_saved` (the draft is on a later page) | This page is already saved. Select Continue to open the next page. (button `case.draft.button.continue`) |
| `case.draft.page_already_saved` (the draft already became a case) | The case is already saved. Select Open case to see it. (button `button.open_case`) |
| `case.draft.expired` | This form expired after 15 minutes. Run /case add again to open a new form. |
| `case.draft.rule_changed` | The rule {rule} changed while you were filling in the form. Run /case add again to use the new version. |
| `case.draft.save_failed` | {the error, §12.3} (button `case.draft.button.try_again`, which saves the completed draft again) |
| `context.required` | {label} — Enter an answer. |
| `context.too_long` | {label} — Use at most {max} characters. ({max} is 500 for short text, 4,000 for long text and message links) |
| `context.not_text` | {label} — Enter text. |
| `context.not_yes_no` | {label} — Choose Yes or No. |
| `context.not_a_number` | {label} — Enter a number, like 3 or 2.5. |
| `context.number_out_of_range` | {label} — Enter a number between −1,000,000,000,000,000 and 1,000,000,000,000,000 with at most 6 decimal places. |
| `context.not_a_message_link` | {label} — Paste a Discord message link. Right-click or press and hold the message, then choose Copy Message Link. |
| `context.other_server` | {label} — That message is in another server. Use a message from this server. |
| `context.unknown_field` | One answer belongs to a field this rule no longer uses. Open the latest form and enter your answers there. (the key stays in the API error's details) |
| `context.duplicate_field` | {label} has two answers. Keep one answer and save again. (label unknown: "One field has two answers. Open the form again and enter one answer per field.") |
| `context.details_label` | Details |
| `member.context.link_saved` | A message saved as evidence |

`{label}` is the field's label, or its key when the key isn't a field of
the rule. Form inputs whose label is over 45 characters show the label cut
to 45 and the whole question as the input's description.

### 12.6 Actions

Action sentences for staff views, receipts, the dashboard, and the log
channel. A `pending` action after at least one attempt reads
`action.retry_scheduled` instead (staff views). A `failed` action uses
`action.<kind>.failed` with its `{reason}`. When the outcome is unknown
(`unconfirmed`, or `ActionView::may_have_applied` because an earlier
attempt may have applied before a later refusal, §5.2) it uses
`action.unconfirmed`, then the `{reason}` of the refusal when there was
one, so a later refusal never hides that the action may have happened and
one sentence never says both "couldn't" and "couldn't confirm". A
`cancelled` action after an attempt that may have applied uses
`action.cancelled_uncertain` (outcome "couldn't confirm"). A failure
someone dismissed appends `action.dismissed_suffix` (staff views only).
`{reason}` never says "retry": the card's `case.card.controls.*` line and
the log post's button offer Retry only when it is allowed. `{ends_at}` is
left out when the end isn't known: a timeout reads "Quack is timing out
{member}." while running, "Quack timed out {member}." when done, and
"{member} already had a timeout. Quack left it in place." when done with no
change.

`{does}` is what the action does to the member, in the tense the sentence
needs: "time out {member}", "remove {member} from the server", "ban
{member}", "remove {member}'s timeout", "unban {member}" (past: "timed out
{member}", "removed {member} from the server", "banned {member}", "removed
{member}'s timeout", "unbanned {member}").

| Id | Text |
| --- | --- |
| `action.none` | This case is a warning. |
| `action.retry_scheduled` | Quack will try to {does} again {time}. |
| `action.unconfirmed` | Quack couldn't confirm whether Discord {does, past}. {check} (`{check}` staff views only) |
| `action.check.timeout` | Check {member}'s timeout in Discord. (timeout, remove timeout) |
| `action.check.kick` | Check Server Settings → Audit Log in Discord for the kick. |
| `action.check.ban` | Check Server Settings → Bans in Discord. (ban, unban) |
| `action.cancelled_uncertain` | Case #{number} was voided. Quack couldn't confirm whether Discord {does, past}. Quack won't try again. Staff views add: "Quack will check whether the timeout can be removed." (timeout), "Quack will check whether the ban can be removed." (ban), or "A kick can't be undone." (kick) |
| `action.timeout.in_progress` | pending: Quack will time out {member} until {ends_at}. / running: Quack is timing out {member} until {ends_at}. |
| `action.timeout.done` | Quack set {member}'s timeout to end {ends_at}. |
| `action.timeout.done_no_change` | {member} already had a timeout ending at or after {datetime}. Quack left it in place. |
| `action.timeout.failed` | Quack couldn't time out {member}. {reason} |
| `action.timeout.cancelled` | Case #{number} was voided before Quack timed out {member}. |
| `action.kick.in_progress` | pending: Quack will remove {member} from the server. / running: Quack is removing {member} from the server. |
| `action.kick.done` | Quack removed {member} from the server. |
| `action.kick.done_no_change` | {member} left the server after this case was added, so Quack didn't remove them. |
| `action.kick.failed` | Quack couldn't remove {member} from the server. {reason} |
| `action.kick.cancelled` | Case #{number} was voided before Quack removed {member} from the server. |
| `action.ban.in_progress` | pending: Quack will ban {member}. / running: Quack is banning {member}. |
| `action.ban.done` | Quack banned {member}. |
| `action.ban.done_delete` | Quack banned {member} and deleted their messages from the last {last}. |
| `action.ban.done_no_change` | {member} was already banned when Quack checked. |
| `action.ban.failed` | Quack couldn't ban {member}. {reason} |
| `action.ban.cancelled` | Case #{number} was voided before Quack banned {member}. |
| `action.remove_timeout.in_progress` | pending: Quack will remove {member}'s timeout. / running: Quack is removing {member}'s timeout. |
| `action.remove_timeout.done` | Quack removed {member}'s timeout. |
| `action.remove_timeout.done_no_change` | {member} had no active timeout when Quack checked. |
| `action.remove_timeout.failed` | Quack couldn't remove {member}'s timeout. {reason} |
| `action.remove_timeout.skipped` | Quack left {member}'s timeout in place because case #{other} still applies. ("another case still applies" when the other case isn't known) |
| `action.remove_timeout.skipped_changed` | Quack left {member}'s timeout in place because it no longer matches case #{number}. Check {member}'s timeout in Discord. |
| `action.unban.in_progress` | pending: Quack will unban {member}. / running: Quack is unbanning {member}. |
| `action.unban.done` | Quack unbanned {member}. |
| `action.unban.done_no_change` | {member} wasn't banned when Quack checked. |
| `action.unban.failed` | Quack couldn't unban {member}. {reason} |
| `action.unban.skipped` | Quack left {member}'s ban in place because case #{other} still applies. ("another case still applies" when the other case isn't known) |
| `action.unban.skipped_changed` | Quack left {member}'s ban in place because it may not have come from case #{number}. Check Server Settings → Bans in Discord. |
| `action.dismissed_suffix` | {actor} dismissed this failure {time}. The case keeps its history. |

Failure reasons (`{reason}`; when the member isn't known, `{member}`
reads "The member"). Permission and role reasons are the shared
`access.*` sentences (§12.3):

| Id | Text |
| --- | --- |
| `action.failure.quack_not_in_server` | `access.quack_not_in_server` |
| `action.failure.quack_missing_permission` | `access.quack_role_needs` (staff) |
| `action.failure.quack_role_too_low` | `access.quack_role_too_low` |
| `action.failure.member_is_owner` | {member} owns this server. Discord doesn't allow this action on the server owner. |
| `action.failure.member_not_in_server` | timeout: {member} isn't in the server, so Discord can't time them out. / remove timeout: {member} isn't in the server. |
| `action.failure.member_is_administrator` | {member} has Administrator, and Discord doesn't allow timeouts for members with Administrator. |
| `action.failure.timeout_window_passed` | The timeout would already have ended, so Quack didn't apply it. |
| `action.failure.discord_refused` | Discord didn't allow it. Check Quack's role and permissions in Server Settings → Roles. |
| `action.failure.retries_exhausted` | one: Quack stopped after one attempt. / other: Quack stopped trying after {attempts} attempts. Both add "Discord didn't apply it." unless an attempt may have applied. |
| `action.failure.original_unconfirmed` | The {ban / timeout} Quack tried for this case was never confirmed, and Discord doesn't show it now. {check} (undos only; see the implementation notes) |

Member-facing action sentences (dashboard member view; `you`):

| Id | Text |
| --- | --- |
| `member.action.none` | This case is a warning. |
| `member.action.timeout.in_progress` | pending: Quack will time you out until {ends_at}. / running: Quack is timing you out until {ends_at}. ("…time you out." when the end isn't known) |
| `member.action.timeout.done` | Quack set your timeout to end {ends_at}. ("Quack timed you out." when the end isn't known; no change: "You already had a timeout. Quack left it in place.") |
| `member.action.unconfirmed` | Quack couldn't confirm whether Discord {timed you out / removed you from the server / banned you / removed your timeout / unbanned you}. A cancelled timeout or ban adds "The case was voided, so Quack will check whether the {timeout / ban} can be removed."; a cancelled kick "The case was voided. A kick can't be undone." |
| `member.action.timeout.not_applied` | failed: Quack didn't time you out. / cancelled: The case was voided before Quack timed you out. |
| `member.action.kick.in_progress` | pending: Quack will remove you from the server. / running: Quack is removing you from the server. |
| `member.action.kick.done` | Quack removed you from the server. (no change: "You left the server after this case was added, so Quack didn't remove you.") |
| `member.action.kick.not_applied` | failed: Quack didn't remove you from the server. / cancelled: The case was voided before Quack removed you from the server. |
| `member.action.ban.in_progress` | pending: Quack will ban you. / running: Quack is banning you. |
| `member.action.ban.done` | Quack banned you. ("Quack banned you and deleted your messages from the last {last}." when it deleted messages; no change: "You were already banned when Quack checked.") |
| `member.action.ban.not_applied` | failed: Quack didn't ban you. / cancelled: The case was voided before Quack banned you. |
| `member.action.remove_timeout.done` | Quack removed your timeout. (no change: "You had no active timeout when Quack checked.") |
| `member.action.remove_timeout.in_progress` | pending: Quack will remove your timeout. / running: Quack is removing your timeout. |
| `member.action.remove_timeout.skipped` | Your timeout stays in place because another case still applies. |
| `member.action.remove_timeout.skipped_changed` | Quack left your timeout in place. Ask the moderators if you need help. |
| `member.action.remove_timeout.not_applied` | Your timeout is still in place. The moderators can see why. |
| `member.action.unban.done` | Quack unbanned you. (no change: "You weren't banned when Quack checked.") |
| `member.action.unban.in_progress` | pending: Quack will unban you. / running: Quack is unbanning you. |
| `member.action.unban.skipped` | Your ban stays in place because another case still applies. |
| `member.action.unban.skipped_changed` | Quack left your ban in place. Ask the moderators if you need help. |
| `member.action.unban.not_applied` | Your ban is still in place. The moderators can see why. |

`not_applied` covers `failed` with a definite reason and `cancelled` with
no attempt that may have applied; the unknown outcomes use
`member.action.unconfirmed`.

Staff controls:

| Id | Text |
| --- | --- |
| `action.retry.done` | Quack will try again to {time out the member / remove the member from the server / ban the member / remove the timeout / unban the member} for case #{number}. Open the case to see the outcome. |
| `action.dismiss.done` | The {timeout / kick / ban} failure for case #{number} was dismissed. The case keeps its history. (undos: "The failed attempt to {remove the timeout / unban the member} for case #{number} was dismissed. …") |
| `action.undo.confirm.remove_timeout` | Remove {member}'s timeout for case #{number}? |
| `action.undo.confirm.unban` | Unban {member} for case #{number}? |
| `action.undo.button.remove_timeout` | Yes, remove timeout |
| `action.undo.button.unban` | Yes, unban |
| `action.undo.queued.remove_timeout` | Quack will check whether it can remove {member}'s timeout. Open case #{number} to see the outcome. |
| `action.undo.queued.unban` | Quack will check whether it can unban {member}. Open case #{number} to see the outcome. |

An undo checks Discord before it changes anything (§5.6), so the queued
and void texts promise a check, not a result.

Voiding:

| Id | Text |
| --- | --- |
| `case.void.modal.title` | Void case #{number} — or "Void case" when the number is unknown |
| `case.void.modal.label` | Why are you voiding this case? |
| `case.void.modal.description` | The member can see this reason. |
| `case.void.done` | Case #{number} is voided. It stays in the history and no longer counts toward {rule}. |
| `case.void.undo.remove_timeout` | Quack will check whether it can remove {member}'s timeout. |
| `case.void.undo.unban` | Quack will check whether it can unban {member}. |
| `case.void.cancelled_action` | Quack won't {time out {member} / remove {member} from the server / ban {member}}. |
| `case.void.running_action` | Quack had already started the {timeout / ban}. ("Quack will check whether it can be removed." is added when no undo sentence follows) |
| `case.void.running_kick` | Quack had already started removing {member} from the server. A kick can't be undone. |
| `case.void.done_kick` | Quack had already removed {member} from the server. A kick can't be undone. |
| `case.void.check_outcome` | Open case #{number} to see the outcome. |
| `case.void.reason.appeal_accepted` | Appeal accepted. |
| `case.void.appeal_closed` | The appeal was closed because the case was voided. Quack will try to send {member} a DM about it. |

`case.void.done` is followed by one sentence for each thing the void did,
in order, then `case.void.check_outcome` when an undo is checking, then
`case.void.appeal_closed` when it closed an appeal.

Context and evidence:

| Id | Text |
| --- | --- |
| `case.context.modal.title` | Edit context · case #{number} |
| `case.context.saved` | Case context saved for case #{number}. |
| `case.context.saved_new_evidence` | Case context saved for case #{number}. one: Quack is saving 1 new message as evidence. / other: Quack is saving {count} new messages as evidence. |
| `case.context.too_many_fields` | Case #{number} has more context fields than fit in a Discord form. Edit it on the dashboard. (link button "Edit case on dashboard") |
| `case.context.details_label` | Details |
| `evidence.modal.title` | Add evidence · case #{number} |
| `evidence.modal.label` | Message links |
| `evidence.modal.description` | One Discord message link per line, up to 10. (placeholder: `case.form.link_placeholder`) |
| `evidence.added` | one: Quack is saving 1 message as evidence for case #{number}. / other: Quack is saving {count} messages as evidence for case #{number}. |
| `evidence.added.retrying` | one: Quack will try again to save 1 message that it couldn't save before. / other: Quack will try again to save {count} messages that it couldn't save before. |
| `evidence.added.already_saved` | one: 1 message was already saved as evidence. / other: {count} messages were already saved as evidence. |
| `evidence.view.heading` | ### Evidence {item} of {total} · case #{number} |
| `evidence.view.message` | Message from {author} in {channel}, sent {time} · [Open message]({url}) ("from {author} " and ", sent {time}" are left out until the message is saved) |
| `evidence.view.edited` | Edited {time}. |
| `evidence.view.text` | > {text} |
| `evidence.view.embeds` | Link previews: {titles or types, comma-separated} |
| `evidence.view.saving` | Quack is still saving this message. |
| `evidence.view.none` | Case #{number} has no evidence yet. Use Add evidence to add message links. |
| `evidence.failure.message_deleted` | The message was deleted before Quack saved it. If another message shows what happened, add its link. |
| `evidence.failure.moderator_no_access` | Quack didn't save this message because the moderator who added it can't read the channel. Ask a moderator who can read the channel to add it. |
| `evidence.failure.quack_no_access` | Quack couldn't save this message because it can't read the channel. Ask an administrator to allow View Channel and Read Message History for Quack there, then add the link again. |
| `evidence.failure.other_server` | Quack didn't save this message because it's from another server. Add a message link from this server. |
| `evidence.failure.discord_error` | Quack couldn't save this message. Add the link again to retry. |
| `evidence.file.copied` | {name} · Copy saved |
| `evidence.file.pending` | {name} · Copying file… |
| `evidence.file.link_only` | {name} · Original link only. No evidence channel is set. |
| `evidence.file.too_large` | {name} · Original link only. Quack copies files up to 10 MB. |
| `evidence.file.unsupported_type` | {name} · Original link only. Quack doesn't copy this file type. |
| `evidence.file.copy_failed` | {name} · Quack couldn't copy this file. The original link may stop working. |
| `evidence.files_not_saved` | one: 1 more file wasn't saved, because a case keeps at most 100 files and 10 per message. / other: {count} more files weren't saved, because a case keeps at most 100 files and 10 per message. |
| `evidence.channel_not_staff_only` | Quack didn't copy files because the evidence channel is visible outside staff. Fix its View Channel permissions or choose another evidence channel in /setup. |
| `evidence.state_text` | The API's `state_text` of an item: saving: `evidence.view.saving`; saved: "Saved."; failed: its `evidence.failure.*` (`evidence.failure.discord_error` when the reason is unknown) |

The reply to adding links is `evidence.added`, then
`evidence.added.retrying`, then "Select View evidence to see the outcome."
when either was shown, then `evidence.added.already_saved`; parts with a
zero count are left out. The viewer shows, after the heading and message
line: the saving sentence, or the failure, or (saved) the edited line, the
text quote, link previews, one line per file, files not saved, and the
staff-only notice; buttons `button.previous` / `button.next` between items
and `button.open_case`.

### 12.7 DM outcome sentences

One staff sentence per DM outcome, built by `quack_text::dm::outcome_sentence`
and used unchanged on the receipt, the card, the dashboard staff view, and
the log channel (which links `{case}`), so the same outcome reads the same
everywhere. `{about}` names what the DM was about: "about {case}" (case
DM), "about the appeal for {case}" (appeal DM), or "with the ticket
transcript" (`ticket_closed`). `{recovery}` is what the reader can do
instead: "{member} can sign in to the Quack dashboard to see the case."
(case), "… to see the appeal." (appeal), or "Staff can open the ticket on
the dashboard to check for the transcript." (ticket). When Quack can't
name the member, `{member}` is "the member" ("The member" at the start of
a sentence).

| Id | Text |
| --- | --- |
| `dm.state.pending` | Quack will try to send {member} a DM {about}. |
| `dm.state.in_progress` | Quack is sending {member} a DM {about}. |
| `dm.state.sent` | Quack sent {member} a DM {about}. |
| `dm.state.failed` | Quack couldn't send {member} a DM {about}. {failure} {recovery} |
| `dm.state.unknown` | Quack couldn't confirm whether Discord accepted the DM to {member} {about}. Quack won't send it again. {recovery} |
| `dm.state.not_needed.level` | This level doesn't send a DM. |
| `dm.state.not_needed.voided` | Quack didn't send a DM because the case was voided first. |
| `dm.failure.dms_closed` | Discord didn't allow the DM. |
| `dm.failure.not_in_shared_server` | {member} and Quack don't share a server. |
| `dm.failure.discord_unavailable` | (no sentence; the lead ends "… {about} in time.") |
| `dm.failure.discord_refused` | Discord didn't allow the DM. |

A failure with no known reason reads as `discord_refused`. Discord's
refusal doesn't say why (closed DMs, a block, or a privacy setting), so the
text doesn't guess.

Combined example (failed DM on the staff card): "**DM:** Quack couldn't
send @Sam a DM about case #12. Discord didn't allow the DM. @Sam can sign in
to the Quack dashboard to see the case."

Member-facing (dashboard member view; the member is already on their case
page, so the sentence doesn't point back to it):

| Id | Text |
| --- | --- |
| `member.dm.pending` | Quack will try to send you a DM about this case. |
| `member.dm.in_progress` | Quack is sending you a DM about this case. |
| `member.dm.sent` | Quack sent you a DM about this case. |
| `member.dm.failed` | Quack couldn't send you a DM about this case. |
| `member.dm.unknown` | Quack couldn't confirm whether Discord accepted the DM about this case. Quack won't send it again. |
| `member.dm.not_needed` | Quack doesn't send a DM for this case. |
| `member.dm.not_needed.voided` | Quack didn't send a DM because the case was voided first. |

### 12.8 DMs to members

`dm.case` (one `Msg`; lines in order, omitted when the condition is false).
The case number, date, action, and appeal line come before the server's
text and the case context, so trimming a long DM never removes them:

| Id | Text | Condition |
| --- | --- | --- |
| `dm.case.heading` | ### Case #{number} in {server} | always |
| `dm.case.rule` | Rule: {rule} · {datetime} | always |
| `dm.case.reason` | > {reason} | always |
| `dm.case.outcome.warning` | **Action for this case:** This case is a warning. | no action |
| `dm.case.outcome.timeout` | **Action for this case:** The rule calls for a timeout until {ends_at}. Open your case on the dashboard to check its outcome. ("…calls for a timeout." when the end isn't known) | timeout |
| `dm.case.outcome.kick` | **Action for this case:** The rule calls for removing you from the server. Open your case on the dashboard to check its outcome. | kick |
| `dm.case.outcome.ban` | **Action for this case:** The rule calls for banning you from the server. Open your case on the dashboard to check its outcome. | ban |
| `dm.case.appeal` | To ask the moderators to review this case, select Appeal case. You can appeal once. | appealable |
| `dm.case.not_appealable` | This case can't be appealed. | not appealable |
| `dm.case.intro` | {dm_intro} (the server's text, Markdown allowed, mentions suppressed) | set and not blank |
| `dm.case.details_heading` | **Details** | the case has non-link context |
| `dm.case.detail_line` | **{label}:** {value} (cut to 300 characters with "…"; booleans Yes/No; `message_link` fields never shown) | per field |
| `dm.case.footer_text` | {dm_footer} (the server's text) | set and not blank |
| `dm.case.trimmed` | Open your case on the dashboard for the full details. | the DM was shortened to fit |
| `dm.case.button.appeal` | Appeal case | appealable (custom ID `1:ap:<case>`) |
| `dm.case.button.dashboard` | Open your case | always (link to `/guilds/{guild_id}/cases/{case_id}/appeal`) |

The action line describes the rule's configured action, never Discord's
result: the DM may arrive before a kick or ban runs (§6.3), and the
member's case page shows the outcome. The DM shows the full date and time
(`{datetime}`), not a relative time, because a DM is read long after it is
sent.

`dm.appeal.*` (one per appeal event, rendered from that event). Every one
has the heading `### Your appeal for case #{number} in {server}`, a lead
saying what the moderators did, the moderator's words as a quote, the
subtext `-# {datetime of the event}`, and one link button to the member's
case page: `dm.appeal.button.reply` "Reply to appeal" when the moderators
asked for information, `dm.appeal.button` "Open your appeal" after a
rejection or close, `dm.appeal.button.case` "Open your case" after an
acceptance or a void. A shortened appeal DM ends with
`dm.appeal.trimmed` "Open your appeal on the dashboard for the full
details.".

| Id | Lead | After the quote |
| --- | --- | --- |
| `dm.appeal.information_requested` | The moderators asked for more information: | Select Reply to appeal to answer on the dashboard. |
| `dm.appeal.reopened` | The moderators reopened your appeal and asked for more information: | Select Reply to appeal to answer on the dashboard. |
| `dm.appeal.accepted` | The moderators accepted your appeal. Case #{number} no longer counts. | Open your case to check whether any timeout or ban was removed. |
| `dm.appeal.accepted.rejoin_ban` | (appended) | Once your ban is removed, you can try this invite to rejoin the server: {invite, as a link} |
| `dm.appeal.accepted.rejoin_kick` | (appended) | You can try this invite to rejoin the server: {invite, as a link} |
| `dm.appeal.rejected` | The moderators rejected your appeal. Case #{number} still counts. | |
| `dm.appeal.closed` | The moderators closed your appeal. Case #{number} still counts. | |
| `dm.appeal.closed_case_voided` | Your appeal was closed because case #{number} was voided. The case no longer counts. | (no quote) |

### 12.9 Appeals

Member side:

| Id | Text |
| --- | --- |
| `appeal.form.title` | Appeal this case |
| `appeal.form.label` | Why should moderators reconsider this case? |
| `appeal.form.description` | Explain what you want the moderators to review. You can appeal this case once. |
| `appeal.submitted` | Your appeal for case #{number} is saved. It's waiting for a decision. Open your appeal on the dashboard to see updates. |
| `appeal.submit.exists` | You already appealed case #{number}. Open your appeal on the dashboard to see the latest update. |
| `appeal.submit.not_allowed` | Case #{number} can't be appealed. You can still review it on the dashboard. |
| `appeal.submit.voided` | Case #{number} was voided and no longer counts. You can still review it on the dashboard. |
| `appeal.submit.not_yours` | This button isn't for your account. Sign in to the dashboard to see your own cases. |
| `appeal.information.saved` | Your reply was added to the appeal for case #{number}. It's waiting for a decision. |
| `appeal.information.not_requested` | There's no question waiting for you to answer. Open your appeal to see the latest update. |
| `appeal.information.outdated` | The appeal changed while you were writing. Reload it and read the latest question before replying. |

Staff side (appeal post, appeal card, `/appeals`):

| Id | Text |
| --- | --- |
| `appeal.post.heading` | ### Appeal for case #{number} · {rule} |
| `appeal.post.from` | From {member} · submitted {time} |
| `appeal.post.state.pending` | Waiting for a decision. |
| `appeal.post.state.needs_information` | Waiting for {member} to reply. |
| `appeal.post.state.accepted` | Accepted by {actor} {time}. ("Accepted." when unknown) |
| `appeal.post.state.rejected` | Rejected by {actor} {time}. ("Rejected." when unknown) |
| `appeal.post.state.closed` | Closed by {actor} {time}. ("Closed." when unknown) |
| `appeal.post.state.closed_case_voided` | Closed {time} because the case was voided. ("Closed because the case was voided." when the time is unknown) |
| `appeal.post.statement_heading` | **Appeal statement** |
| `appeal.post.statement` | > {statement, first 1,500 characters, cut with "…"} |
| `appeal.post.statement_cut` | -# Open the appeal on the dashboard for the full statement. |
| `appeal.post.information_heading` | **Reply from the member**, then > {latest reply, first 1,500 characters} |
| `appeal.post.reason_heading` | **Reason shared with the member**, then > {decision reason} (not shown when a void closed the appeal) |
| `appeal.button.accept` | Accept (while pending) |
| `appeal.button.reject` | Reject (while pending) |
| `appeal.button.open` | Open appeal (link to the dashboard) |
| `appeal.post.trimmed` | Open the appeal on the dashboard for the full statement and decision. (a post shortened to fit) |
| `appeal.modal.accept.title` | Accept appeal |
| `appeal.modal.reject.title` | Reject appeal |
| `appeal.modal.reason.label` | Reason the member will see |
| `appeal.modal.reason.description` | Leave out staff names and private details. |
| `appeal.reason.default.accept` | We reviewed your appeal. The case no longer counts. |
| `appeal.reason.default.reject` | We reviewed your appeal. The case still counts. |
| `appeal.decided.accepted` | Appeal accepted. Case #{number} is voided and no longer counts. Quack will try to send {member} a DM. Open the case to check the outcome of any timeout or ban. |
| `appeal.decided.rejected` | Appeal rejected. Case #{number} still counts. Quack will try to send {member} a DM. |
| `appeal.closed_by_void` | The appeal was closed because the case was voided. |
| `appeal.post.moved` | -# Appeals now go to {channel}. Review this appeal there or on the dashboard. |
| `appeals.heading` | ### Appeals waiting for a decision |
| `appeals.row` | Case #{number} · {member} · {rule name, not bold} · submitted {time} |
| `appeals.empty` | No appeals are waiting for a decision. Open Appeals on the dashboard to see replies and past decisions. |
| `appeals.button.open` | Open appeal for #{number} (one per row, then `button.previous` / `button.next`; the page counter `word.page` is subtext when there is more than one page) |

Timeline (the case's history on the dashboard). Staff see who acted;
members see "you" and never a staff name. Action and DM items reuse
§12.6 and §12.7 (`member.action.*`, `member.dm.*` on the member view).

| Id | Event | Staff view | Member view |
| --- | --- | --- | --- |
| `timeline.case_added` | case added | Case #{number} was added. | same |
| `timeline.case_voided` | The case was voided. It no longer counts.\n> {void_reason} | The case was voided, so it no longer counts:\n> {void_reason} | same |
| `appeal.event` | submitted | {member} appealed the case. | You appealed the case. |
| `appeal.event` | information requested | {actor} asked for more information. | The moderators asked for more information. |
| `appeal.event` | information provided | {member} replied to the appeal. | You replied to the appeal. |
| `appeal.event` | accepted | {actor} accepted the appeal. The case was voided and no longer counts. | The moderators accepted the appeal. The case was voided and no longer counts. |
| `appeal.event` | rejected | {actor} rejected the appeal. The case still counts. | The moderators rejected the appeal. The case still counts. |
| `appeal.event` | closed by staff | {actor} closed the appeal. | The moderators closed the appeal. |
| `appeal.event` | closed by Quack (case voided) | `appeal.closed_by_void` | `appeal.closed_by_void` |
| `appeal.event` | reopened | {actor} reopened the appeal and asked for more information. | The moderators reopened the appeal and asked for more information. |

On the staff view, an event without a known actor reads "The moderators …"
like the member view.

### 12.10 Log channel entries

Each post is: the lead sentence below, an optional quote (the void reason
of a staff void), and the subtext `-# {datetime} · [Open moderation log](<link>)`
(the link left out when unknown); failed and refused entries have the
danger accent. The dashboard's `text` is the lead sentence, then the quote
on a new line as `> {quote}`. In entries, `{member}` reads "a member" ("A
member" at the start) and `{rule}` "a rule" when unknown, and a DM entry's
`{case}` reads "their case" when the case number is unknown.

`{couldnt}` is the start of `action.<kind>.failed` without its period
("Quack couldn't remove {member}'s timeout"); `{whether}` is the start of
`action.unconfirmed` without its period ("Quack couldn't confirm whether
Discord unbanned {member}").

| Id | Kind / result | Text |
| --- | --- | --- |
| `log.case_created` | `case_created` ok | {actor} added {case} for {member} under {rule} (level {level_index}, case count {case_count}). Action for this case: {level outcome}. ("This case is a warning." for a warning) |
| `log.case_created.honeypot` | `case_created` ok, source honeypot | Quack added {case} for {member} under {rule} after {member} posted in the honeypot channel. Action for this case: {level outcome}. ("This case is a warning." for a warning) |
| `log.case_created.replaces` | (appended) | It replaces case #{other}. |
| `log.case_create_refused.quack_role_too_low` | `case_create_refused` refused | No case was added for {member} under {rule}. `access.quack_role_too_low` |
| `log.case_create_refused.quack_missing_permission` | `case_create_refused` refused | No case was added for {member} under {rule}. This rule calls for a {timeout / kick / ban}. `access.quack_role_needs` (staff; the level's action's permission when the permission isn't recorded, else Ban Members) |
| `log.case_create_refused.quack_not_in_server` | `case_create_refused` refused | No case was added for {member} under {rule}. `access.quack_not_in_server` |
| `log.case_create_refused` | `case_create_refused` refused, any other refusal | No case was added for {member} under {rule}. {refusal reason, §12.3, about the actor in the third person: "{actor} isn't in this server anymore.", "{actor} can't add a case about themselves.", "{actor}'s highest role must be above {member}'s highest role."; "{member} isn't in the server anymore."} |
| `log.case_voided` | `case_voided` ok, cause staff | {actor} voided {case} for {member}.\n> {void_reason} |
| `log.case_voided.appeal` | `case_voided` ok, cause appeal | {actor} accepted {member}'s appeal, so {case} is voided. |
| `log.case_context_edited` | `case_context_edited` ok | {actor} edited the case context of {case}. |
| `log.evidence_added` | `evidence_added` ok (not posted) | one: {actor} added 1 evidence item to {case}. / other: {actor} added {count} evidence items to {case}. |
| `log.action_done.timeout` | `action_done` ok | Quack set {member}'s timeout to end {ends_at} for {case}. ("Quack timed out {member} for {case}." when the end isn't known) |
| `log.action_done` (timeout, no change) | `action_done` ok | {member} already had a timeout ending at or after {datetime}, so Quack left it in place for {case}. (" ending at or after {datetime}" left out when the end isn't known) |
| `log.action_done.kick` | `action_done` ok | Quack removed {member} from the server for {case}. |
| `log.action_done.kick_no_change` | `action_done` ok | {member} left the server after {case} was added, so Quack didn't remove them. |
| `log.action_done.ban` | `action_done` ok | Quack banned {member} for {case}. |
| `log.action_done` (ban, no change) | `action_done` ok | {member} was already banned when Quack checked for {case}. |
| `log.action_done.remove_timeout` | `action_done` ok | Quack removed {member}'s timeout for {case}. |
| `log.action_done.remove_timeout_no_change` | `action_done` ok | {member} had no active timeout when Quack checked for {case}. |
| `log.action_done.unban` | `action_done` ok | Quack unbanned {member} for {case}. |
| `log.action_done.unban_no_change` | `action_done` ok | {member} wasn't banned when Quack checked for {case}. |
| `log.action_failed` | `action_failed` failed | {couldnt} for {case}. {reason} — with a **Retry {action}** button while retryable |
| `log.action_failed.unconfirmed` | `action_failed` failed, reason `unconfirmed` | {whether} for {case}. {check, `action.check.*`} |
| `log.action_retry_requested` | `action_retry_requested` ok | {actor} chose to retry {the timeout / the kick / the ban / removing the timeout / unbanning the member} for {case}. |
| `log.action_dismissed` | `action_dismissed` ok | {actor} dismissed {the timeout / kick / ban failure; the failed attempt to remove the timeout / unban the member} for {case}. |
| `log.action_undo_requested.remove_timeout` | `action_undo_requested` ok | {actor} asked Quack to remove {member}'s timeout for {case}. (Quack as actor: "Quack will check whether it can remove {member}'s timeout for {case}.") |
| `log.action_undo_requested.unban` | `action_undo_requested` ok | {actor} asked Quack to unban {member} for {case}. (Quack as actor: "Quack will check whether it can unban {member} for {case}.") |
| `log.action_undo_skipped.timeout` | `action_undo_skipped` ok | Quack left {member}'s timeout in place after {case} was voided because case #{other} still applies. ("another case still applies" when the other case isn't known) |
| `log.action_undo_skipped.ban` | `action_undo_skipped` ok | Quack left {member}'s ban in place after {case} was voided because case #{other} still applies. ("another case still applies" when the other case isn't known) |
| `log.action_undo_skipped.changed_timeout` | `action_undo_skipped` ok, `changed_outside_case` | Quack left {member}'s timeout in place because it no longer matches {case}. Check {member}'s timeout in Discord. |
| `log.action_undo_skipped.changed_ban` | `action_undo_skipped` ok, `changed_outside_case` | Quack left {member}'s ban in place because it may not have come from {case}. Check Server Settings → Bans in Discord. |
| `log.dm_sent.case` | `dm_sent` ok (not posted) | `dm.state.sent` (§12.7) |
| `log.dm_sent.appeal` | `dm_sent` ok (not posted) | `dm.state.sent` about the appeal |
| `log.dm_sent.ticket` | `dm_sent` ok (not posted) | `dm.state.sent` with the ticket transcript |
| `log.dm_failed.case` | `dm_failed` failed, state failed | `dm.state.failed` (§12.7), with {case} linked |
| `log.dm_failed.case_unknown` | `dm_failed` failed, state unknown | `dm.state.unknown` |
| `log.dm_failed.appeal` | `dm_failed` failed, state failed | `dm.state.failed` about the appeal |
| `log.dm_failed.appeal_unknown` | `dm_failed` failed, state unknown | `dm.state.unknown` about the appeal |
| `log.dm_failed.ticket` | `dm_failed` failed | `dm.state.failed` with the ticket transcript |
| `log.dm_failed.ticket_unknown` | `dm_failed` failed, state unknown | `dm.state.unknown` with the ticket transcript |
| `log.appeal_submitted` | `appeal_submitted` ok | {member} appealed {case}. |
| `log.appeal_information_requested` | `appeal_information_requested` ok | {actor} asked {member} for more information about their appeal for {case}. |
| `log.appeal_information_provided` | `appeal_information_provided` ok | {member} replied to the appeal for {case}. |
| `log.appeal_accepted` | `appeal_accepted` ok | {actor} accepted {member}'s appeal for {case}. |
| `log.appeal_rejected` | `appeal_rejected` ok | {actor} rejected {member}'s appeal for {case}. |
| `log.appeal_closed` | `appeal_closed` ok | {actor} closed {member}'s appeal for {case}. |
| `log.appeal_closed.case_voided` | `appeal_closed` ok, by Quack | Quack closed {member}'s appeal for {case} because the case was voided. |
| `log.appeal_reopened` | `appeal_reopened` ok | {actor} reopened {member}'s appeal for {case} and asked for more information. |
| `log.rule_created` | `rule_created` ok | {actor} created the rule {rule}. |
| `log.rule_edited` | `rule_edited` ok | {actor} edited the rule {rule}. New cases use the updated rule (version {version}). |
| `log.rule_archived` | `rule_archived` ok | {actor} archived the rule {rule}. It can't be applied until someone restores it. |
| `log.rule_restored` | `rule_restored` ok | {actor} restored the rule {rule}. |
| `log.rule_imported` | `rule_imported` ok | {actor} imported the rule {rule}. |
| `log.settings_changed` | `settings_changed` ok | {actor} changed Quack's settings: {fields}. (no nameable field: "{actor} changed Quack's settings. Open the settings on the dashboard to review them.") |
| `log.module_enabled` | `module_enabled` ok | {actor} turned on {module}. |
| `log.module_disabled` | `module_disabled` ok, by staff | {actor} turned off {module}. |
| `log.module_disabled.channel_deleted` | `module_disabled` ok, by Quack | Quack turned off {module} because its channel was deleted. Choose a new channel in /setup, then turn it on. |
| `log.module_disabled.quack_left` | `module_disabled` ok, by Quack (not posted) | Quack turned off {module} because Quack left the server. |
| `log.module_settings_changed` | `module_settings_changed` ok | {actor} changed the {module settings name} settings: {fields}. (no nameable field: "… settings. Open the settings on the dashboard to review them.") |
| `log.quack_joined.first` | `quack_joined` ok | Quack joined the server and created the starter rule {rule}. Review it on the dashboard. |
| `log.quack_joined.again` | `quack_joined` ok | Quack rejoined the server. |
| `log.quack_left` | `quack_left` ok (not posted) | Quack left the server. |
| `log.channel_setting_cleared` | `channel_setting_cleared` ok | The {setting} was deleted. Choose a new channel in /setup. |
| `log.log_channel_failed.not_staff_only` | `log_channel_failed` failed (not posted) | Quack stopped posting to the log channel because members outside staff can see it. The entries are saved in the moderation log. Fix the channel's View Channel permissions or choose another log channel in /setup. |
| `log.log_channel_failed.discord_refused` | `log_channel_failed` failed (not posted) | Discord didn't let Quack post in the log channel. The entries are saved in the moderation log. In Edit Channel → Permissions, check View Channel and Send Messages for Quack. |
| `log.log_channel_failed.retries_exhausted` | `log_channel_failed` failed (not posted) | Quack couldn't confirm whether an entry reached the log channel. The entries are saved in the moderation log. |
| `log.ticket_opened` | `ticket_opened` ok | {member} opened a ticket: {thread}. ("{member} opened a ticket." when the thread is unknown) |
| `log.ticket_closed` | `ticket_closed` ok, closed by staff | {actor} closed {member}'s ticket. Open the ticket on the dashboard to check for a transcript. |
| `log.ticket_closed` | `ticket_closed` ok, closed by the member | {member} closed their ticket. Open the ticket on the dashboard to check for a transcript. |
| `log.ticket_closed.partial` | (appended) | Some messages may be missing from it. |
| `log.honeypot_case_failed` | `honeypot_case_failed` failed, a member refusal | Quack didn't add a honeypot case for {member}. {refusal reason, §12.3, for Quack as the actor: a refusal about the actor (self, role hierarchy) reads "Review the post and use /case add if a case is needed."} |
| `log.honeypot_case_failed` | `honeypot_case_failed` failed, `rule_unavailable` | Quack didn't add a honeypot case for {member}. The honeypot rule is archived or no longer exists. Choose a rule in /setup or on the dashboard. |
| `log.honeypot_case_failed` | `honeypot_case_failed` failed, `policy_changed` | Quack didn't add a honeypot case for {member}. The honeypot settings changed while Quack was adding it. Review the post and use /case add if a case is needed. |
| `log.honeypot_case_failed` | `honeypot_case_failed` failed, `discord_unavailable` | Quack didn't add a honeypot case for {member}. Quack couldn't reach Discord in time. Review the post and use /case add if a case is needed. |
| `log.legacy` | `legacy` (imported, not posted) | Imported from an earlier version of Quack: {legacy action}. (no action name: "This entry was imported from an earlier version of Quack.") |

`{refusal}` reuses the §12.3 refusal texts without their first sentence
("No case was added."), with the first letter lowercased unless the text
starts with "Quack" or "Discord"; its own period ends the entry. Its
`{action_verb}` and `{permission}` come from the level's action, which the
poster passes as `LogFacts.action` (Ban Members when unknown).
`{level outcome}` is
`level.outcome.*`.

`{legacy action}` is the v4 action name in today's words: `_`, `.`, `:`,
`-`, and spaces become single spaces, and old words are replaced, keeping
plurals plural (template, policy → rule; execution, punishment,
enforcement → action; infraction, incident → case; notification, notice →
DM; audit log, audit → moderation log; mirror → log channel; user, target,
offender → member; mod → moderator; admin → administrator; "templates" →
"rules" and so on). Example: the v4 name case_execution.retry reads "case
action retry", and audit_log.export reads "moderation log export".

`{fields}` names each changed setting once, joined with commas and "and"
("log channel, appeal channel, and DM footer"). A key without a name reads
"another setting", so a machine name never reaches a sentence.

Server settings (`log.settings_changed`), by setting key or API field:

| Id | Keys | Name |
| --- | --- | --- |
| `setting.log_channel` | log_channel, log_channel_id | log channel |
| `setting.appeal_channel` | appeal_channel, appeal_channel_id | appeal channel |
| `setting.evidence_channel` | evidence_channel, evidence_channel_id | evidence channel |
| `setting.appeal_invite` | appeal_invite, appeal_invite_url | rejoin invite |
| `setting.dm_intro` | dm_intro | DM introduction |
| `setting.dm_footer` | dm_footer | DM footer |
| `setting.moderator_roles` | moderator_roles, moderator_role_ids | moderator roles |
| `setting.rules_manager_roles` | rules_manager_roles, rules_manager_role_ids | rules manager roles |
| `setting.starter_notice` | starter_notice, starter_notice_acknowledged_at | starter rule reminder |
| (any other key) | | another setting |

Module settings (`log.module_settings_changed`), by key or API field within
that module's settings:

| Id | Module | Keys | Name |
| --- | --- | --- | --- |
| `setting.ticket_entry_channel` | tickets | entry_channel, entry_channel_id, ticket_entry_channel | ticket entry channel |
| `setting.ticket_queue_channel` | tickets | queue_channel, queue_channel_id, ticket_queue_channel | ticket staff channel |
| `setting.transcript_retention` | tickets | transcript_retention, transcript_retention_days | transcript retention |
| `setting.honeypot_channel` | honeypot | channel, channel_id, honeypot_channel | honeypot channel |
| `setting.honeypot_rule` | honeypot | rule, rule_id, honeypot_rule | honeypot rule |
| `setting.honeypot_warning` | honeypot | warning, warning_text, honeypot_warning | honeypot warning |
| `setting.activity_routes` | server activity log | routes, activity_routes | server activity channels |
| `setting.activity_content` | server activity log | include_message_content, activity_content | message text in the server activity log |
| (any other key) | any | | another setting |

`{module}`: `module.tickets` "tickets", `module.activity_log` "the server
activity log", `module.honeypot` "the honeypot"; at the start of a sentence
(`{Module}`) the first letter is uppercase. `{module settings name}`:
"ticket", "server activity log", "honeypot". `{setting}` in
`log.channel_setting_cleared`: log channel, appeal channel, evidence
channel, ticket entry channel, ticket staff channel, honeypot channel,
server activity channel.

### 12.11 Setup

| Id | Text |
| --- | --- |
| `setup.home.heading` | ### Set up Quack |
| `setup.home.intro` | Choose what to set up. Changes save right away and also show on the dashboard. (the result of the last change, if any, comes before it) |
| `setup.home.log` | **Log channel:** {channel}, or "Not set" |
| `setup.home.appeals` | **Appeals:** {channel}, or "Not set"{", rejoin invite set" when set} |
| `setup.home.evidence` | **Evidence copies:** {channel}, or "Off" |
| `setup.home.tickets` | **Tickets:** On/Off{" · members open tickets in {entry} · new tickets go to {queue}" when both are set} |
| `setup.home.honeypot` | **Honeypot:** On/Off{" · {channel} · rule {rule}" when both are set} |
| `setup.home.activity_log` | **Server activity log:** On/Off{" · {channel}" or " · several channels"} |
| `setup.home` (link button) | Open server settings (the dashboard's settings page; after the six area buttons) |
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
| `setup.button.stop_log` | Stop posting log entries |
| `setup.button.stop_evidence` | Stop copying files |
| `setup.button.set_invite` | Set rejoin invite |
| `setup.button.remove_invite` | Remove invite |
| `setup.button.stop_appeals` | Stop posting appeals |
| `setup.button.clear_rule` | Clear rule (while the honeypot is off and has a rule) |
| `setup.button.edit_warning` | Edit warning |
| `setup.select.channel` | Choose a channel |
| `setup.select.entry` | Channel where members open tickets |
| `setup.select.queue` | Staff channel for new tickets |
| `setup.select.rule` | Rule the honeypot applies |
| `setup.log.explain` | Quack posts moderation log entries in this staff-only channel. |
| `setup.log.saved` | Quack will post moderation log entries in {channel}. |
| `setup.log.cleared` | Quack stopped posting entries to Discord. You can still read them in the moderation log on the dashboard. |
| `setup.appeals.explain` | New appeals are posted in this staff-only channel for moderators to review. Quack can include a rejoin invite in DMs about accepted appeals for kicks and bans. |
| `setup.appeals.saved` | Appeals will be posted in {channel}, including open appeals that weren't posted yet. |
| `setup.appeals.cleared` | Quack stopped posting new appeals to Discord. You can still review them with /appeals or on the dashboard. |
| `setup.appeals.invite_saved` | Rejoin invite saved: {invite, as a link}. Quack includes it in DMs about accepted appeals for kicks and bans. |
| `setup.appeals.invite_cleared` | The rejoin invite is removed. |
| `setup.evidence.explain` | Quack copies evidence files to this staff-only channel so they last after the original message is deleted. |
| `setup.evidence.saved` | Quack will copy evidence files to {channel}. |
| `setup.evidence.cleared` | Quack stopped copying evidence files. Existing copies stay where they are. |
| `setup.tickets.explain` | Members open private tickets from a button in the entry channel. Staff see new tickets in the staff channel. |
| `setup.tickets.entry_saved` | Members will open tickets from {channel}. |
| `setup.tickets.queue_saved` | New tickets will be posted in {channel}. |
| `setup.tickets.on` | Tickets are on. Quack will post the Open a ticket button in {channel}. |
| `setup.tickets.off` | Tickets are off. Tickets that are already open stay open until someone closes them. |
| `setup.honeypot.explain` | When the honeypot is on, a member who posts in this channel gets a case under the chosen rule. Quack ignores posts from staff and bots. |
| `setup.honeypot.channel_saved` | The honeypot channel is {channel}. |
| `setup.honeypot.rule_saved` | The honeypot rule is now {rule}. |
| `setup.honeypot.rule_cleared` | The honeypot no longer uses a rule. Choose one before turning it on. |
| `setup.honeypot.rule_created` | Quack created the rule {rule} for the honeypot. Edit it on the dashboard to change what it does. |
| `setup.honeypot.warning_saved` | The honeypot warning is updated. |
| `setup.honeypot.on` | The honeypot is on in {channel}. |
| `setup.honeypot.off` | The honeypot is off. Posts there no longer add cases. |
| `setup.activity_log.explain` | Quack posts message edits and deletions, members joining and leaving, bans, and server changes here. |
| `setup.activity_log.saved` | Server activity will be posted in {channel}. |
| `setup.activity_log.on` | The server activity log is on. |
| `setup.activity_log.off` | The server activity log is off. |
| `setup.created` | Quack created {channel} with staff-only permissions and saved it. |
| `setup.created_public` | Quack created {channel} so members can see it, and saved it. |
| `setup.adopted` | Quack found {channel} and saved it. |
| `setup.create_failed.permission` | Quack couldn't create #{name}. `access.quack_role_needs` (administrator) Or choose an existing channel. |
| `setup.create_failed.discord` | Discord didn't let Quack create #{name}. Choose an existing channel. |
| `setup.create_unconfirmed` | Quack couldn't confirm whether Discord created #{name}. Check the channel list. If #{name} is there, choose it here. Otherwise, create it again. |
| `setup.channel_not_staff_only` | Quack didn't save {channel}:\n{problems, one per line; each says how to fix it} |
| `setup.problem.not_text` | Choose a text channel in this server. |
| `setup.problem.everyone_can_view` | Everyone can view this channel. In Edit Channel → Permissions, turn off View Channel for the @everyone role. |
| `setup.problem.role_can_view` | {role} can view this channel. In Edit Channel → Permissions, remove its View Channel access or choose another channel. |
| `setup.problem.member_can_view` | {member} can view this channel. Remove their access in Edit Channel → Permissions or choose another channel. |
| `setup.problem` (inaccessible) | Quack can't check this channel. In Edit Channel → Permissions, allow View Channel for Quack, then choose the channel again. |
| `setup.problem` (missing) | This channel no longer exists. Choose another channel. |
| `setup.quack_missing_channel_permissions` | `access.quack_channel_needs` (administrator) Then choose the channel again. |
| `setup.module.not_ready` | {Module} can't be turned on yet:\n{problems, one per line} (no problems known: "{Module} can't be turned on yet. Open /setup to see what needs fixing.") |
| `module.problem.ticket_entry_missing` | Choose where members open tickets. |
| `module.problem.ticket_queue_missing` | Choose a staff channel for new tickets. |
| `module.problem.ticket_same_channel` | The entry channel and the staff channel must be different. |
| `module.problem.honeypot_channel_missing` | Choose the honeypot channel. |
| `module.problem.honeypot_rule_missing` | Choose the rule the honeypot applies. |
| `module.problem.honeypot_rule_required_fields` | The rule {rule} has required case context, which the honeypot can't fill in. Choose another rule or make those fields optional on the dashboard. ("The rule the honeypot uses has …", not bold, when the name isn't known) |
| `module.problem.activity_no_route` | Choose a channel for server activity. |
| `module.problem.activity_needs_audit_log` | `access.quack_role_needs` (administrator, View Audit Log) Quack uses it to show who banned or unbanned a member. |
| `module.problem.channel_permissions` | `access.quack_channel_needs` (administrator) |
| `module.problem.channel_not_staff_only` | {channel} can be viewed by members outside staff. Fix its View Channel permissions or choose another channel. |
| `setup.invite.modal.title` | Rejoin invite |
| `setup.invite.modal.label` | Invite link |
| `setup.invite.modal.description` | Paste a Discord invite link. Quack includes it in DMs about accepted appeals for kicks and bans. (placeholder: https://discord.gg/abc123; prefilled with the current invite) |
| `setup.invite.invalid` | That isn't a Discord invite link. Use a link like https://discord.gg/abc123. |
| `setup.warning.modal.title` | Honeypot warning |
| `setup.warning.modal.label` | Warning text |
| `setup.warning.modal.description` | Shown in the honeypot channel. Leave it empty for Quack's warning, which lists the rule's actions. (prefilled with the current custom warning) |

An area panel is: the area's name as the heading (its `setup.button.*`
label), the result of the last change, its `explain` line, its
`setup.home.*` state line, its selects, its buttons, and **Back**.

Problems with a settings value (`setting.problem`, one variant per
`SettingsProblem`, the `message` of a `validation_failed` field on the
dashboard's settings):

| Code | Text |
| --- | --- |
| `too_many_roles` | Choose at most 25 roles. |
| `duplicate_role` | Choose each role once. |
| `everyone_role` | @everyone can't be a staff role. |
| `unknown_role` | One of these roles isn't in the server anymore. Choose a current role. |
| `invite_url` | `setup.invite.invalid` |
| `too_long` | Use at most {max} characters. |
| `retention_days` | Keep transcripts for 1 to 365 days. |

Created channels (name, topic, first message):

| Id | Name | Topic | First message |
| --- | --- | --- | --- |
| `channel.log` | `moderation-log` | Quack's moderation log: cases, actions, appeals, and setting changes. | ### Moderation log\nQuack posts moderation log entries here. Only staff can see this channel. |
| `channel.appeals` | `appeals` | Appeals for moderators to review. | ### Appeals\nNew appeals appear here. Accept or reject them here or on the dashboard. |
| `channel.evidence` | `evidence` | Copies of evidence files for Quack cases. | ### Evidence\nQuack copies evidence files here so they last after the original message is deleted. Please don't delete messages here. |
| `channel.ticket_entry` | `support` | Need help from the staff? Open a ticket below. | (the ticket panel, §12.13) |
| `channel.ticket_queue` | `ticket-log` | New tickets and their transcripts. | ### Tickets\nNew tickets appear here. Open a ticket's thread to reply. |
| `channel.honeypot` | `honeypot` | Don't post here. Read the warning below. | (the honeypot warning, §12.13) |
| `channel.activity_log` | `server-log` | Message edits and deletions, members joining and leaving, bans, and server changes. | ### Server activity log\nQuack posts server activity here. Only staff can see this channel. |

### 12.12 Help, rules, lists

| Id | Text |
| --- | --- |
| `help.overview` | ### Quack\nQuack keeps track of moderation cases. Rules managers define the rules on the dashboard, moderators apply them, and Quack carries out the rule's action.\n**Moderators:** /case add, /case view, /case list, /case history, /case void, /appeals\n**Rules:** /rules (rules are edited on the dashboard)\n**Setup:** /setup (needs Manage Server)\nUse /help with a topic for more. |
| `help.cases` | ### Cases\nUse /case add, or right-click a message or member and choose **Apps → Add case**. Choose a rule. Quack counts the member's valid cases under that rule, including the new case, and uses that count to pick the level and carry out its action. Set private to True to show the receipt only to you.\nWhen you add a case from a message, Quack saves that message as evidence. Add more with **Add evidence** on the case. Members can see the case context and evidence on their case page; Quack hides staff names and message authors there.\nMade a mistake? Use /case void or **Void case**. The case stays in the history but stops counting. Open the case afterwards to check whether Quack removed its timeout or ban; a kick can't be undone.\nIf an action fails, open the case to see which actions you can retry or dismiss. Open Failed actions on the dashboard to review every outstanding failure. |
| `help.rules` | ### Rules\nA rule is one kind of problem, like spam. Each rule has levels that start at a case count, for example: at 1 case, a warning; at 3 cases, a 1-hour timeout; at 5 cases, a ban. Quack counts each member's valid cases under the rule, including the new one; voided cases don't count. A rule can count only recent cases, like the last 30 days.\nRules are edited on the dashboard, and editing a rule never changes old cases. Use /rules to see this server's rules. |
| `help.appeals` | ### Appeals\nMembers can appeal eligible cases once, from Quack's DM or the dashboard, even after a ban. Use /setup to choose where new appeals appear. Moderators can also use /appeals or the dashboard. Accepting an appeal voids the case so it stops counting. Open the case to check whether its timeout or ban was removed. |
| `help.setup` | ### Setup\nUse /setup to pick channels or let Quack create them, and to turn on tickets, the honeypot, and the server activity log. Keep the log, appeal, evidence, ticket staff, and server activity log channels staff-only. Members need access to the ticket entry and honeypot channels. You can also change these settings on the dashboard. |
| `help.dashboard_link` | Open the dashboard |
| `rules.heading` | ### Rules |
| `rules.row` | **{name}** · {levels, `level.summary.item` joined with " · "} · [Open rule]({url}) |
| `rules.footer` | -# Rules are edited on the dashboard.{" 1 archived rule is there too." / " {count} archived rules are there too." when any}{" Page {page} of {pages}." when more than one page} |
| `rules.empty` | This server has no active rules. Create one on the dashboard. |
| `case.list.heading` | ### Recent cases |
| `case.list.heading_member` | ### Recent cases about {member} |
| `case.list.heading_rule` | ### Recent cases under {rule} |
| `case.list.heading_both` | ### Recent cases about {member} under {rule} |
| `case.list.row` | **#{number}** · {member} · {rule name, not bold} · {date}{" · Voided"}{" · Imported"} |
| `case.list.empty` | No cases match these filters. Try choosing a different member or rule. |
| `case.history.heading` | ### Case history for {member} |
| `case.history.totals` | one: 1 case · {valid} valid · {voided} voided / other: {total} cases · {valid} valid · {voided} voided |
| `case.history.empty` | {member} has no cases in this server. |

Lists show 10 rows per page; when there is more than one page,
`word.page` follows as subtext, then the `select.open_case` select and
`button.previous` / `button.next`. `/rules` (10 rules per page) puts the
page counter in its footer instead. `/help` messages end with the
`help.dashboard_link` link button.

Problems with a rule definition (`rule.problem`, one variant per
`RuleProblemCode`, the `message` of a `validation_failed` field on the
dashboard's rule editor):

| Code | Text |
| --- | --- |
| `name_length` | Give the rule a name of 1 to 100 characters. |
| `description_length` | Keep the description to 500 characters or fewer. |
| `reason_length` | Give the rule a reason of 1 to 1,000 characters. |
| `counting_window` | Count cases from the last 1 to 36,500 days, or count every case. |
| `too_many_fields` | A rule can have at most 10 context fields. |
| `field_key` | Use 2 to 64 lowercase letters, digits, dashes, or underscores for the field name in the API, starting with a letter or digit. |
| `field_key_duplicate` | Another field of this rule already uses this field name. |
| `field_label_length` | Give the field a label of 1 to 100 characters. |
| `level_count` | A rule needs 1 to 10 levels. |
| `first_level_not_one` | The first level must start at a case count of 1. |
| `level_case_count` | Start each later level at a case count from 2 to 1,000,000. |
| `level_order` | Start each level at a higher case count than the level before it. |
| `timeout_duration` | A timeout lasts from 1 second to 28 days. |
| `ban_delete_window` | A ban can delete at most the last 7 days of messages. |

### 12.13 Module messages

Tickets:

| Id | Text |
| --- | --- |
| `ticket.panel.heading` | ### Need help? |
| `ticket.panel.text` | Open a ticket to talk privately with the moderators. |
| `ticket.panel.off` | Tickets are turned off right now. (replaces the text; no button) |
| `ticket.panel.button` | Open a ticket |
| `ticket.panel.moved` | Tickets moved to {channel}. Open a ticket there. |
| `ticket.open.opening` | Quack is opening your ticket. Look for a private thread in {entry channel}. Quack will mention you there. |
| `ticket.open.exists` | You already have an open ticket: {thread}. |
| `ticket.open.closing` | Your previous ticket is still closing. Select Open a ticket again after it closes. |
| `ticket.open.disabled` | Tickets are turned off in this server. |
| `ticket.open.failed` | Quack couldn't open your ticket. Ask a server administrator to check Quack's permissions in {entry channel}. |
| `ticket.welcome` | {member}, your ticket is open. Tell the moderators what you need help with here. (button `ticket.button.close`; the one message whose mention pings) |
| `ticket.button.close` | Close ticket |
| `ticket.button.open_thread` | Open thread |
| `ticket.button.fix_access` | Restore private access |
| `ticket.post.heading` | ### Ticket from {member} |
| `ticket.post.open` | Ticket open · {thread} · opened {time} ("Ticket open · opened {time}" when the thread isn't known) |
| `ticket.post.opening` | Opening… |
| `ticket.post.open_failed` | Quack couldn't open this ticket. Check that Quack can create private threads in the ticket entry channel. |
| `ticket.post.closing` | Closing. Quack is saving the transcript. |
| `ticket.post.closed` | Closed by {actor} {time}. ("Closed." when unknown) |
| `ticket.post.transcript_attached` | The transcript is attached. |
| `ticket.post.transcript_dashboard` | The transcript is available on the dashboard until {date}. |
| `ticket.post.transcript_expired` | The transcript expired on {date}. |
| `ticket.post.partial` | `ticket.post.partial.history` or `ticket.post.partial.trimmed` |
| `ticket.post.partial.history` | Some messages may be missing because Quack couldn't read the full thread. |
| `ticket.post.partial.trimmed` | Some messages are missing because Quack reached the 8 MB limit. |
| `ticket.post.thread_not_deleted` | Quack closed the ticket but couldn't delete the thread. Ask someone with Manage Threads to delete {thread}. ("… to delete it." when the thread isn't known) |
| `ticket.close.closing` | Quack is closing this ticket and saving the transcript. |
| `ticket.close.not_open` | This ticket is already closed or closing. |
| `ticket.close.not_allowed` | Only the person who opened this ticket or a Quack moderator can close it. Ask a moderator if you need help. |
| `ticket.fix.done` | {member} can open the ticket thread.{" Quack removed 1 person who didn't have permission to read this ticket." / " Quack removed {count} people who didn't have permission to read this ticket." when it removed anyone} |
| `ticket.fix.failed` | Discord didn't let Quack update ticket access. `access.quack_channel_needs` (staff, Manage Threads, the thread's channel) |
| `dm.ticket_closed.heading` | ### Your ticket in {server} is closed |
| `dm.ticket_closed.text` | The attached file is a copy of the conversation. |
| `ticket.thread_name` | ticket-{ticket u22} |
| `ticket.transcript.header` | Ticket transcript · {server} · opened {transcript time} by {member name} (Discord member ID {member id}) · closed {transcript time} |
| `ticket.transcript.line` | [{transcript time}] {author name} ({author id}): {text} |
| `ticket.transcript.file_line` | `  File: {name} ({size})` (size as "512 bytes", "1.5 KB", "8 MB") |
| `ticket.transcript.partial` | `ticket.post.partial.history` or `ticket.post.partial.trimmed` |

`{transcript time}` is UTC as "2026-11-17 14:05:09 UTC". The staff post
shows the transcript, partial, and thread lines only once closed (and not
for a ticket that failed to open); its buttons (Open thread, Restore private access,
Close ticket) only while open.

Server activity log (posted in the routed channel):

| Id | Text |
| --- | --- |
| `activity.message_edited` | A message from {author} was edited in {channel}. |
| `activity.before` | **Before:**\n> {text} |
| `activity.before_unknown` | **Before:** Quack hadn't seen the earlier text. |
| `activity.message` (before, no text) | **Before:** The message had no text. |
| `activity.after` | **After:**\n> {text} |
| `activity.after_empty` | **After:** The message has no text. |
| `activity.message` (after, not sent) | **After:** Discord didn't include the new text. |
| `activity.message_deleted` | A message from {author} was deleted in {channel}.{"\n> {text}" when it had text} |
| `activity.message_deleted_unknown` | A message was deleted in {channel}. Quack hadn't seen it, so its author and text are unavailable. |
| `activity.messages_deleted_unknown` | {count} messages were deleted in {channel}. Quack hadn't seen these messages, so their authors and text are unavailable. |
| `activity.messages_bulk_deleted` | {count} messages were deleted in {channel}. Then `activity.bulk_known`, or, when Quack had seen none, "Quack hadn't seen these messages, so their authors and text are unavailable." |
| `activity.bulk_known` | one: Quack had seen 1 of them: / other: Quack had seen {known} of them: (more than 20: "Quack had seen {known} of them. Showing excerpts from 20:") |
| `activity.bulk_row` | {author}: {text on one line, cut to 70 characters so 20 rows always fit} ("(no text)" for a message without text, "(text not shown)" when the text is hidden or unknown; at most 20 rows) |
| `activity.files` | Files: {names, each linked to its original URL, comma-separated} |
| `activity.embeds` | Link previews: {types} |
| `activity.text_hidden` | This server's activity log hides message text. |
| `activity.message_ids` | -# Deleted message IDs: {ids, comma-separated, at most 50} |
| `activity.subtext` | -# Message ID {message_id} · {datetime} |
| `activity.member_joined` | {member} joined the server. Their account was created {time}. |
| `activity.member_left` | **{username}** left the server. Discord member ID: {member_id}. |
| `activity.member_banned` | {member} was banned by {actor}. |
| `activity.member_unbanned` | {member} was unbanned by {actor}. |
| `activity.reason` | **Reason:**\n> {reason} |
| `activity.server_changed` | The server settings changed. |
| `activity.channel_created` | The channel {channel} was created. |
| `activity.channel_changed` | Settings changed for {channel}. |
| `activity.channel_deleted` | The channel #{name} was deleted. |
| `activity.trimmed` | This post shows excerpts. Quack shortened some message text to fit in Discord. (a post shortened to fit; there is no dashboard history of activity posts) |

An edit with both texts hidden shows `activity.text_hidden` once instead of
Before and After. Posts about several messages end with
`activity.message_ids` and then `-# {datetime}`; posts about one message end
with `activity.subtext`. Files and link previews follow the text.

Honeypot:

| Id | Text |
| --- | --- |
| `honeypot.warning.heading` | ### Don't post here |
| `honeypot.warning.one_outcome` | If you post here, Quack can add a case under {rule}. This rule can {outcome}. |
| `honeypot.warning.many_outcomes` | If you post here, Quack can add a case under {rule}. This rule can {outcomes, in level order, each once, joined with ", " and " or "}. What happens depends on how many of your cases count under it. |
| `honeypot.warning.warning_only` | If you post here, Quack can add a case under {rule}. This rule can give you a warning. |
| `honeypot.warning.tail` | This channel catches spam accounts. Quack ignores posts from staff and bots. |
| `honeypot.warning.count` | -# zero: No honeypot cases yet. / one: 1 honeypot case so far. / other: {count} honeypot cases so far. |
| `honeypot.warning.off` | ### The honeypot is off\nPosts here don't add cases right now. |
| `honeypot.outcome.ban` | ban you |
| `honeypot.outcome.kick` | remove you from the server |
| `honeypot.outcome.timeout` | time you out for {duration} |
| `honeypot.outcome.warning` | give you a warning |

The warning describes what the rule can do, never that an action will
succeed. A custom warning replaces the heading and the generated sentences; the
count line is always appended. While the honeypot is off, the warning shows
only `honeypot.warning.off` (a custom warning is not shown).

### 12.14 API messages

The `message` of each API error (§14.6). Where an `error.*` or `refusal.*`
id fits, the API uses it (domain errors through
`quack_text::error::for_domain`, §12.3); the API-only texts are:

| Id | Text |
| --- | --- |
| `api.sign_in_required` | Sign in with Discord to continue. |
| `api.session_expired` | Your session has ended. Sign in with Discord again. |
| `api.origin_rejected` | Quack only accepts changes made from its own dashboard. Open the dashboard and try again there. |
| `api.csrf_rejected` | Quack couldn't verify your sign-in. Reload this page and try again. |
| `api.not_found` | Quack couldn't find what you opened. Go back to the list and open it again. |
| `api.server_not_found` | Quack couldn't open that server. Choose a server from your dashboard. (It doesn't say whether Quack left or the reader isn't a member.) |
| `api.file_unavailable` | This file is no longer available. Go back to the page where you opened it to check for another copy. |
| `api.validation_failed` | Check the marked fields and save again. (the field messages are in `details`) |
| `api.range_invalid` | Choose an end after the start, at most 366 days later. (statistics `from`/`to`) |
| `api.rule_file_wrong_format` | This file isn't a Quack rule file. Choose a file you exported from a rule's page. |
| `api.rule_file_unsupported_version` | This rule file comes from a different version of Quack. Export the rule again and import the new file. |
| `api.value_invalid` | Quack couldn't read this value. Go back, choose it again, and retry. (an ID or filter value in the path or query) |
| `api.invalid_json` | Quack couldn't read your changes. Reload this page and try again. |
| `api.body_too_large` | Quack couldn't save this because it's over 256 KB. Shorten the text or save fewer changes at once. |
| `api.unsupported_media_type` | Quack couldn't read your changes. Reload this page and try again. |
| `api.idempotency_key_required` | Quack didn't save this change. Reload the dashboard and try again. |
| `api.idempotency_key_reused` | Quack didn't save this change because it conflicts with an earlier save. Reload the dashboard, check what's saved, and try again. |
| `api.rate_limited` | one: Please wait 1 second before trying again. / other: Please wait {seconds} seconds before trying again. |
| `api.rule_changed` | Someone else saved this rule. Reload it to see the latest version before you save. |
| `api.rule_name_taken` | Another active rule is already called {name}. Choose a different name. |
| `api.rule_used_by_honeypot` | The honeypot uses this rule, so it can't be archived. Choose another rule for the honeypot first. |
| `api.required_field_on_honeypot_rule` | The honeypot uses this rule, so its fields can't be required. Make the fields optional or choose another rule for the honeypot. |
| `api.too_many_rules` | A server can have at most 100 active rules. Archive one first. |
| `api.settings_changed` | Someone else changed the staff roles. Reload the server settings and check the current roles before saving again. |
| `api.appeal_exists` | You already appealed this case. Open your appeal to see the latest update. |
| `api.appeal_not_allowed` | This case can't be appealed. You can still review it on the dashboard. |
| `api.module_not_ready` | {Module} can't be turned on yet. Open the settings on the dashboard to see what needs fixing. |
| `api.discord_unavailable` | Quack couldn't reach Discord, so nothing changed. Try again in a moment. |
| `api.unavailable` | Quack is having trouble right now, so nothing changed. Try again in a moment. |
| `api.internal_error` | Something went wrong inside Quack. Reload the page and check whether your changes were saved before trying again. |

`{name}` in `api.rule_name_taken` is bold on Discord, like `{rule}`.
Transport codes that reuse another text (snapshot `api.code`):
`action_not_found` uses `error.action_not_found`, `context_changed` uses
`error.context_changed`, `case_numbers_exhausted` uses
`error.case_numbers_exhausted`, `discord_unavailable` uses
`error.discord_unavailable`, and `unsupported_media_type` uses the
`api.invalid_json` words.

Sign-in pages (`/login?error=…`, route 2), not API errors:

| Id | Text |
| --- | --- |
| `api.sign_in_failed` | Discord sign-in didn't finish. Choose Sign in with Discord to try again. |
| `api.sign_in_denied` | Discord sign-in was cancelled. Choose Sign in with Discord when you're ready. |

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
| `appeal_invite_url` | NULL | Accepted input: `discord.gg/<code>`, `discord.com/invite/<code>`, or `discordapp.com/invite/<code>`, with or without `https://` and `www.`; code 2–64 of `[A-Za-z0-9_-]`. Stored as `https://discord.gg/<code>`. | `manage_settings` |
| `dm_intro` | NULL | 0–1,000 characters; empty stores NULL. | `manage_settings` |
| `dm_footer` | NULL | 0–500 characters; empty stores NULL. | `manage_settings` |
| `starter_rule_id` | set at bootstrap | read-only | — |
| `starter_notice_acknowledged_at` | NULL | Set by the acknowledge endpoint; first timestamp kept. | `manage_settings` |

Several settings may name the same channel.

**Problem codes.** A rejected value is reported in the `fields` of
`400 validation_failed` (§14.6) with one of these codes
(`quack_core::settings::SettingsProblem`; message `setting.problem`,
§12.11). Channel settings report `ChannelProblem` codes instead (§13.1.3).

| Code | When | Checked by |
| --- | --- | --- |
| `too_many_roles` | More than 25 roles in a staff role list. | `check_role_list` |
| `duplicate_role` | A role appears twice in a staff role list. | `check_role_list` |
| `everyone_role` | @everyone in a staff role list. | `check_role_list` |
| `unknown_role` | A role the server doesn't have and that wasn't already saved. | `check_role_list` |
| `invite_url` | Not an accepted invite link (above), or a code outside 2–64 of `[A-Za-z0-9_-]`. | `normalize_invite` |
| `too_long` | `dm_intro`, `dm_footer`, or the honeypot warning (§13.4.1) is longer than its limit. | `normalize_optional_text` |
| `retention_days` | Ticket `transcript_retention_days` outside 1–365 (§13.2.1). | `check_retention_days` |

`check_role_list` reports the first problem it finds in list order.

#### 13.1.2 Updating settings

`PATCH /api/guilds/{guild_id}/settings` writes only the fields it contains:

1. Gather the Discord facts the sent fields need (channels, overwrites,
   roles) before any lock.
2. One transaction: `SELECT … FROM guild_settings WHERE guild_id = $1 FOR
   UPDATE`.
3. When the request contains `moderator_role_ids` or
   `rules_manager_role_ids`, it must also contain `expected_version`; a
   mismatch with `guild_settings.version` → `409 settings_changed` and
   nothing is saved. This stops a stale page from putting back roles an
   administrator just removed.
4. Validate every sent field (pure, `quack_core::settings`) against the
   locked row and the facts from step 1; channel checks use the moderator
   roles being saved when the request contains them, else the locked ones.
   Failures are reported per field and nothing is saved.
5. Update the sent columns, `version = version + 1`, `updated_at`,
   `updated_by`; bind or move tracked messages the change affects (§8.3,
   §10.2); log `settings_changed` with the field names; `NOTIFY
   quack_settings`.

Module enable and disable are separate endpoints (§13.1.7). Module settings
(`PUT …/modules/{module}/settings`) follow the same five steps on their own
row.

#### 13.1.3 Staff-only channel check

`quack_core::permissions::check_staff_channel(channel: &StaffChannelFacts, moderator_role_ids: &[RoleId], quack_needs: Permissions) -> Vec<ChannelProblem>`,
pure over channel and role facts. A staff destination passes when:

1. It is a text channel (type 0) in this server (`not_text`).
2. The @everyone overwrite denies View Channel (`everyone_can_view`).
3. Every role overwrite that allows View Channel is for a role that is staff
   for Discord access: it has Administrator or Moderate Members, or it is a
   configured moderator role (`role_can_view`).
4. Every member overwrite that allows View Channel is for Quack, the owner,
   or a member who is staff for Discord access (`member_can_view`).
5. Quack's computed permissions in the channel include every permission in
   `quack_needs` (`quack_missing`).

A channel that is not text or belongs to another server reports only
`not_text`. Two more `ChannelProblem` codes come from server health, not
from this check: `inaccessible` (Discord answers 403 for the channel, so
Quack can't check it; the setting is kept, §13.1.5) and `missing` (the
channel no longer exists). Messages: `setup.problem.*` (§12.11).

The check runs when a channel is saved and again, from current facts (§2),
before every post to a staff destination (log channel, appeal posts,
evidence copies, ticket staff posts, server activity posts). It protects
new posts only: a channel later opened to more people shows them what was
already posted, and Discord offers no way to take that back (§20.4).
Changing staff roles later never edits existing channel overwrites.

#### 13.1.4 Creating channels

`/setup` "Create" buttons call `quack_engine::setup::create_channel(kind)`.
Creates for one server run one at a time (an in-process lock; Quack runs
as one process, §17.5).

1. **Find first.** List the server's channels. A text channel with the
   catalog name (§12.11) whose ID's timestamp is under 10 minutes old and
   that has a member overwrite for Quack (every channel Quack creates has
   one) is a channel Quack just made: adopt it (`setup.adopted`). This makes
   a double click, or a retry after a lost answer, harmless.
2. **Check.** Creation needs Manage Channels, and Discord lets Quack allow
   or deny only permissions it holds itself; a missing permission →
   `setup.create_failed.permission` naming it.
3. **Create** the channel with the name, topic, and overwrites below.
   `Unknown` → `setup.create_unconfirmed` (the next click finds it in step
   1).
4. **Save** the setting in one transaction (§13.1.2 steps 2–5), including
   the tracked message the channel holds (the ticket panel, the honeypot
   warning), so its first post is the normal sync (§10.3). The channel
   intro message of a staff destination (§12.11 "First message") is then
   created following §10.4; a failure there doesn't undo anything.
5. Answer `setup.created` (staff destinations) or `setup.created_public`
   (ticket entry, honeypot).

| Kind | @everyone | Quack (member overwrite) | Staff roles* |
| --- | --- | --- | --- |
| Staff destination (log, appeals, evidence, ticket queue, server activity) | deny View Channel | allow View Channel, Send Messages, Read Message History, Attach Files, Embed Links | allow View Channel, Send Messages, Read Message History |
| Ticket entry | allow View Channel, Read Message History, Send Messages in Threads; deny Send Messages | allow View Channel, Send Messages, Read Message History, Create Private Threads, Manage Threads, Send Messages in Threads | — |
| Honeypot | allow View Channel, Send Messages, Read Message History | allow View Channel, Send Messages, Read Message History, Manage Messages | — |

\* Configured moderator roles that still exist; when none exist, every role
with Administrator or Moderate Members.

#### 13.1.5 Channel reconciliation

A channel deleted while Quack is running (`CHANNEL_DELETE`), or one missing
from the `GUILD_CREATE` channel list and confirmed gone by `GET
/channels/{id}` answering 404 Unknown Channel, clears every setting that
points at it, in one transaction per server. Each clear is
compare-and-clear: it changes a setting only if it still holds that
channel. A 403 or any other answer is not a deletion: the setting is kept
and server health reports the channel as inaccessible.

| Setting | Effect |
| --- | --- |
| `log_channel_id`, `appeal_channel_id`, `evidence_channel_id` | Set NULL; log `channel_setting_cleared`. |
| Ticket entry or queue channel | Set NULL; tickets disabled (`channel_deleted`); log `module_disabled`. |
| Honeypot channel | Set NULL; honeypot disabled (`channel_deleted`); warning row `gone`; log `module_disabled`. |
| Server activity route | Route removed; when no route remains, the module is disabled (`channel_deleted`); log `module_settings_changed` or `module_disabled`. |
| A ticket's thread (`THREAD_DELETE`, or on `GUILD_CREATE` an `opening` or `open` ticket whose thread is not in the event's thread list and `GET /channels/{thread}` answers 404) | The ticket closes (§13.2.2). |

Tracked messages in a deleted channel are handled by their sync (§10.3).

#### 13.1.6 The `Module` contract

Each optional module is one crate exporting one `Module` value, defined in
`quack-engine`. What a module handles is static data, so routing has one
source:

```rust
/// What a module handles. The engine builds one dispatch table from every
/// module's spec at startup and refuses to start on a duplicate key.
pub struct ModuleSpec {
    pub name: ModuleName,
    pub setup_area: SetupArea,
    pub events: &'static [EventKind],
    pub component_codes: &'static [&'static str],
    pub job_kinds: &'static [JobKind],
}

pub trait Module: Send + Sync + 'static {
    fn spec(&self) -> &'static ModuleSpec;
    /// One gateway event of a kind in `spec().events`.
    fn on_event<'a>(&'a self, ctx: &'a Ctx, event: &'a ModuleEvent) -> BoxFuture<'a, ()>;
    /// A button, select, or modal with one of `spec().component_codes`.
    fn on_component<'a>(&'a self, ctx: &'a Ctx, call: &'a ComponentCall) -> BoxFuture<'a, Msg>;
    /// One claimed job of a kind in `spec().job_kinds`.
    fn run_job<'a>(&'a self, ctx: &'a Ctx, job: &'a ClaimedJob) -> BoxFuture<'a, JobResult>;
    /// Settle a job whose budget is spent, without calling Discord (§16.1).
    fn finalize_job<'a>(&'a self, ctx: &'a Ctx, job: &'a ClaimedJob) -> BoxFuture<'a, JobResult>;
    /// Its `/setup` panel, and one `/setup` operation (returns the re-rendered panel).
    fn setup_panel<'a>(&'a self, ctx: &'a Ctx, guild: GuildId) -> BoxFuture<'a, Msg>;
    fn on_setup<'a>(&'a self, ctx: &'a Ctx, who: &'a Principal, op: &'a SetupOp) -> BoxFuture<'a, Msg>;
    /// Why it can't be turned on (empty = it can), from facts and the locked settings row.
    fn enable_problems(&self, facts: &GuildFacts, settings: &ModuleSettings) -> Vec<ModuleProblem>;
    /// Writes that go with turning it on or off (bump a panel), inside that transaction.
    fn on_enabled_changed<'a>(&'a self, tx: &'a mut Tx, guild: GuildId, enabled: bool) -> BoxFuture<'a, Result<(), DbError>>;
}
```

`ModuleEvent` and `ComponentCall` are engine types; `quack-bot` converts
Twilight's types into them, so neither the engine nor a module sees
Twilight. `Ctx` gives access to the database, the `Discord` port, the
caches, the engine's use cases (the honeypot's case entry), and
`quack-text`. HTTP routes for a module are thin handlers in `quack-api`
that call the module crate's public service functions and DTOs (§14.3); a
module never depends on `quack-api`.

#### 13.1.7 Enable and disable

`POST /api/guilds/{guild_id}/modules/{module}/enable` and `…/disable`
(`manage_settings` + 2FA), also used by `/setup`:

- Enable: gather facts; one transaction locks the module's settings row
  (and, for the honeypot, its rule `FOR SHARE`, §4.12), computes
  `enable_problems` against them and the facts, and, when there are none, sets `enabled = true`, clears `disabled_reason`, bumps `version`,
  runs `on_enabled_changed`, logs `module_enabled`, and `NOTIFY
  quack_settings`. Otherwise `409 module_not_ready` with `problems`
  (§12.11 `module.problem.*`).
- Disable: the same transaction shape: `enabled = false`, `disabled_reason
  = NULL`, `on_enabled_changed`, log `module_disabled`. Saved settings are
  kept.
- Enabling an enabled module or disabling a disabled one is a no-op that
  returns the current state and writes nothing.

Work already queued for a module when it is turned off finishes or stops
by its own rule: honeypot hits admitted under the old configuration fail
`policy_changed` (§13.4.2); open tickets stay open and can still be closed;
server activity posts already queued are sent.

### 13.2 Tickets

Private support threads between a member and staff. Tickets are separate
from cases and appeals.

#### 13.2.1 Settings (`ticket_settings`)

| Column | Default | Validation |
| --- | --- | --- |
| `enabled` | false | via enable/disable only |
| `disabled_reason` | NULL | `channel_deleted`, `quack_left` |
| `entry_channel_id` | NULL | text channel in the server |
| `queue_channel_id` | NULL | staff-only; Quack needs View Channel, Send Messages, Attach Files |
| `transcript_retention_days` | 90 | 1–365 |

Enable problems: entry missing, queue missing, entry equals queue, Quack
lacking View Channel, Send Messages, Read Message History, Create Private
Threads, Send Messages in Threads, or Manage Threads in the entry channel,
queue not staff-only or lacking permissions.

Changing the entry channel moves the panel (§10.3). `PUT
…/modules/tickets/settings` validates every field even while disabled.

#### 13.2.2 States

`ticket_state`: `opening`, `open`, `closing`, `closed`. A partial unique
index allows one ticket per member in `opening`, `open`, or `closing`, so a
member has at most one live ticket and the "slot" is freed exactly when the
ticket reaches `closed`. Every non-closed state ends: each has a job with a
budget whose finalization moves it on.

| From | Event | To |
| --- | --- | --- |
| (none) | Member clicks **Open a ticket** | `opening` (+ `open_ticket` job) |
| `opening` | Thread ready, member added, welcome posted | `open` (+ the `ticket_post` row and its sync) |
| `opening` | A step definitely refused, the thread was deleted, or the job's budget is spent, and no thread exists | `closed` (`close_reason = open_failed`) |
| `opening` | The same, but a thread exists | `closing` (`close_reason = open_failed`; the close job only deletes the thread) |
| `open` | Member or moderator closes (button or dashboard) | `closing` (+ `close_ticket` job) |
| `open` | Thread deleted (`THREAD_DELETE`) | `closing` (closed by Quack) |
| `closing` | Close job settles, or its budget is spent | `closed` |

#### 13.2.3 Opening

Component `to`: defer ephemerally. Tickets disabled → `ticket.open.disabled`.
Transaction: insert `tickets` (`opening`) and the `open_ticket` job. A
unique violation on `tickets_one_live` → load the member's live ticket and
answer `ticket.open.exists` (`open`), `ticket.open.closing` (`closing`), or
`ticket.open.opening` (`opening`); any other error is `error.internal`.
Otherwise answer `ticket.open.opening`; the job does the rest, and the
welcome message mentions the member in the new thread.

`open_ticket` job (budget 4, backoff 2 s, 10 s, 30 s):

1. State not `opening` → `Done`.
2. **Thread.** No `thread_id`: look for threads named `ticket-<ticket u22>`
   (the full ID, so names never collide) under the entry channel, among
   active threads (`GET /guilds/{g}/threads/active`) and archived private
   threads (`GET /channels/{entry}/threads/archived/private`). One found →
   adopt it; several → adopt the oldest and delete the others (they are
   Quack's own duplicates); none → create a private thread (type 12,
   `invitable: false`, `auto_archive_duration: 1440`). Save `thread_id` at
   once. Refused → the open fails (§13.2.2).
3. Add the member (`PUT /channels/{t}/thread-members/{m}`, idempotent).
   Refused → the open fails.
4. Post `ticket.welcome` with a **Close ticket** button (pings the member),
   following §10.4 (`welcome_started_at`, nonce = ticket u22 + `w`).
5. Transaction: `open`, `opened_at`; insert the `ticket_post` row in the
   queue channel and its `sync_message` job; log `ticket_opened`. The ticket
   post (`ticket.post.*`) shows the member, the state, and the transcript,
   with **Close ticket** (`tc`) and **Fix access** (`tf`) while the ticket is
   `open` and an **Open thread** link; each click re-checks the clicker.

Finalization (budget spent): the open fails (§13.2.2).

#### 13.2.4 Journal and closing

- **Journal.** `MESSAGE_CREATE` in the thread of an `opening`, `open`, or
  `closing` ticket that is not yet frozen inserts `ticket_messages`
  (message ID, author, author name, text, file names and sizes, sent time;
  `ON CONFLICT DO NOTHING`). Edits and deletes never change journal rows,
  so the transcript keeps original wording. `journal_gaps` is set when an
  event for the thread was dropped (§11.9), an insert failed after 3 tries
  (recorded once the database accepts writes again), or the gateway
  session was invalidated while the ticket was open.
- **Closing.** Component `tc` or `POST …/tickets/{id}/close`: the ticket's
  member or `moderate` + 2FA. Transaction: lock the ticket; it must be
  `open` (else `ticket.close.not_open`); `closing`, `closed_by`,
  `close_requested_at`, `close_ticket` job. Answer `ticket.close.closing`.

`close_ticket` job (each step records its completion column, so a retry
resumes; budget 6, backoff 5 s, 30 s, 2 min, 10 min, 1 h):

1. **Freeze** the thread: `PATCH /channels/{t}` `{archived: true, locked:
   true}`, so members can no longer post; 404 → the thread is gone.
   `frozen_at`. The journal admits nothing after this.
2. **Transcript** (skipped for `open_failed`; unless `transcript_saved_at`):
   wait until this server's event queue has handled every event received
   before `frozen_at` (at most 5 s). Read the thread's history, newest
   first, at most 10 pages of 100, and merge with the journal by message ID:
   journal text wins, history adds messages the journal lacks. `complete` =
   the history reached the thread's first message, or (history unavailable)
   the journal has no gaps. Render with `ticket.transcript.*` (UTC times);
   over 8 MiB, cut at a UTF-8 boundary and mark partial (`trimmed`). Insert
   `ticket_transcripts` (content, complete, partial cause, `expires_at = now
   + retention`). `transcript_saved_at`.
3. **Close** (one transaction): `closed`, `closed_at`; set the journal rows'
   `expires_at`; unless `open_failed`, insert the `ticket_closed` DM with its
   job (`ON CONFLICT DO NOTHING` on `ticket_id`); bump the `ticket_post`
   (its sync attaches the transcript as a file once, §10.1
   `attachment_id`); log `ticket_closed`. The slot is free from here.
4. **Delete the thread**: `DELETE /channels/{t}`; 404 → done; 403 →
   `thread_delete_failed = true` (the ticket post says so). `thread_deleted_at`.
   The ticket is already `closed`; a retry runs only this step.

Finalization (budget spent, database only): a ticket not yet `closed`
saves a journal-only transcript (partial, history unavailable) if none was
saved, then runs step 3; a thread not yet deleted gets
`thread_delete_failed = true`. No ticket stays `opening` or `closing`.

- **Fix access** (component `tf`, `moderate` + 2FA, ticket `open`):
  unarchive the thread if it auto-archived, re-add the member, list thread
  members, and remove everyone who is not the member, Quack, the owner, or
  staff for Discord access by current facts. Answer `ticket.fix.done` with
  the count removed, or `ticket.fix.failed`. Not logged.

#### 13.2.5 Storage, jobs, retention

Tables `ticket_settings`, `tickets`, `ticket_messages`,
`ticket_transcripts` (§15). Jobs `open_ticket`, `close_ticket`. The tracked
messages `ticket_panel` and `ticket_post`. Housekeeping deletes
`ticket_transcripts` and `ticket_messages` rows past `expires_at`, and
journal rows of tickets open for more than 90 days beyond their newest
50,000 rows (marking `journal_gaps`); tickets themselves are kept. The
dashboard serves a transcript to the ticket's member and to staff until
`expires_at`; after that it answers `file_unavailable`. Discord copies (the
staff post's file, the member's DM) are not deleted.

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
staff-only or Quack lacking View Channel and Send Messages there. Quack
lacking View Audit Log is not an enable problem: the module runs without
`member_banned` and `member_unbanned`, and its settings and server health
show `module.problem.activity_needs_audit_log`.

#### 13.3.2 Behaviour

- **One message cache** (§11.10). Twilight's message cache is off, so the
  module's cache is the only one. Messages are cached on `MESSAGE_CREATE`
  in servers where the module is enabled, for text channels and threads.
- **Redaction first.** Before text is cached or posted it passes
  `quack_core::modules::redact_secrets`, which replaces Discord bot tokens,
  webhook URLs, and `token=`, `password=`, `secret=`, `authorization:`,
  `bearer ` values with `[redacted]` (the Go patterns). The privacy flags
  are applied both when caching and when posting, so turning text off also
  stops it from being cached.
- **Consistent filtering.** Messages written by Quack or by webhooks, and
  messages in a channel that is itself an activity route destination, are
  never posted. They are cached as tombstones (author kind only), so their
  later edits and deletes are recognized and skipped rather than reported
  as unknown. Other bots are logged.
- **Edits.** A field missing from a partial update is unknown, not empty; an
  empty text field is known-empty (`activity.after_empty`). Known fields
  merge into the cached copy one by one. When the text and file list are
  unchanged (an embed-only update), nothing is posted.
- **Deletes.** A cached message posts at once with its author and text.
  Uncached deletes are batched per channel for 60 seconds or until 50 IDs
  (at most 1,000 open batches; more are posted at once unbatched): one ID →
  `activity.message_deleted_unknown`; more →
  `activity.messages_deleted_unknown` with `activity.message_ids`.
- **Privacy flags.** `include_message_content = false` replaces text with
  `activity.text_hidden`; the other flags drop file names (with their
  links) and embed types.
- **Delivery.** In memory, not durable: a bounded queue of 10,000 posts and
  2 sender tasks. At delivery each post re-reads the route from the config
  cache (a changed route sends it to the new channel; a removed route or a
  disabled module drops it) and re-checks the destination (§13.1.3, result
  cached per channel for 60 seconds), then creates the message following
  §10.4 (nonce: event hash, 25 characters). `RetryAfter` or `Unknown` → up
  to 2 more tries within the window; definite refusals are not retried.
  Counted as `quack.activity_log.events{quack.activity.event,quack.activity.stage}`
  with stages `seen`, `posted`, `dropped` (queue full, route gone), `failed`
  (refused or out of tries). Disabling the module or Quack leaving the
  server empties its batches and cache for that server.
- **Graceful shutdown** flushes pending delete batches and drains the queue
  within the shutdown deadline (§17.5).

### 13.4 Honeypot

A trap channel. A post there from a member who is not staff adds a case
under the honeypot's rule through the normal case path.

#### 13.4.1 Settings (`honeypot_settings`)

| Column | Default | Validation |
| --- | --- | --- |
| `enabled` | false | via enable/disable only |
| `disabled_reason` | NULL | `channel_deleted`, `quack_left` |
| `channel_id` | NULL | text channel; Quack needs View Channel, Send Messages, Read Message History, Manage Messages |
| `rule_id` | NULL | active rule of this server with no required fields (§3.8); cleared with "Clear rule" while the honeypot is off |
| `warning_text` | NULL | 0–1,500 characters; NULL or empty uses the generated warning |
| `version` | 1 | incremented by every change, including enable and disable |

Enable problems: channel missing, rule missing, rule with required fields,
Quack lacking channel permissions. Turning it on from `/setup` with no rule
creates the "Honeypot" rule (§3.8). Re-enabling reactivates a `gone`
warning (§10.2).

#### 13.4.2 From post to case

1. `MESSAGE_CREATE` in `channel_id` of a server with the honeypot enabled.
   Ignored when the author is a bot (any bot, including Quack), a webhook,
   or staff for Discord access (owner, Administrator, Moderate Members, a
   configured moderator role; from the message's member payload and the
   config cache). This matches the core rule that cases are never about
   bots (§2.3 check 5).
2. Insert `honeypot_hits` (message ID unique, member, `pending`, the
   honeypot `version` and `rule_id` at that moment) and enqueue the
   `honeypot_case` job keyed `honeypot_case:<guild>:<member>` (coalesced). A
   full gateway queue drops the event and counts it (§11.9).
3. `honeypot_case` job (budget 5, backoff 5 s, 30 s, 2 min, 10 min):
   1. Load the member's `pending` hits, ordered by message ID. This is the
      batch; its identity is its first message ID, which stays the same
      until the batch settles because later hits have larger IDs.
   2. Re-check the poster from current facts (§2): now staff for Discord
      access, or a bot → hits `failed` (`exempt`).
   3. A valid case created from this member's hits under the same honeypot
      `version`, whose first hit is less than 30 seconds older than this
      batch's first hit → attach the batch to that case (a burst is one
      case).
   4. Otherwise apply the rule (§4.2) with source `honeypot`, actor Quack,
      operation key `honeypot:<first message ID>`, the batch's honeypot
      `version` (checked under lock, §4.2 step 8), and the first hit's
      message as evidence.
   5. Created or replayed → hits `case_created` with `case_id`; enqueue
      `honeypot_cleanup` per hit; bump the warning.
   6. A refusal (§2.3), `HoneypotPolicyChanged` (`policy_changed`), or a
      missing or archived rule (`rule_unavailable`) → hits `failed` with the
      code; log `honeypot_case_failed` (for honeypot cases the engine does
      not also write `case_create_refused`). Their messages are kept.
   7. `DiscordUnavailable` → retry. Finalization → hits `failed`
      (`discord_unavailable`), logged.

   The stored `honeypot_hits.failure` is a `quack_core::modules::HoneypotFailure`:
   either `Refused(HoneypotRefusal)` — a member refusal code (§2.3),
   `rule_unavailable`, `policy_changed`, or `discord_unavailable`; logged as
   `honeypot_case_failed` with that reason — or `exempt` (step 3.2, not
   logged) or `imported_pending` (set only by the importer, §19.9). Only
   `HoneypotRefusal` values appear in the `honeypot_case_failed` entry.
4. `honeypot_cleanup` job (keyed `honeypot_cleanup:<guild>:<message>`; budget 10,
   backoff 1, 2, 4, 8, 16, 32, 64, 64, 64 s): while the case's evidence row
   for that message is still `saving`, `Defer` (at most 10 minutes), so the
   post is saved before it is deleted; then delete the message; 404 counts
   as done; record `cleaned_up_at`. Finalization records
   `cleanup_failed_at`; the post stays and staff can delete it.

#### 13.4.3 The warning

The `honeypot_warning` tracked message in the honeypot channel renders
`honeypot.warning.*` from the bound rule's current levels (or the custom
text) plus the count of distinct cases created from hits; while the
honeypot is off it shows only `honeypot.warning.off`. It is bumped by a new
honeypot case, settings changes, enable and disable, edits of the bound
rule, and its own deletion (posted again, §10.3).

---

## 14. HTTP API

The API serves the dashboard only. It lives under `/api` on the same origin
as the dashboard files, served by `quack serve`. axum 0.8, routes in
`quack-api`, documented with utoipa.

### 14.1 Conventions

- JSON in and out. A route that takes a body requires `Content-Type:
  application/json` (else `415 unsupported_media_type`); routes without a
  body (archive, restore, retry, dismiss, undo, enable, disable, logout)
  take none and ignore the header.
- Discord IDs, UUIDs, and context numbers are **strings** in JSON, so no
  client rounds them. Case numbers and versions are JSON numbers; case
  numbers never exceed 2^53 − 1 (§4.4), so they are exact in JavaScript. Timestamps are RFC 3339 UTC with
  milliseconds. Enums are the snake_case values defined in this document.
  Snowflakes are parsed by one checked parser (positive, below 2^63); a
  value that does not fit is `400 validation_failed`, never wrapped.
- `{guild_id}` is the Discord server ID. In staff routes `{case_ref}` is a
  case number, a case UUID, or a legacy ULID (mapped, §19.3). Member routes
  take `{case_id}`: a UUID or a legacy ULID, never a number. Other path IDs
  are UUIDs, or legacy ULIDs for links from the Go era.
- Lists use keyset pagination: `limit` (1–100, default 50) and `cursor`
  (opaque base64url of the last row's sort key plus a hash of the filters;
  a cursor reused with other filters is `400 validation_failed`); responses
  carry `next_cursor` (null on the last page). No offsets. Lists that are
  bounded by a limit (a case's actions, a rule's versions up to its version
  count, settings directories) are returned whole.
- Every write (POST, PUT, PATCH, DELETE) under `/api/guilds` and `/api/me`
  requires `Idempotency-Key` (8–200 printable ASCII characters) and is
  idempotent through `operation_keys` (§4.3), except the auth routes.
- Every response carries `traceparent` (W3C) so a report can be matched to
  its trace; errors also put the 32-hex trace ID in `error.trace_id` (null
  when there is no trace).

### 14.2 Routes

Capability "staff:X" means a session, membership in an installed server
(current facts, §2), capability X, and the 2FA rule. "member" means a signed-in session
acting as the member. Rate classes are in §14.7. Pages are dashboard
routes (§14.11).

| # | Method | Path | Auth | Request | Response | Idem. | Rate | Page |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | GET | `/api/auth/login` | public | query `redirect_to` | 302 to Discord | — | sign_in | `/login` |
| 2 | GET | `/api/auth/callback` | public | query `code`, `state` or `error` | 302 to `redirect_to` or `/login?error=…` | — | sign_in | (Discord redirect) |
| 3 | GET | `/api/auth/me` | session | — | `Me` | — | read | every signed-in page |
| 4 | POST | `/api/auth/logout` | session + CSRF | — | 204 | naturally | write | sidebar |
| 5 | GET | `/api/guilds` | session | — | `{guilds: GuildSummary[]}` | — | read | `/guilds` |
| 6 | GET | `/api/guilds/{guild_id}/me` | session + member of server | — | `GuildMe` (with exact pending-appeal and failed-action counts for staff) | — | read | staff layout |
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
| 21 | POST | `/api/guilds/{guild_id}/cases` | staff:moderate | `{member_id, rule_id, rule_version?: number, context: {key: value}, evidence_links: string[], replaces_case_number?: number}` | 201 `Case` | yes | case_create | case dialog |
| 22 | GET | `/api/guilds/{guild_id}/cases/{case_ref}` | staff:moderate | — | `Case` | — | read | cases/$caseRef |
| 23 | POST | `/api/guilds/{guild_id}/cases/{case_ref}/void` | staff:moderate | `{reason}` | `Case` | yes | write | cases/$caseRef |
| 24 | PUT | `/api/guilds/{guild_id}/cases/{case_ref}/context` | staff:moderate | `{context: {key: value}, expected_context_revision: number}` | `Case` | yes | write | cases/$caseRef |
| 25 | POST | `/api/guilds/{guild_id}/cases/{case_ref}/evidence` | staff:moderate | `{links: string[]}` (1–10) | `Case` | yes | write | cases/$caseRef |
| 26 | GET | `/api/guilds/{guild_id}/cases/{case_ref}/evidence/{file_id}` | staff:moderate | — | 302 to a signed Discord URL | — | read | cases/$caseRef |
| 27 | GET | `/api/guilds/{guild_id}/members/{member_id}/summary` | staff:moderate | — | `MemberSummary` | — | read | members/$memberId |
| 28 | GET | `/api/guilds/{guild_id}/actions` | staff:moderate | query `state` (default `failed`), `dismissed` (bool, default false), `limit`, `cursor` | `{actions: ActionListItem[], next_cursor}` | — | read | failures, overview |
| 29 | POST | `/api/guilds/{guild_id}/actions/{action_id}/retry` | staff:moderate | — | `Action` | yes | action_control | failures, cases/$caseRef |
| 30 | POST | `/api/guilds/{guild_id}/actions/{action_id}/dismiss` | staff:moderate | — | `Action` | yes | action_control | failures, cases/$caseRef |
| 31 | POST | `/api/guilds/{guild_id}/actions/{action_id}/undo` | staff:moderate | — | 201 `Action` (the undo) | yes | action_control | cases/$caseRef |
| 32 | GET | `/api/guilds/{guild_id}/appeals` | staff:moderate | query `state`, `limit`, `cursor` | `{appeals: AppealSummary[], next_cursor}` | — | read | appeals |
| 33 | GET | `/api/guilds/{guild_id}/appeals/{appeal_id}` | staff:moderate | — | `Appeal` | — | read | appeals/$appealId |
| 34 | POST | `/api/guilds/{guild_id}/appeals/{appeal_id}/decision` | staff:moderate | `{decision: "request_information" \| "accept" \| "reject" \| "close" \| "reopen", reason, expected_version}` | `Appeal` | yes | write | appeals/$appealId |
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
| 52 | GET | `/api/me/cases/{case_id}` | member | — | `MemberCase` | — | read | `/guilds/$guildId/cases/$caseId/appeal` |
| 53 | GET | `/api/me/cases/{case_id}/evidence/{file_id}` | member | — | 302 | — | read | member case view |
| 54 | POST | `/api/me/cases/{case_id}/appeal` | member | `{statement}` | 201 `MemberCase` | yes | member_write | member case view |
| 55 | POST | `/api/me/cases/{case_id}/appeal/information` | member | `{text, expected_version}` | `MemberCase` | yes | member_write | member case view |
| 56 | GET | `/livez` | public | — | `{"status":"ok"}` | — | none | (probes) |
| 57 | GET | `/readyz` | public | — | `Readiness` (200 or 503) | — | none | (probes) |
| 58 | GET | `/api/guilds/{guild_id}/cases/preview` | staff:moderate | query `member_id`, `rule_id` | `CasePreview` | — | read | case dialog |
| 59 | GET | `/api/guilds/{guild_id}/rules/{rule_id}/versions/{version}` | staff:moderate or manage_rules | — | `RuleVersion` | — | read | rules/$ruleId (history) |

`{module}` is `tickets`, `activity-log`, or `honeypot` in URLs; the same
modules are `tickets`, `activity_log`, `honeypot` in JSON (one fixed
conversion). Member routes (51–55) find the case by its ID and its member
in one query; any other case is `404 not_found`. Ticket routes 48–50 give
the ticket's member the member projection (no staff IDs) and staff the full
one.

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
// `capabilities` are effective (after the 2FA rule); `mfa_blocked` is true when
// the account would have a staff capability but lacks a recorded 2FA confirmation.
type GuildSummary = { guild_id: string; name: string; icon_url: string | null; installed: boolean;
                      capabilities: Capabilities; mfa_required: boolean; mfa_blocked: boolean; can_install: boolean }
type GuildMe = { guild: { guild_id: string; name: string; icon_url: string | null; owner_id: string },
                 capabilities: Capabilities; mfa_required: boolean; mfa_blocked: boolean; is_owner: boolean;
                 counts: { pending_appeals: number; failed_actions: number } | null }
type Permission = "view_channel" | "send_messages" | "send_messages_in_threads" | "embed_links" | "attach_files"
  | "read_message_history" | "moderate_members" | "kick_members" | "ban_members" | "manage_messages"
  | "manage_channels" | "manage_threads" | "create_private_threads" | "view_audit_log"
type ChannelProblem = { code: "not_text" | "everyone_can_view" | "role_can_view" | "member_can_view" | "quack_missing"
                        | "inaccessible" | "missing"; message: string }
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
type RuleVersion = { rule_id: string; version: number; created_at: string; created_by: string | null;
                     reconstructed: boolean; definition: RuleDefinition }
type RuleFile = { format: "quack-rule"; format_version: 1; exported_at: string; rule: RuleDefinition }
type CaseSummary = { id: string; number: number; member_id: string; rule_id: string | null; rule_name: string;
                     rule_version: number | null; level_index: number | null; source: CaseSource; moderator_id: string | null;
                     created_at: string; voided_at: string | null;
                     action: { kind: ActionKind; state: ActionState; outcome_text: string } | null;
                     dm_state: DmState | "not_needed"; appeal_state: AppealState | null }
type CasePreview = { rule_version: number; counted_cases: number; case_count: number; level_index: number;
                     level: Level; counting_window_days: number | null; at: string }
// context values: strings (text, message links, canonical decimal numbers) or booleans
type Case = CaseSummary & { case_count: number | null; level_from_case_count: number | null;
  reason: string; appealable: boolean; fields: Field[]; context: Record<string, string | boolean>; context_revision: number;
  void: { by: string | null; reason: string; at: string; cause: "staff" | "appeal_accepted" } | null;
  replaces_case_number: number | null; replaced_by_case_number: number | null;
  actions: Action[]; dm: Dm | null; evidence: Evidence[]; appeal: AppealSummary | null;
  imported: { v4_action: string; v4_moderator: string | null; v4_reason: string } | null;
  timeline: ModLogEntry[]; member_timeline: { at: string; text: string }[]; timeline_truncated: boolean }
// purpose is derived from undo_of_action_id
type Action = { id: string; case_id: string; case_number: number; kind: ActionKind; purpose: "original" | "undo";
  undo_of_action_id: string | null; state: ActionState; outcome_text: string; timeout_ends_at: string | null;
  delete_message_seconds: number | null; attempts: number; next_attempt_at: string | null; failure_reason: ActionFailure | null;
  http_status: number | null; discord_code: number | null; skip_reason: "other_case_applies" | "changed_outside_case" | null;
  skipped_because_case_number: number | null;
  requested_by: string | null; dismissed_at: string | null; dismissed_by: string | null; done_at: string | null;
  created_at: string; attempt_log: { number: number; started_at: string; finished_at: string | null;
  outcome: AttemptOutcome | null; http_status: number | null; discord_code: number | null }[];
  can_retry: boolean; can_dismiss: boolean; can_undo: boolean }
type ActionListItem = Action & { member_id: string; rule_name: string }
// dm is null when the level sends no DM ("not needed")
type Dm = { state: DmState; failure: DmFailure | null; outcome_text: string; settled_at: string | null }
type Evidence = { id: string; state: EvidenceState; failure: EvidenceFailure | null; state_text: string;
  channel_id: string; message_id: string; url: string; author_id: string | null; content: string | null;
  message_created_at: string | null; message_edited_at: string | null;
  embeds: { type: string; title: string | null; description: string | null; url: string | null }[];
  files: { id: string; filename: string; content_type: string | null; size_bytes: number; copy_state: FileCopyState; state_text: string }[];
  files_not_saved: number; added_by: string | null; created_at: string }
type AppealSummary = { id: string; case_id: string; case_number: number; member_id: string; rule_name: string;
                       state: AppealState; version: number; created_at: string; updated_at: string }
type Appeal = AppealSummary & { statement: string; decided_by: string | null; decided_at: string | null;
  decision_reason: string | null; events: { id: string; kind: AppealEventKind; actor_id: string | null; text: string;
  closed_by_void: boolean; created_at: string }[];
  case: CaseSummary }
type MemberCaseSummary = { id: string; number: number; guild: { guild_id: string; name: string; icon_url: string | null };
  rule_name: string; reason: string; created_at: string; voided_at: string | null; action_text: string;
  can_appeal: boolean; appeal_state: AppealState | null }
type MemberCase = MemberCaseSummary & { void_reason: string | null; undo_text: string | null; timeout_ends_at: string | null;
  dm_text: string;
  // message_link values are replaced by the text of member.context.link_saved
  context: { label: string; kind: FieldKind; value: string | boolean }[];
  evidence: { id: string; content: string | null; embeds: { type: string; title: string | null }[];
              files: { id: string; filename: string; content_type: string | null; size_bytes: number; available: boolean }[] }[];
  // only when both cases are about this member (§4.8)
  replaces_case_number: number | null; replaced_by_case_number: number | null;
  appeal: { id: string; state: AppealState; version: number; statement: string; created_at: string; can_reply: boolean;
            events: { kind: AppealEventKind; text: string; at: string; by_you: boolean }[] } | null;
  timeline: { at: string; text: string }[]; timeline_truncated: boolean }
type MemberSummary = { member_id: string; total: number; valid: number; voided: number;
                       by_rule: { rule_id: string | null; rule_name: string; valid: number; voided: number }[] }
type ModLogEntry = { id: string; kind: ModLogKind; result: ModLogResult; actor_id: string | null; source: ModLogSource;
  case_id: string | null; case_number: number | null; member_id: string | null; appeal_id: string | null;
  action_id: string | null; rule_id: string | null; ticket_id: string | null; detail: object; text: string;
  links: { case?: string; appeal?: string; member?: string; rule?: string; ticket?: string }; created_at: string }  // dashboard paths derived from the typed IDs
type Settings = { version: number; moderator_role_ids: string[]; rules_manager_role_ids: string[];
  log_channel_id: string | null; appeal_channel_id: string | null; evidence_channel_id: string | null;
  appeal_invite_url: string | null; dm_intro: string | null; dm_footer: string | null;
  starter_rule_id: string | null; starter_notice_acknowledged_at: string | null;
  modules: Record<"tickets" | "activity_log" | "honeypot", ModuleState> }
type SettingsPatch = Partial<Omit<Settings, "version" | "starter_rule_id" | "starter_notice_acknowledged_at" | "modules">>
                     & { expected_version?: number }
type ModuleState = { enabled: boolean; disabled_reason: "channel_deleted" | "quack_left" | null; problems: ModuleProblem[] }
type ModuleProblem = { code: string; message: string }
type TicketSettings = { entry_channel_id: string | null; queue_channel_id: string | null; transcript_retention_days: number }
type TicketsModule = ModuleState & { settings: TicketSettings; open_tickets: number }
type ActivityLogSettings = { routes: Partial<Record<ActivityEvent, string>>; include_message_content: boolean;
                             include_file_names: boolean; include_embed_types: boolean }
// counters are in-process and reset when Quack restarts
type ActivityLogModule = ModuleState & { settings: ActivityLogSettings; posted_since_start: number; dropped_since_start: number }
type HoneypotSettings = { channel_id: string | null; rule_id: string | null; warning_text: string | null }
type HoneypotModule = ModuleState & { settings: HoneypotSettings; eligible_rules: { id: string; name: string }[];
                                      cases_created: number; pending_hits: number; failed_hits: number }
// The ticket's member gets this without closed_by (closed_by_you instead).
type Ticket = { id: string; member_id: string; state: TicketState; thread_id: string | null; created_at: string;
  opened_at: string | null; close_requested_at: string | null; closed_at: string | null; closed_by: string | null;
  closed_by_you?: boolean; close_reason: "open_failed" | null; thread_delete_failed: boolean;
  transcript: { state: "available" | "expired"; complete: boolean; partial_cause: "history" | "trimmed" | null;
                expires_at: string } | null }
type Readiness = { ready: boolean; checks: Record<"database" | "migrations" | "fence" | "gateway" | "workers" | "shutdown", "ok" | "failing"> }
```

`activity_log` module counts come from in-process counters since start and
are named `_since_start` for that reason.

**Case preview** (route 58): the counted cases, case count, and level the
rule's current version would choose for that member now (§3.4), so the case
dialog can show what will happen; the POST sends that `rule_version` back,
and a rule edited in between is `409 rule_changed`.

**Statistics** (route 8): `to` defaults to now and `from` to one calendar
month before `to`; the range must be 1 second to 366 days (else
`validation_failed`). Rows are counted by `created_at` in `[from, to)`
using their current state, grouped by UTC day (`cases_by_day`), by rule
(`cases_by_rule`, imported cases under "Imported"), by valid/voided, by
source, by action kind and state (original and undo actions, each by
its own `created_at`), by appeal
state, and by moderation log kind. Buckets without rows are omitted. One
query per group, all in one `REPEATABLE READ` read-only transaction. No
per-moderator ranking.

### 14.4 Authentication

Discord OAuth 2.0 authorization code flow with PKCE (S256), scopes
`identify guilds`, redirect URI `<QUACK_PUBLIC_URL>/api/auth/callback`
(register it in the Discord developer portal). The `oauth2` crate builds the
URLs and does the exchange through a small reqwest adapter in `quack-api`,
traced like every outbound call (§18.4).

Cookie names: on `https` public URLs `__Host-quack_oauth` and
`__Host-quack_session` (`Secure; Path=/`, no `Domain`); on `http`, which is
allowed only for a loopback public URL in development, `quack_oauth` and
`quack_session` without `Secure`. Both are `HttpOnly; SameSite=Lax`.

1. **Login** (route 1). `redirect_to` (at most 2,048 characters) is parsed
   once as a URL relative to `QUACK_PUBLIC_URL`; its resolved origin must
   equal the public URL's origin, and only its path, query, and fragment are
   kept; anything else becomes `/guilds`. Generate `state` (32 random
   bytes), a PKCE verifier (64 characters), and a browser nonce (32 random
   bytes). Insert `oauth_states (state_hash = SHA-256(state), code_verifier,
   redirect_to, nonce_hash, expires_at = now + 10 min)`. Set the OAuth cookie
   to the nonce (`Max-Age=600`). 302 to
   `https://discord.com/oauth2/authorize?…&state=…&code_challenge=…&code_challenge_method=S256`.
2. **Callback** (route 2).
   `DELETE FROM oauth_states WHERE state_hash = $1 AND nonce_hash = $2 AND expires_at > now() RETURNING *`
   with the hash of the cookie's nonce, so a state is consumed only by the
   browser that started it; no row → 302 `/login?error=failed`. Then:
   `error` parameter → 302 `/login?error=denied`. Exchange the code (10 s
   deadline, response body ≤ 1 MiB, no redirects), `GET /users/@me` with the
   access token, then one transaction: upsert `discord_accounts
   (mfa_enabled, mfa_checked_at = now())`, create the session, and delete
   the browser's previous session row if it sent one. Clear the OAuth
   cookie, set the session cookie, 302 to `redirect_to`. Any failure → 302
   `/login?error=failed`. Metric `quack.auth.sign_ins{quack.sign_in.outcome}`
   (`done`, `cancelled` for the `error` parameter, `refused` for a missing
   or mismatched state, `failed`).
3. **Session.** 32 random bytes, sent base64url in the cookie; the table
   stores only `SHA-256(id)`. Row: account, CSRF token (32 random bytes),
   the Discord access token encrypted with AES-256-GCM under
   `QUACK_SESSION_SECRET` (a fresh random nonce per encryption, the session
   ID hash and account ID as associated data; needed only for route 5),
   `key_fingerprint` (the first 8 bytes of SHA-256 of the secret),
   `created_at`, `last_seen_at`, `expires_at = now + min(Discord token
   lifetime, 7 days)`. There is no refresh: when the session ends the member
   signs in again. Each request: `SELECT … WHERE id_hash = $1 AND expires_at
   > now() AND key_fingerprint = $2`; touch `last_seen_at` at most every 5
   minutes. Rotating the secret therefore signs everyone out; housekeeping
   deletes the old rows.
4. **Cookie.** The session cookie has `Max-Age=<seconds until
   expires_at>`.
5. **Logout** (route 4, origin and CSRF checked like every mutation) deletes
   the row and clears the cookie.

Server membership for staff routes is checked from current facts (§2),
never from the session. Route 5 combines the account's Discord server list
(`GET /users/@me/guilds` with the session's token, cached 60 s), the
servers where Quack is installed, and capabilities computed per installed
server (`can_install` = not installed and the account has Manage Server
there).

### 14.5 CSRF and origin

For every POST, PUT, PATCH, and DELETE under `/api`, logout included:

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
    "message": "Check the marked fields and save again.",
    "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736",
    "fields": [ { "field": "levels[1].from_case_count", "code": "level_order", "message": "…" } ],
    "problems": [ { "code": "ticket_queue_missing", "message": "Choose a staff channel for new tickets." } ],
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
| 404 | `action_not_found` | `error.action_not_found` | |
| 404 | `server_not_found` | `api.server_not_found` | |
| 404 | `file_unavailable` | `api.file_unavailable` | |
| 409 | `idempotency_key_reused` | `api.idempotency_key_reused` | |
| 409 | `rule_changed` | `api.rule_changed`, or `error.rule_changed` for case creation | |
| 409 | `context_changed` | `error.context_changed` | |
| 409 | `rule_name_taken` | `api.rule_name_taken` | |
| 409 | `rule_used_by_honeypot` | `api.rule_used_by_honeypot` | |
| 409 | `required_field_on_honeypot_rule` | `api.required_field_on_honeypot_rule` | |
| 409 | `too_many_rules` | `api.too_many_rules` | |
| 409 | `settings_changed` | `api.settings_changed` | |
| 409 | `case_already_voided` | `error.case_already_voided` | |
| 409 | `case_voided` | `error.case_voided` or `appeal.submit.voided` | |
| 409 | `action_not_retryable` | `error.action_not_retryable` | |
| 409 | `action_not_dismissable` | `error.action_not_dismissable` | |
| 409 | `action_not_undoable` | `error.action_not_undoable` | |
| 409 | `appeal_changed` | `error.appeal_changed`, or `appeal.information.outdated` on member routes | |
| 409 | `appeal_exists` | `api.appeal_exists` | |
| 409 | `ticket_not_open` | `ticket.close.not_open` | |
| 409 | `module_not_ready` | `api.module_not_ready` | `problems` |
| 409 | `replacement_invalid` | `error.replacement_invalid` | |
| 409 | `case_numbers_exhausted` | `error.case_numbers_exhausted` | |
| 413 | `body_too_large` | `api.body_too_large` | |
| 415 | `unsupported_media_type` | `api.unsupported_media_type` | |
| 422 | `rule_archived` | `error.rule_archived` | |
| 422 | `member_refused` | the matching `refusal.*` | `fields: [{field: "member_id", code: <refusal>}]` |
| 422 | `appeal_not_allowed` | `api.appeal_not_allowed` | |
| 422 | `timeout_window_passed` | `error.timeout_window_passed` | |
| 422 | `evidence_limit_reached` | `error.evidence_limit_reached` (also for more than 50 evidence sources at creation) | |
| 422 | `too_many_links` | `error.too_many_links` | |
| 428 | `idempotency_key_required` | `api.idempotency_key_required` | |
| 429 | `rate_limited` | `api.rate_limited` | `retry_after_seconds`, header `Retry-After` |
| 502 | `discord_unavailable` | `api.discord_unavailable` | |
| 503 | `unavailable` | `api.unavailable` | |
| 500 | `internal_error` | `api.internal_error` | |

**Where codes come from.** `quack_core::DomainError` is the refusal a
pure domain check can make; `DomainError::code()` and
`DomainError::http_status()` give its row above (`CaseNotFound`,
`RuleNotFound`, and `AppealNotFound` all map to `not_found`). A domain error
carries no case number, rule name, action kind, or member, so the message
comes from `quack_text::error::for_domain(&err, &ErrorFacts)`: the caller
passes what it knows (`scene` — staff, adding a case, editing a rule, or a
member's own view — `case_number`, `rule`, `action`, `member`),
and a message whose facts are missing falls back to a sentence that says
"this case", "that rule", or "this action" (§12.3). Refusals that only the
engine can detect are variants of the engine's own error type, not of
`DomainError`, each with its own `quack-text` function:
`ActionNotFound` (`action_not_found`), `ContextChanged`
(`context_changed`), `CaseNumbersExhausted` (`case_numbers_exhausted`), and
`HoneypotPolicyChanged` (no API code: the honeypot flow records it on the
hit, §13.4.2, and only the log text `error.honeypot_policy_changed` is
shown). The engine error wraps `DomainError`, member refusals,
`ContextProblem` lists, and infrastructure failures (`discord_unavailable`,
`unavailable`, `internal_error`).

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
the n-th address from the right of `X-Forwarded-For`. Set it only when every
connection reaches Quack through those proxies and they overwrite the
header; otherwise a client can choose its own address. Metric
`quack.http.rate_limited{quack.rate_limit.class}`.

### 14.8 Limits, timeouts, headers

- Request body ≤ 262,144 bytes (`413 body_too_large`).
- Request deadline 10 s (`503 unavailable`) for everything before a write
  transaction begins; once it has begun, the request finishes (statement and
  lock timeouts bound it), so "nothing changed" is never said about a write
  that committed. Header read 5 s; idle keep-alive 60 s. Database statement
  timeout 5 s (2 s inside case creation, §4.2).
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
  equals the binary's set, verified at startup), `fence` (this process
  holds the singleton lock, §17.5), `gateway` (the shard's current session
  is `READY` or `RESUMED`), `workers` (every job dispatcher completed a
  pass within the last 15 s, `Runner::heartbeats_fresh`), `shutdown` (not
  shutting down). A pass never waits for a handler: when its kind or the
  global limit has no free permit it ends at once, so a dispatcher whose
  permits are all busy still heartbeats at least every 2 s. A pass that
  fails (the database is unreachable) records no heartbeat, and a
  dispatcher that never completed a pass is not fresh.

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
client accepts it. Dashboard CSP (the Go server's, unchanged; the
visitors.now analytics script loads from `cdn.visitors.now` and reports to
`e.visitors.now`): `default-src 'self'; script-src 'self'
https://cdn.visitors.now; style-src 'self' 'unsafe-inline'; img-src 'self'
data: https://cdn.discordapp.com https://media.discordapp.net; font-src
'self'; connect-src 'self' https://e.visitors.now; frame-ancestors 'none';
base-uri 'self'; form-action 'self'`. At startup `serve` checks that
`QUACK_DASHBOARD_DIR` contains `index.html` and refuses to start
otherwise.

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
Staff deep links from the Go era (`/guilds/{id}/appeals/{ulid}`,
`/rules/{ulid}`) resolve the same way, after the staff check.

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
  `RoleId`), strings in JSON. One checked parser converts them; a value that
  does not fit is rejected, never wrapped.
- `timestamptz` everywhere; the application writes UTC.
- Every tenant table has `guild_id`. A child that must belong to the same
  server as its parent uses a composite foreign key `(guild_id, parent_id)`
  against `UNIQUE (guild_id, id)`; a child that must be about the same
  member as its case (actions, appeals, DMs, honeypot hits) uses `(guild_id,
  case_id, member_id)`; evidence files and undo actions must also match
  their parent's case.
- States are Postgres enums; invariants are `CHECK` constraints. Text limits
  are enforced by the application on every write (§3.9); a CHECK repeats a
  limit only where imported data cannot exceed it, and otherwise only
  requires non-empty text.
- **One migration file per change, in dependency order.** The schema below
  is `crates/quack-db/migrations/0001_initial.sql`: the SQL blocks of
  §15.1–§15.7 concatenated in this order, including the triggers. A test
  applies that exact file to an empty PostgreSQL 18 database in one
  transaction. Later changes are new files (`0002_…`), listed with their
  SQL in §15.10. `quack migrate` (sqlx
  migrator, ledger `_sqlx_migrations`) applies them; `quack serve` only
  verifies the applied set equals the binary's set, unless started with
  `--migrate`. The triggers reject changes even for the table owner; for
  defense in depth `serve` may run as a role that owns nothing and has only
  `INSERT` and `SELECT` on `mod_log` and `rule_versions` (§20.3).
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
  last_number bigint NOT NULL DEFAULT 0 CHECK (last_number BETWEEN 0 AND 9007199254740991)  -- 2^53 - 1
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
  account_id               bigint NOT NULL REFERENCES discord_accounts (account_id),
  csrf_token               text NOT NULL CHECK (char_length(csrf_token) = 43),
  discord_token_ciphertext bytea NOT NULL,                          -- AES-256-GCM(nonce || ciphertext || tag)
  key_fingerprint          bytea NOT NULL CHECK (octet_length(key_fingerprint) = 8),  -- which session secret
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
  name            text NOT NULL CHECK (char_length(name) >= 1),     -- current version's name; 1-100 on write (§3.3)
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

-- Rejects every UPDATE, DELETE, and TRUNCATE on permanent history.
CREATE FUNCTION reject_change() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  RAISE EXCEPTION '% is append-only', TG_TABLE_NAME USING ERRCODE = 'insufficient_privilege';
END $$;
CREATE TRIGGER rule_versions_no_change BEFORE UPDATE OR DELETE ON rule_versions
  FOR EACH ROW EXECUTE FUNCTION reject_change();
CREATE TRIGGER rule_versions_no_truncate BEFORE TRUNCATE ON rule_versions
  FOR EACH STATEMENT EXECUTE FUNCTION reject_change();

-- Core server settings.
CREATE TABLE guild_settings (
  guild_id                       bigint PRIMARY KEY REFERENCES guilds (guild_id),
  version                        bigint NOT NULL DEFAULT 1 CHECK (version >= 1),  -- optimistic check for staff role edits
  moderator_role_ids             bigint[] NOT NULL DEFAULT '{}'
                                 CHECK (cardinality(moderator_role_ids) <= 25 AND array_position(moderator_role_ids, NULL) IS NULL),
  rules_manager_role_ids         bigint[] NOT NULL DEFAULT '{}'
                                 CHECK (cardinality(rules_manager_role_ids) <= 25 AND array_position(rules_manager_role_ids, NULL) IS NULL),
  log_channel_id                 bigint NULL CHECK (log_channel_id > 0),
  appeal_channel_id              bigint NULL CHECK (appeal_channel_id > 0),
  evidence_channel_id            bigint NULL CHECK (evidence_channel_id > 0),
  appeal_invite_url              text NULL CHECK (appeal_invite_url ~ '^https://discord\.gg/[A-Za-z0-9_-]{2,64}$'),
  dm_intro                       text NULL CHECK (char_length(dm_intro) >= 1),   -- 1-1,000 on write
  dm_footer                      text NULL CHECK (char_length(dm_footer) >= 1),  -- 1-500 on write
  starter_rule_id                uuid NULL,
  starter_notice_acknowledged_at timestamptz NULL,
  updated_at                     timestamptz NOT NULL DEFAULT now(),
  updated_by                     bigint NULL,
  FOREIGN KEY (guild_id, starter_rule_id) REFERENCES rules (guild_id, id)
);

-- Module settings (one row each per server, created by bootstrap).
CREATE TABLE ticket_settings (
  guild_id                  bigint PRIMARY KEY REFERENCES guilds (guild_id),
  enabled                   boolean NOT NULL DEFAULT false,
  disabled_reason           module_disabled_reason NULL,
  entry_channel_id          bigint NULL CHECK (entry_channel_id > 0),
  queue_channel_id          bigint NULL CHECK (queue_channel_id > 0),
  transcript_retention_days smallint NOT NULL DEFAULT 90 CHECK (transcript_retention_days BETWEEN 1 AND 365),
  version                   bigint NOT NULL DEFAULT 1 CHECK (version >= 1),
  updated_at                timestamptz NOT NULL DEFAULT now(),
  updated_by                bigint NULL,
  CHECK (NOT enabled OR (entry_channel_id IS NOT NULL AND queue_channel_id IS NOT NULL
                         AND entry_channel_id <> queue_channel_id)),
  CHECK (NOT (enabled AND disabled_reason IS NOT NULL))
);

CREATE TABLE activity_log_settings (
  guild_id                bigint PRIMARY KEY REFERENCES guilds (guild_id),
  enabled                 boolean NOT NULL DEFAULT false,
  disabled_reason         module_disabled_reason NULL,
  include_message_content boolean NOT NULL DEFAULT true,
  include_file_names      boolean NOT NULL DEFAULT true,
  include_embed_types     boolean NOT NULL DEFAULT true,
  version                 bigint NOT NULL DEFAULT 1 CHECK (version >= 1),
  updated_at              timestamptz NOT NULL DEFAULT now(),
  updated_by              bigint NULL,
  CHECK (NOT (enabled AND disabled_reason IS NOT NULL))
);

CREATE TABLE activity_log_routes (
  guild_id   bigint NOT NULL REFERENCES activity_log_settings (guild_id),
  event      activity_event NOT NULL,
  channel_id bigint NOT NULL CHECK (channel_id > 0),
  PRIMARY KEY (guild_id, event)
);
CREATE INDEX activity_log_routes_channel ON activity_log_routes (guild_id, channel_id);

CREATE TABLE honeypot_settings (
  guild_id        bigint PRIMARY KEY REFERENCES guilds (guild_id),
  enabled         boolean NOT NULL DEFAULT false,
  disabled_reason module_disabled_reason NULL,
  channel_id      bigint NULL CHECK (channel_id > 0),
  rule_id         uuid NULL,
  warning_text    text NULL CHECK (char_length(warning_text) >= 1),  -- 1-1,500 on write
  version         bigint NOT NULL DEFAULT 1 CHECK (version >= 1),    -- stamped on every hit (§13.4.2)
  updated_at      timestamptz NOT NULL DEFAULT now(),
  updated_by      bigint NULL,
  FOREIGN KEY (guild_id, rule_id) REFERENCES rules (guild_id, id),
  CHECK (NOT enabled OR (channel_id IS NOT NULL AND rule_id IS NOT NULL)),
  CHECK (NOT (enabled AND disabled_reason IS NOT NULL))
);
```

### 15.4 Cases, actions, evidence, appeals

```sql
-- One row per case. Never deleted.
CREATE TABLE cases (
  id                         uuid PRIMARY KEY DEFAULT uuidv7(),
  guild_id                   bigint NOT NULL REFERENCES guilds (guild_id),
  number                     bigint NOT NULL CHECK (number BETWEEN 1 AND 9007199254740991),
  member_id                  bigint NOT NULL CHECK (member_id > 0),
  member_joined_at           timestamptz NULL,                    -- the membership the case is about; NULL when imported
  source                     case_source NOT NULL,
  moderator_id               bigint NULL CHECK (moderator_id > 0), -- NULL for honeypot and some imported cases
  rule_id                    uuid NULL,
  rule_version               integer NULL,
  case_count                 integer NULL CHECK (case_count >= 1),
  level_index                smallint NULL CHECK (level_index >= 1),
  level_from_case_count      integer NULL CHECK (level_from_case_count >= 1),
  action_kind                action_kind NULL CHECK (action_kind IN ('timeout', 'kick', 'ban')),
  timeout_seconds            integer NULL CHECK (timeout_seconds BETWEEN 1 AND 2419200),
  ban_delete_message_seconds integer NULL CHECK (ban_delete_message_seconds BETWEEN 0 AND 604800),
  sends_dm                   boolean NOT NULL,
  context                    jsonb NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(context) = 'object'),  -- field key -> value
  context_revision           integer NOT NULL DEFAULT 1 CHECK (context_revision >= 1),
  operation_key              text NULL CHECK (char_length(operation_key) BETWEEN 1 AND 400),  -- <account>:<operation>:<key>
  request_hash               bytea NULL CHECK (octet_length(request_hash) = 32),
  replaces_case_id           uuid NULL,
  voided_at                  timestamptz NULL,
  voided_by                  bigint NULL,                         -- NULL = Quack or unknown (imported)
  void_reason                text NULL CHECK (char_length(void_reason) >= 1),  -- 1-1,000 on write
  void_cause                 void_cause NULL,
  imported                   jsonb NULL CHECK (jsonb_typeof(imported) = 'object'),  -- {v4_action, v4_moderator, v4_reason, v4_source_id}
  created_at                 timestamptz NOT NULL DEFAULT now(),
  updated_at                 timestamptz NOT NULL DEFAULT now(),
  UNIQUE (guild_id, number),
  UNIQUE (guild_id, id),
  UNIQUE (guild_id, id, member_id),
  UNIQUE (guild_id, operation_key),
  FOREIGN KEY (guild_id, rule_id) REFERENCES rules (guild_id, id),
  FOREIGN KEY (rule_id, rule_version) REFERENCES rule_versions (rule_id, version),
  FOREIGN KEY (guild_id, replaces_case_id) REFERENCES cases (guild_id, id),
  CHECK (replaces_case_id IS DISTINCT FROM id),
  CHECK ((rule_id IS NULL) = (rule_version IS NULL)),
  CHECK (request_hash IS NULL OR operation_key IS NOT NULL),
  CHECK ((voided_at IS NULL) = (void_reason IS NULL) AND (voided_at IS NULL) = (void_cause IS NULL)),
  CHECK ((source = 'imported') = (imported IS NOT NULL)),
  CHECK (source = 'imported' OR (rule_id IS NOT NULL AND case_count IS NOT NULL
                                 AND level_index IS NOT NULL AND level_from_case_count IS NOT NULL)),
  CHECK (source IN ('honeypot', 'imported') OR moderator_id IS NOT NULL),
  CHECK ((action_kind IS NOT DISTINCT FROM 'timeout') = (timeout_seconds IS NOT NULL)),
  CHECK ((action_kind IS NOT DISTINCT FROM 'ban') = (ban_delete_message_seconds IS NOT NULL)),
  CHECK (source <> 'imported' OR (action_kind IS NULL AND NOT sends_dm))
);
CREATE INDEX cases_escalation ON cases (guild_id, member_id, rule_id, created_at)
  WHERE voided_at IS NULL AND source <> 'imported';
CREATE UNIQUE INDEX cases_replaces ON cases (replaces_case_id) WHERE replaces_case_id IS NOT NULL;
CREATE INDEX cases_member     ON cases (guild_id, member_id, number DESC);
CREATE INDEX cases_rule       ON cases (guild_id, rule_id, number DESC);
CREATE INDEX cases_moderator  ON cases (guild_id, moderator_id, number DESC) WHERE moderator_id IS NOT NULL;
CREATE INDEX cases_created    ON cases (guild_id, created_at);                -- statistics
CREATE INDEX cases_honeypot   ON cases (guild_id) WHERE source = 'honeypot';  -- the warning's count
CREATE INDEX cases_member_all ON cases (member_id, created_at DESC, id DESC);  -- a member's own cases

-- Original actions (timeout, kick, ban) and undo actions (remove_timeout, unban).
CREATE TABLE actions (
  id                      uuid PRIMARY KEY DEFAULT uuidv7(),
  guild_id                bigint NOT NULL,
  case_id                 uuid NOT NULL,
  member_id               bigint NOT NULL,                        -- the case's member (composite FK), for the undo guard
  kind                    action_kind NOT NULL,
  undo_of_action_id       uuid NULL,                              -- set exactly for undo actions; same case
  state                   action_state NOT NULL DEFAULT 'pending',
  timeout_ends_at         timestamptz NULL,                       -- fixed at case creation, whole seconds (§5.2)
  delete_message_seconds  integer NULL CHECK (delete_message_seconds BETWEEN 0 AND 604800),
  requested_by            bigint NULL,                            -- staff who requested an undo or retry; NULL = Quack
  attempts                integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),  -- in the current round
  next_attempt_at         timestamptz NULL,
  last_attempt_at         timestamptz NULL,
  failure_reason          action_failure NULL,
  failure_permission      discord_permission NULL,                -- for quack_missing_permission
  failure_http_status     smallint NULL,
  failure_discord_code    integer NULL,
  skip_reason             action_skip_reason NULL,
  skipped_because_case_id uuid NULL,
  no_change               boolean NOT NULL DEFAULT false,         -- done because the desired state already held
  done_at                 timestamptz NULL,
  dismissed_at            timestamptz NULL,
  dismissed_by            bigint NULL,
  created_at              timestamptz NOT NULL DEFAULT now(),
  updated_at              timestamptz NOT NULL DEFAULT now(),
  UNIQUE (guild_id, id),
  UNIQUE (guild_id, case_id, id),
  UNIQUE (undo_of_action_id),
  FOREIGN KEY (guild_id, case_id, member_id) REFERENCES cases (guild_id, id, member_id),
  FOREIGN KEY (guild_id, case_id, undo_of_action_id) REFERENCES actions (guild_id, case_id, id),
  FOREIGN KEY (guild_id, skipped_because_case_id) REFERENCES cases (guild_id, id),
  CHECK (undo_of_action_id IS DISTINCT FROM id),
  CHECK ((kind IN ('remove_timeout', 'unban')) = (undo_of_action_id IS NOT NULL)),
  CHECK ((kind = 'timeout') = (timeout_ends_at IS NOT NULL)),
  CHECK ((kind = 'ban') = (delete_message_seconds IS NOT NULL)),
  CHECK ((state = 'failed') = (failure_reason IS NOT NULL)),
  CHECK (state = 'failed' OR (failure_permission IS NULL AND failure_http_status IS NULL
                              AND failure_discord_code IS NULL)),
  CHECK ((failure_reason IS NOT DISTINCT FROM 'quack_missing_permission') = (failure_permission IS NOT NULL)),
  CHECK ((state = 'skipped') = (skip_reason IS NOT NULL)),
  CHECK ((skip_reason IS NOT DISTINCT FROM 'other_case_applies') = (skipped_because_case_id IS NOT NULL)),
  CHECK (state <> 'skipped' OR undo_of_action_id IS NOT NULL),
  CHECK (state <> 'cancelled' OR undo_of_action_id IS NULL),
  CHECK ((state = 'done') = (done_at IS NOT NULL)),
  CHECK (NOT no_change OR state = 'done'),
  CHECK (dismissed_at IS NULL OR state = 'failed'),
  CHECK ((dismissed_at IS NULL) = (dismissed_by IS NULL))
);
CREATE UNIQUE INDEX actions_one_original ON actions (case_id) WHERE undo_of_action_id IS NULL;
CREATE INDEX actions_case   ON actions (case_id);
CREATE INDEX actions_member ON actions (guild_id, member_id, kind) WHERE state IN ('running', 'done');
CREATE INDEX actions_review ON actions (guild_id, created_at DESC, id DESC) WHERE state = 'failed' AND dismissed_at IS NULL;
CREATE INDEX actions_guild_state ON actions (guild_id, state, created_at DESC, id DESC);

-- Every attempt, opened before the Discord call.
CREATE TABLE action_attempts (
  guild_id     bigint NOT NULL,
  action_id    uuid NOT NULL,
  number       integer NOT NULL CHECK (number >= 1),               -- across all rounds
  started_at   timestamptz NOT NULL,
  finished_at  timestamptz NULL,
  outcome      attempt_outcome NULL,
  http_status  smallint NULL,
  discord_code integer NULL,
  PRIMARY KEY (action_id, number),
  FOREIGN KEY (guild_id, action_id) REFERENCES actions (guild_id, id),
  CHECK ((finished_at IS NULL) = (outcome IS NULL)),
  CHECK (finished_at IS NULL OR finished_at >= started_at)
);
CREATE UNIQUE INDEX action_attempts_one_open ON action_attempts (action_id) WHERE finished_at IS NULL;

-- Saved messages. One row per (case, message).
CREATE TABLE evidence (
  id                          uuid PRIMARY KEY DEFAULT uuidv7(),
  guild_id                    bigint NOT NULL,
  case_id                     uuid NOT NULL,
  channel_id                  bigint NOT NULL CHECK (channel_id > 0),
  message_id                  bigint NOT NULL CHECK (message_id > 0),
  added_by                    bigint NULL,                        -- NULL = Quack (honeypot, imported)
  state                       evidence_state NOT NULL DEFAULT 'saving',
  failure                     evidence_failure NULL,
  author_id                   bigint NULL,
  content                     text NULL CHECK (char_length(content) <= 4000),
  message_created_at          timestamptz NULL,
  message_edited_at           timestamptz NULL,
  embeds                      jsonb NOT NULL DEFAULT '[]'
                              CHECK (jsonb_typeof(embeds) = 'array' AND jsonb_array_length(embeds) <= 10),
  files_not_saved             integer NOT NULL DEFAULT 0 CHECK (files_not_saved >= 0),
  copy_skipped_not_staff_only boolean NOT NULL DEFAULT false,
  created_at                  timestamptz NOT NULL DEFAULT now(),
  saved_at                    timestamptz NULL,
  UNIQUE (guild_id, id),
  UNIQUE (guild_id, case_id, id),
  UNIQUE (case_id, message_id),
  FOREIGN KEY (guild_id, case_id) REFERENCES cases (guild_id, id),
  CHECK ((state = 'failed') = (failure IS NOT NULL)),
  CHECK ((state = 'saved') = (saved_at IS NOT NULL))
);
CREATE INDEX evidence_case ON evidence (case_id, created_at, id);

-- Files of a saved message (at most 10 per message, 100 per case), and their copies.
CREATE TABLE evidence_files (
  id                 uuid PRIMARY KEY DEFAULT uuidv7(),
  guild_id           bigint NOT NULL,
  case_id            uuid NOT NULL,
  evidence_id        uuid NOT NULL,
  position           smallint NOT NULL CHECK (position BETWEEN 1 AND 10),
  attachment_id      bigint NULL CHECK (attachment_id > 0),       -- the original's Discord attachment ID
  filename           text NOT NULL CHECK (char_length(filename) BETWEEN 1 AND 255),
  content_type       text NULL CHECK (char_length(content_type) <= 191),
  size_bytes         bigint NOT NULL CHECK (size_bytes >= 0),
  original_url       text NOT NULL CHECK (char_length(original_url) <= 2048),
  copy_state         file_copy_state NOT NULL DEFAULT 'pending',
  copy_started_at    timestamptz NULL,                            -- the upload's nonce window (§10.4)
  copy_channel_id    bigint NULL,
  copy_message_id    bigint NULL,
  copy_attachment_id bigint NULL,
  created_at         timestamptz NOT NULL DEFAULT now(),
  UNIQUE (evidence_id, position),
  FOREIGN KEY (guild_id, case_id, evidence_id) REFERENCES evidence (guild_id, case_id, id),
  CHECK (num_nonnulls(copy_channel_id, copy_message_id, copy_attachment_id)
         = CASE WHEN copy_state = 'copied' THEN 3 ELSE 0 END)
);
CREATE INDEX evidence_files_case ON evidence_files (case_id);

-- One appeal per case.
CREATE TABLE appeals (
  id               uuid PRIMARY KEY DEFAULT uuidv7(),
  guild_id         bigint NOT NULL,
  case_id          uuid NOT NULL,
  member_id        bigint NOT NULL,                               -- the case's member (composite FK)
  state            appeal_state NOT NULL DEFAULT 'pending',
  version          integer NOT NULL DEFAULT 1 CHECK (version >= 1),
  statement        text NOT NULL CHECK (char_length(statement) >= 1),  -- 1-4,000 on write
  decided_by       bigint NULL,                                   -- latest staff decision
  decided_at       timestamptz NULL,
  decision_reason  text NULL CHECK (char_length(decision_reason) >= 1),
  first_decided_at timestamptz NULL,                              -- first accept/reject/close, for metrics
  created_at       timestamptz NOT NULL DEFAULT now(),
  updated_at       timestamptz NOT NULL DEFAULT now(),
  UNIQUE (case_id),
  UNIQUE (guild_id, id),
  FOREIGN KEY (guild_id, case_id, member_id) REFERENCES cases (guild_id, id, member_id)
);
CREATE INDEX appeals_guild_state   ON appeals (guild_id, state, created_at, id);
CREATE INDEX appeals_guild_created ON appeals (guild_id, created_at DESC, id DESC);

-- Appeal timeline: statement, replies, decisions.
CREATE TABLE appeal_events (
  id             uuid PRIMARY KEY DEFAULT uuidv7(),
  guild_id       bigint NOT NULL,
  appeal_id      uuid NOT NULL,
  kind           appeal_event_kind NOT NULL,
  actor_id       bigint NULL,                                     -- member or staff; NULL = Quack
  text           text NOT NULL CHECK (char_length(text) >= 1),    -- 1-4,000 (replies) or 1-2,000 (reasons) on write
  closed_by_void boolean NOT NULL DEFAULT false,
  created_at     timestamptz NOT NULL DEFAULT now(),
  UNIQUE (guild_id, id),
  FOREIGN KEY (guild_id, appeal_id) REFERENCES appeals (guild_id, id),
  CHECK (NOT closed_by_void OR (kind = 'closed' AND actor_id IS NULL))
);
CREATE INDEX appeal_events_appeal ON appeal_events (appeal_id, created_at, id);
```

### 15.5 Tickets

Ticket tables come before DMs and the moderation log, which reference them.

```sql
CREATE TABLE tickets (
  id                   uuid PRIMARY KEY DEFAULT uuidv7(),
  guild_id             bigint NOT NULL REFERENCES guilds (guild_id),
  member_id            bigint NOT NULL CHECK (member_id > 0),
  state                ticket_state NOT NULL DEFAULT 'opening',
  entry_channel_id     bigint NOT NULL,                           -- the thread's parent
  thread_id            bigint NULL UNIQUE,                        -- named ticket-<id u22>
  opened_at            timestamptz NULL,
  close_requested_at   timestamptz NULL,
  closed_at            timestamptz NULL,
  closed_by            bigint NULL,                               -- member or staff; NULL = Quack
  close_reason         ticket_close_reason NULL,
  journal_gaps         boolean NOT NULL DEFAULT false,            -- the journal may be missing messages
  welcome_started_at   timestamptz NULL,                          -- the welcome's nonce window (§10.4)
  welcome_posted_at    timestamptz NULL,
  frozen_at            timestamptz NULL,                          -- close step 1 done
  transcript_saved_at  timestamptz NULL,                          -- close step 2 done
  thread_deleted_at    timestamptz NULL,                          -- close step 4 done
  thread_delete_failed boolean NOT NULL DEFAULT false,
  created_at           timestamptz NOT NULL DEFAULT now(),
  updated_at           timestamptz NOT NULL DEFAULT now(),
  UNIQUE (guild_id, id),
  UNIQUE (guild_id, id, member_id),
  CHECK ((state = 'closed') = (closed_at IS NOT NULL)),
  CHECK (state <> 'open' OR (thread_id IS NOT NULL AND opened_at IS NOT NULL)),
  CHECK (close_reason IS NULL OR state IN ('closing', 'closed'))
);
CREATE UNIQUE INDEX tickets_one_live ON tickets (guild_id, member_id) WHERE state IN ('opening', 'open', 'closing');
CREATE INDEX tickets_guild_state ON tickets (guild_id, state, created_at, id);

-- Ticket journal: original text of every message seen in a ticket thread before it froze.
CREATE TABLE ticket_messages (
  guild_id    bigint NOT NULL,
  ticket_id   uuid NOT NULL,
  message_id  bigint NOT NULL,
  author_id   bigint NOT NULL,
  author_name text NOT NULL CHECK (char_length(author_name) >= 1),
  text        text NOT NULL,
  files       jsonb NOT NULL DEFAULT '[]' CHECK (jsonb_typeof(files) = 'array'),  -- [{name, size}]
  sent_at     timestamptz NOT NULL,
  expires_at  timestamptz NULL,                                   -- set at close
  PRIMARY KEY (ticket_id, message_id),
  FOREIGN KEY (guild_id, ticket_id) REFERENCES tickets (guild_id, id)
);
CREATE INDEX ticket_messages_expiry ON ticket_messages (expires_at) WHERE expires_at IS NOT NULL;

CREATE TABLE ticket_transcripts (
  ticket_id     uuid PRIMARY KEY,
  guild_id      bigint NOT NULL,
  content       text NOT NULL CHECK (octet_length(content) <= 8388608),
  complete      boolean NOT NULL,
  partial_cause transcript_partial NULL,
  captured_at   timestamptz NOT NULL DEFAULT now(),
  expires_at    timestamptz NOT NULL,
  FOREIGN KEY (guild_id, ticket_id) REFERENCES tickets (guild_id, id),
  CHECK (complete = (partial_cause IS NULL))
);
CREATE INDEX ticket_transcripts_expiry ON ticket_transcripts (expires_at);
```

### 15.6 DMs, moderation log, tracked messages, jobs, operation keys, drafts, import

```sql
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
  first_attempt_at     timestamptz NULL,                          -- start of the nonce window (§10.4)
  settled_at           timestamptz NULL,
  created_at           timestamptz NOT NULL DEFAULT now(),
  UNIQUE (appeal_event_id),
  UNIQUE (ticket_id),
  FOREIGN KEY (guild_id, case_id, member_id) REFERENCES cases (guild_id, id, member_id),
  FOREIGN KEY (guild_id, appeal_event_id) REFERENCES appeal_events (guild_id, id),
  FOREIGN KEY (guild_id, ticket_id, member_id) REFERENCES tickets (guild_id, id, member_id),
  CHECK (kind <> 'case'          OR (case_id IS NOT NULL AND appeal_event_id IS NULL AND ticket_id IS NULL)),
  CHECK (kind <> 'appeal_update' OR (case_id IS NOT NULL AND appeal_event_id IS NOT NULL AND ticket_id IS NULL)),
  CHECK (kind <> 'ticket_closed' OR (ticket_id IS NOT NULL AND case_id IS NULL AND appeal_event_id IS NULL)),
  CHECK ((state = 'failed') = (failure IS NOT NULL)),
  CHECK (state NOT IN ('sending', 'sent') OR (channel_id IS NOT NULL AND first_attempt_at IS NOT NULL)),
  CHECK ((state = 'sent') = (message_id IS NOT NULL)),
  CHECK ((settled_at IS NULL) = (state IN ('pending', 'sending')))
);
CREATE UNIQUE INDEX dms_one_per_case ON dms (case_id) WHERE kind = 'case';

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
CREATE TRIGGER mod_log_no_change BEFORE UPDATE OR DELETE ON mod_log
  FOR EACH ROW EXECUTE FUNCTION reject_change();
CREATE TRIGGER mod_log_no_truncate BEFORE TRUNCATE ON mod_log
  FOR EACH STATEMENT EXECUTE FUNCTION reject_change();

-- Tracked Discord messages (§10). The owner column matches the kind.
CREATE TABLE messages (
  id                uuid PRIMARY KEY DEFAULT uuidv7(),
  guild_id          bigint NOT NULL REFERENCES guilds (guild_id),
  kind              message_kind NOT NULL,
  case_id           uuid NULL,
  appeal_id         uuid NULL,
  ticket_id         uuid NULL,
  channel_id        bigint NULL,                                  -- NULL: an appeal post waiting for a channel
  message_id        bigint NULL,                                  -- NULL until posted
  generation        integer NOT NULL DEFAULT 1 CHECK (generation >= 1),
  create_started_at timestamptz NULL,                             -- the current create's nonce window (§10.4)
  private           boolean NOT NULL DEFAULT false,               -- a private receipt (token edits only)
  interaction_token text NULL,                                    -- secret; never logged
  token_expires_at  timestamptz NULL,
  wanted_revision   integer NOT NULL DEFAULT 1,
  posted_revision   integer NOT NULL DEFAULT 0,
  wanted_at         timestamptz NOT NULL DEFAULT now(),
  state             message_state NOT NULL DEFAULT 'active',
  stale             jsonb NOT NULL DEFAULT '[]'                   -- [{channel_id, message_id}] to clean up
                    CHECK (jsonb_typeof(stale) = 'array' AND jsonb_array_length(stale) <= 5),
  attachment_id     bigint NULL,                                  -- the attached transcript file
  last_error        text NULL CHECK (char_length(last_error) <= 200),  -- machine code, e.g. "http_403_50001"
  created_at        timestamptz NOT NULL DEFAULT now(),
  updated_at        timestamptz NOT NULL DEFAULT now(),
  FOREIGN KEY (guild_id, case_id)   REFERENCES cases (guild_id, id),
  FOREIGN KEY (guild_id, appeal_id) REFERENCES appeals (guild_id, id),
  FOREIGN KEY (guild_id, ticket_id) REFERENCES tickets (guild_id, id),
  CHECK ((kind = 'case_receipt') = (case_id IS NOT NULL)),
  CHECK ((kind = 'appeal_post') = (appeal_id IS NOT NULL)),
  CHECK ((kind = 'ticket_post') = (ticket_id IS NOT NULL)),
  CHECK (channel_id IS NOT NULL OR kind = 'appeal_post'),
  CHECK ((interaction_token IS NULL) = (token_expires_at IS NULL)),
  CHECK (interaction_token IS NULL OR kind = 'case_receipt'),
  CHECK (NOT private OR kind = 'case_receipt'),
  CHECK (wanted_revision >= 1 AND posted_revision BETWEEN 0 AND wanted_revision)
);
CREATE UNIQUE INDEX messages_case      ON messages (case_id) WHERE case_id IS NOT NULL;
CREATE UNIQUE INDEX messages_appeal    ON messages (appeal_id) WHERE appeal_id IS NOT NULL;
CREATE UNIQUE INDEX messages_ticket    ON messages (ticket_id) WHERE ticket_id IS NOT NULL;
CREATE UNIQUE INDEX messages_singleton ON messages (guild_id, kind) WHERE kind IN ('ticket_panel', 'honeypot_warning');
CREATE INDEX messages_location         ON messages (channel_id, message_id) WHERE message_id IS NOT NULL;
CREATE INDEX messages_waiting          ON messages (guild_id) WHERE kind = 'appeal_post' AND (channel_id IS NULL OR state = 'gone');

-- The job queue (§16).
CREATE TABLE jobs (
  id            uuid PRIMARY KEY DEFAULT uuidv7(),
  kind          job_kind NOT NULL,
  key           text NOT NULL UNIQUE CHECK (char_length(key) BETWEEN 1 AND 200),  -- <kind>:<owner ids>
  guild_id      bigint NULL REFERENCES guilds (guild_id),         -- NULL only for housekeeping
  payload       jsonb NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(payload) = 'object'),  -- immutable IDs only
  run_at        timestamptz NOT NULL DEFAULT now(),
  attempts      integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
  max_attempts  integer NOT NULL CHECK (max_attempts >= 1),
  claim_token   uuid NULL,                                        -- the current claim's fence
  locked_until  timestamptz NULL,
  rerun_at      timestamptz NULL,                                 -- enqueued again while claimed
  last_error    text NULL CHECK (char_length(last_error) <= 500),
  trace_context text NULL CHECK (char_length(trace_context) <= 256),  -- W3C traceparent of the first enqueuer
  dead_at       timestamptz NULL,
  created_at    timestamptz NOT NULL DEFAULT now(),
  updated_at    timestamptz NOT NULL DEFAULT now(),
  CHECK (key = kind::text OR starts_with(key, kind::text || ':')),
  CHECK ((claim_token IS NULL) = (locked_until IS NULL)),
  CHECK (dead_at IS NULL OR claim_token IS NULL),
  CHECK ((kind = 'housekeeping') = (guild_id IS NULL))
);
CREATE INDEX jobs_runnable ON jobs (kind, run_at, id) WHERE dead_at IS NULL AND locked_until IS NULL;
CREATE INDEX jobs_leased   ON jobs (locked_until) WHERE locked_until IS NOT NULL;
CREATE INDEX jobs_dead     ON jobs (kind, dead_at) WHERE dead_at IS NOT NULL;

-- Operation keys (§4.3). `response` is a result reference, e.g. {"case_id": "…"}.
CREATE TABLE operation_keys (
  guild_id        bigint NOT NULL CHECK (guild_id >= 0),          -- 0 = no server (DMs, /api/me)
  account_id      bigint NOT NULL CHECK (account_id >= 0),        -- 0 = Quack
  operation       text NOT NULL CHECK (char_length(operation) BETWEEN 1 AND 120),
  key             text NOT NULL CHECK (char_length(key) BETWEEN 1 AND 200),
  request_hash    bytea NOT NULL CHECK (octet_length(request_hash) = 32),
  response_status smallint NOT NULL,
  response        jsonb NOT NULL CHECK (jsonb_typeof(response) = 'object'),
  created_at      timestamptz NOT NULL DEFAULT now(),
  expires_at      timestamptz NOT NULL,
  PRIMARY KEY (guild_id, account_id, operation, key)
);
CREATE INDEX operation_keys_expiry ON operation_keys (expires_at);

-- /case add drafts (§11.6).
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
  page               smallint NOT NULL DEFAULT 1 CHECK (page BETWEEN 1 AND 3),  -- 3 = every page saved
  expires_at         timestamptz NOT NULL,
  created_at         timestamptz NOT NULL DEFAULT now(),
  updated_at         timestamptz NOT NULL DEFAULT now(),
  FOREIGN KEY (guild_id, rule_id) REFERENCES rules (guild_id, id),
  FOREIGN KEY (rule_id, rule_version) REFERENCES rule_versions (rule_id, version),
  CHECK ((rule_id IS NULL) = (rule_version IS NULL)),
  CHECK ((source_channel_id IS NULL) = (source_message_id IS NULL))
);
CREATE INDEX case_drafts_expiry ON case_drafts (expires_at);

-- Importer runs and per-server completion (§19).
CREATE TABLE import_runs (
  id                 uuid PRIMARY KEY DEFAULT uuidv7(),
  mode               text NOT NULL CHECK (mode IN ('dry_run', 'apply')),
  source_fingerprint text NOT NULL,                               -- SHA-256 over the source snapshot (§19.11)
  transform_version  integer NOT NULL,
  evaluated_at       timestamptz NOT NULL,                        -- the fixed "now" of the import
  started_at         timestamptz NOT NULL DEFAULT now(),
  finished_at        timestamptz NULL,
  report             jsonb NULL
);
CREATE TABLE import_guilds (
  guild_id           bigint PRIMARY KEY REFERENCES guilds (guild_id),
  run_id             uuid NOT NULL REFERENCES import_runs (id),
  source_fingerprint text NOT NULL,
  imported_at        timestamptz NOT NULL DEFAULT now()
);
```

### 15.7 Honeypot hits

```sql
-- One row per post in the honeypot channel. Pruned after 30 days (§16.6).
CREATE TABLE honeypot_hits (
  guild_id          bigint NOT NULL REFERENCES guilds (guild_id),
  message_id        bigint NOT NULL,
  channel_id        bigint NOT NULL,
  member_id         bigint NOT NULL,
  settings_version  bigint NOT NULL,                              -- honeypot_settings.version when admitted
  rule_id           uuid NULL,                                    -- honeypot_settings.rule_id when admitted
  state             honeypot_hit_state NOT NULL DEFAULT 'pending',
  case_id           uuid NULL,
  failure           text NULL CHECK (char_length(failure) <= 64), -- HoneypotFailure: a MemberRefusal code, rule_unavailable, policy_changed, discord_unavailable, exempt, imported_pending
  cleaned_up_at     timestamptz NULL,                             -- the post was deleted
  cleanup_failed_at timestamptz NULL,
  created_at        timestamptz NOT NULL DEFAULT now(),
  updated_at        timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (guild_id, message_id),
  FOREIGN KEY (guild_id, case_id, member_id) REFERENCES cases (guild_id, id, member_id),
  FOREIGN KEY (guild_id, rule_id) REFERENCES rules (guild_id, id),
  CHECK ((state = 'case_created') = (case_id IS NOT NULL)),
  CHECK ((state = 'failed') = (failure IS NOT NULL)),
  CHECK (cleaned_up_at IS NULL OR cleanup_failed_at IS NULL)
);
CREATE INDEX honeypot_hits_pending ON honeypot_hits (guild_id, member_id, message_id) WHERE state = 'pending';
CREATE INDEX honeypot_hits_burst   ON honeypot_hits (guild_id, member_id, created_at) WHERE state = 'case_created';
CREATE INDEX honeypot_hits_expiry  ON honeypot_hits (created_at);
```

### 15.8 Ownership notes

- `jobs` rows are work, not records: deleted on success, kept as `dead` for
  7 days.
- Nothing references `sessions`, `oauth_states`, `operation_keys`,
  `case_drafts`, or `jobs`; housekeeping deletes their expired rows.
  `honeypot_hits` are pruned after 30 days (the warning counts honeypot
  cases from `cases`, not hits).
- Rows in `cases`, `rule_versions`, `mod_log`, `appeals`, `appeal_events`,
  `actions`, `action_attempts`, `dms`, and `evidence*` are never deleted.
- Each job's payload holds only immutable IDs; the handler loads and checks
  every row it names against the job's `guild_id`.

### 15.9 Hot-path queries

| Query | SQL shape | Index |
| --- | --- | --- |
| Case number | `UPDATE case_counters SET last_number = last_number + 1 WHERE guild_id = $1 RETURNING last_number` | `case_counters_pkey` |
| Rule under lock | `SELECT current_version, archived_at FROM rules WHERE guild_id = $1 AND id = $2 FOR SHARE` | `rules_guild_id_id_key` |
| Escalation count | `SELECT count(*) FROM cases WHERE guild_id = $1 AND member_id = $2 AND rule_id = $3 AND voided_at IS NULL AND source <> 'imported' AND created_at >= $4` | `cases_escalation` |
| Case by number | `… WHERE guild_id = $1 AND number = $2` | `cases_guild_id_number_key` |
| Case list, newest first | `… WHERE guild_id = $1 AND number < $cursor ORDER BY number DESC LIMIT $n` (+ filters) | `cases_guild_id_number_key`; `cases_member` / `cases_rule` / `cases_moderator` with those filters |
| A member's own cases | `… WHERE member_id = $1 AND (created_at, id) < $cursor ORDER BY created_at DESC, id DESC` | `cases_member_all` |
| Undo guard, member busy | §5.6; `… WHERE guild_id = $1 AND member_id = $2 AND state = 'running'` | `actions_member` |
| Failed actions | `… WHERE guild_id = $1 AND state = 'failed' AND dismissed_at IS NULL ORDER BY created_at DESC, id DESC` | `actions_review` (dismissed ones: `actions_guild_state`) |
| Job claim | §16.2 | `jobs_runnable`; the lease sweep uses `jobs_leased` |
| Operation key replay | `SELECT … FROM operation_keys WHERE guild_id = $1 AND account_id = $2 AND operation = $3 AND key = $4` | `operation_keys_pkey` |
| Session | `SELECT … FROM sessions WHERE id_hash = $1 AND expires_at > now() AND key_fingerprint = $2` | `sessions_pkey` |
| Moderation log page | `… WHERE guild_id = $1 AND (created_at, id) < $cursor ORDER BY created_at DESC, id DESC LIMIT $n` | `mod_log_guild_time` (or the filter's partial index) |
| Case timeline | `… FROM mod_log WHERE case_id = $1 ORDER BY created_at, id` | `mod_log_case` |
| Pending appeals | `… WHERE guild_id = $1 AND state = 'pending' ORDER BY created_at, id` | `appeals_guild_state` |
| Tracked message bump | §10.2 | `messages_case` / `messages_appeal` / `messages_ticket` / `messages_singleton` |
| Tracked message by location | `… WHERE channel_id = $1 AND message_id = $2` | `messages_location` |
| Rules cache load | `SELECT r.*, v.definition FROM rules r JOIN rule_versions v ON v.rule_id = r.id AND v.version = r.current_version WHERE r.guild_id = ANY($1)` | `rules_guild`, `rule_versions_pkey` |
| Live ticket of a member | `… WHERE guild_id = $1 AND member_id = $2 AND state IN ('opening','open','closing')` | `tickets_one_live` |
| Honeypot batch | `… WHERE guild_id = $1 AND member_id = $2 AND state = 'pending' ORDER BY message_id` | `honeypot_hits_pending` |
| Honeypot burst | `… WHERE guild_id = $1 AND member_id = $2 AND state = 'case_created' AND created_at >= $3` | `honeypot_hits_burst` |
| Statistics | one `count(*) … GROUP BY` per bucket over `cases` / `actions` / `appeals` / `mod_log` `WHERE guild_id = $1 AND created_at >= $from AND created_at < $to`; actions by the action's own `created_at` (an undo counts when it was requested, not when its case was created) | `cases_created`, `mod_log_guild_time`, `appeals_guild_created`, `actions_guild_state` |
| Expiry sweeps | `DELETE … WHERE ctid IN (SELECT ctid … WHERE expires_at < now() LIMIT 1000)`, repeated | `*_expiry` |

### 15.10 Later migrations

Each is its own immutable file in `crates/quack-db/migrations/`, applied
after `0001_initial.sql`.

**`0002_log_channel_posted_at.sql`** — when the log channel last accepted a
post, for the "once per outage" rule of §9.4. Set by the `post_mod_log`
job on `Done`; never shown, never part of `version`, no `NOTIFY`. Imported
servers start with NULL.

```sql
ALTER TABLE guild_settings ADD COLUMN log_channel_posted_at timestamptz NULL;
```

---

## 16. Job queue

All background work is a row in `jobs`. Jobs are inserted in the same
transaction as the change that needs them, so committed work is never lost
and rolled-back work never runs.

### 16.1 Lifecycle

```
enqueue ──> runnable (run_at ≤ now, unclaimed) ──claim──> claimed (attempts + 1, token, lease)
                ^                                            │
                │   Retry(at) / Defer(at) / Continue          │  Done ──> deleted (or runnable again at rerun_at)
                └────────────────────────────────────────────┤
                  lease expired: sweep unclaims it ──────────┘  attempts > max ──> finalize ──> Done / Dead
```

- **Claim** returns a fresh `claim_token` and `locked_until`, both from
  PostgreSQL (`now()`), never from the worker's clock.
- **Fence.** Every transaction a handler runs (start, progress, settle)
  ends with the fence: `UPDATE jobs SET updated_at = now() WHERE id = $1 AND
  claim_token = $2 AND locked_until > now()` (or, in the settle, the job's
  own update or delete with the same predicate). No row → the whole
  transaction rolls back, domain writes included, and the handler stops: it
  lost its lease. Running it last keeps `jobs` last in the lock order
  (§4.12). In
  addition every domain write is a compare-and-set on the state the handler
  read (an action still `running` with the attempt it opened, a DM still
  `sending`, a message row still at its generation), so a handler that lost
  its lease can never overwrite newer state.
- **Deadline.** Each handler runs under a hard deadline of its lease minus
  15 seconds (§16.5). Before starting a Discord call it checks the time left;
  when less than that call's deadline plus 5 seconds remains, it saves
  progress and returns `Continue`. A handler that hits the deadline is
  dropped; dropping a future may not stop a request Discord already
  received, which is why every effect is read back before it is repeated
  (§5.1, §10.4).
- **Lease sweep.** Each dispatcher pass unclaims expired leases (`UPDATE
  jobs SET claim_token = NULL, locked_until = NULL WHERE locked_until <
  now() AND dead_at IS NULL`, index `jobs_leased`) and counts
  `quack.job.lease_expired`. The handler that runs next finds the domain
  record mid-effect (an open attempt, a `sending` DM) and resolves it by its
  own rule.
- **Budget.** A claim increments `attempts`. When `attempts >
  max_attempts`, or a handler returns `Retry` on its last attempt, the
  runner calls the kind's **finalize** instead: a database-only
  transaction that settles the domain record terminally (§16.5) and
  finishes the job. Finalize never calls Discord, so a job can never run
  past its budget, however often its process crashes.

`JobResult` (returned by handlers; applied in the handler's settle
transaction, `quack_db::jobs::settle(conn: &mut PgConnection, job: &ClaimedJob, result: &JobResult)`):

| Result | Effect |
| --- | --- |
| `Done` | Delete the row; if `rerun_at` is set (enqueued again while claimed), keep it instead: unclaim, `attempts = 0`, `run_at = rerun_at`, `rerun_at = NULL`. |
| `Retry { at, error }` | Unclaim, `run_at = at`, `last_error = error`. |
| `Defer { at }` | Like `Retry` but `attempts = attempts - 1`: the run did not count (a busy member, a DM still sending). |
| `Continue` | `Defer` at now: progress is saved and the work continues in a new run. |
| `Dead { error }` | `dead_at = now()`, unclaim, `last_error`. Only for a job whose records are missing or malformed; the domain is already terminal or has nothing to settle. |
| `Reschedule { at }` | Periodic jobs: unclaim, `attempts = 0`, `run_at = at`. |

When `rerun_at` is set (the job was enqueued again while claimed), `Retry`,
`Defer`, `Continue`, and `Reschedule` run it at the earlier of their own
time and `rerun_at`, and clear `rerun_at`; `Dead` clears it too (a dead job
is revived only by a later enqueue, §16.3). Every result is applied only
while the claim holds (`claim_token` matches and `locked_until > now()`);
otherwise `settle` returns `DbError::Stale` and changes nothing.

### 16.2 Claim

```sql
WITH picked AS (
  SELECT id FROM jobs
  WHERE kind = $1 AND dead_at IS NULL AND locked_until IS NULL AND run_at <= now()
  ORDER BY run_at, id
  LIMIT $2
  FOR UPDATE SKIP LOCKED
)
UPDATE jobs j
SET claim_token  = gen_random_uuid(),
    locked_until = now() + make_interval(secs => $3),
    attempts     = j.attempts + 1,
    updated_at   = now()
FROM picked
WHERE j.id = picked.id
RETURNING j.id, j.kind, j.key, j.guild_id, j.payload, j.attempts, j.max_attempts,
          j.claim_token, j.locked_until, j.trace_context, j.run_at, j.created_at;
```

Uses `jobs_runnable` (verified with `EXPLAIN` on PostgreSQL 18: an index
scan with no sort). `$2` is the number of permits the dispatcher holds.

### 16.3 Enqueue and coalescing

Every job is keyed: `key` is `<kind>:<owner IDs>` built by
`quack_db::jobs::NewJob`'s per-kind constructors (`NewJob::run_action`,
`NewJob::honeypot_case`, …; `JobKey` has no public constructor) from the
kind and the full owner identity
(`run_action:<action id>`, `honeypot_case:<guild>:<member>`), and the
payload holds only immutable IDs, so one key always means the same payload.

```sql
INSERT INTO jobs (kind, key, guild_id, payload, run_at, max_attempts, trace_context)
VALUES ($1, $2, $3, $4, $5, $6, $7)
ON CONFLICT (key) DO UPDATE SET
  run_at     = CASE WHEN jobs.locked_until IS NULL THEN LEAST(jobs.run_at, EXCLUDED.run_at) ELSE jobs.run_at END,
  rerun_at   = CASE WHEN jobs.locked_until IS NOT NULL
                    THEN LEAST(COALESCE(jobs.rerun_at, EXCLUDED.run_at), EXCLUDED.run_at) ELSE jobs.rerun_at END,
  attempts   = CASE WHEN jobs.dead_at IS NOT NULL THEN 0 ELSE jobs.attempts END,
  dead_at    = NULL,
  updated_at = now();
SELECT pg_notify('quack_jobs', $1::text);
```

A keyed job therefore exists at most once: enqueueing it again while it
waits moves it earlier; while it is claimed, records `rerun_at` so it runs
once more after it finishes (never earlier than requested); when it is
dead, a new triggering change (a staff retry, a new bump) revives it with a
fresh budget. `trace_context` keeps the first enqueuer's. PostgreSQL
delivers the notification on commit and folds identical notifications in
one transaction. Metric `quack.jobs.enqueued{quack.job.kind}`.

### 16.4 Dispatch and wake-up

- One dispatcher task per kind. It wakes on `NOTIFY quack_jobs` for its
  kind, when one of its handlers finishes (permits freed), on a 2-second
  poll, or at the earliest `run_at` of its unclaimed jobs (whichever is
  first). Each pass sweeps expired leases (§16.1), then claims as many jobs
  as it holds permits for and starts them.
- Permits: a per-kind limit (§16.5) and a global limit
  `QUACK_WORKER_CONCURRENCY` (default 16). Global permits are handed out
  round-robin across kinds that are waiting, so one busy kind cannot starve
  the others. A freed global permit wakes the next waiting kind.
- One dedicated `PgListener` connection outside the pool; after it
  reconnects, every dispatcher runs a pass immediately (notifications may
  have been missed).
- Each handler runs inside a span `job {kind}` linked to `trace_context`
  (§18.4).
- Every dispatcher records a heartbeat at the end of each successful pass;
  readiness requires every heartbeat within 15 seconds (§14.9). A pass
  claims in batches of at most 8 (`RunnerConfig::claim_batch`), taking a
  kind permit and a global permit per job, and keeps claiming until the
  permits run out or a claim returns fewer jobs than asked. A kind that
  finds no global permit joins a FIFO waiting list and ends its pass;
  each finished job frees its permits and wakes its own kind and the first
  waiting kind. After a pass the dispatcher sleeps until a wake-up, the
  next due `run_at`, or the 2-second poll.
- The runner is generic and lives in `quack-db` (`jobs::Runner`, §17.2.1):
  the engine supplies one `Handler` for every kind,
  `Arc<dyn Fn(ClaimedJob, Phase) -> HandlerFuture>`, called with
  `Phase::Run` or, when the budget is spent or the last run returned
  `Retry`, `Phase::Finalize`. It returns `Handled::Settled(result)` when it
  already called `jobs::settle` in its own transaction with its domain
  writes, or `Handled::Settle(result)` for the runner to apply in a
  transaction of its own. `DbError::Stale` means the lease was lost; any
  other error is retried with the kind's backoff (`last_error =
  db_<class>`). Each run is bounded by the kind's deadline (lease minus
  15 s). The `notify::Listener` (one dedicated connection) yields
  `Notification::{Job(kind), Rules(guild), Settings(guild), Reconnected}`;
  `Runner::on_notification` wakes the matching dispatcher (all of them
  after `Reconnected`), and the binary routes `Rules`/`Settings` to the
  caches.
- Durable times (`run_at`, `locked_until`) come from PostgreSQL; local waits
  use monotonic time, and no wait is longer than the 2-second poll.
- Ordering is FIFO by `run_at` per kind; actions are additionally
  serialized per member (§5.4).

### 16.5 Job kinds

The handler deadline is the lease minus 15 seconds.

| Kind | Payload | Key | Per-kind limit | Lease | Backoff (after attempt n) | Max attempts | Why it is safe to repeat | Finalize (database only) |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `run_action` | `{action_id}` | `run_action:<action>` | 8 | 60 s | 5 s, 15 s, 1 min, 5 min, 15 min, 1 h, 3 h | 8 | Every attempt reads Discord before acting (§5.1); start and settle are compare-and-set on the action's state and attempt. | An open attempt → `interrupted`; the action → `failed` (`unconfirmed` or `retries_exhausted`, §5.5); wake nothing. |
| `send_dm` | `{dm_id}` | `send_dm:<dm>` | 4 | 120 s | 5 s, 30 s (only while `pending`) | 3 | One nonce per DM, repeated only inside the window (§10.4). | `pending` → `failed` (`discord_unavailable`); `sending` → `unknown`; wake a waiting kick or ban (§6.2). |
| `sync_message` | `{message_row_id}` | `sync_message:<row>` | 4 | 60 s | 2 s, 10 s, 1 min, 5 min, 30 min | 6 | One writer per row; renders current state; write-back compared on the generation (§10.3). | `last_error = "exhausted"`; the next bump tries again. |
| `post_mod_log` | `{entry_id, channel_id}` | `post_mod_log:<entry>` | 2 | 60 s | 10 s, 30 s, 2 min, 10 min, 1 h | 6 | Nonce = entry ID inside the window; the channel is pinned (§9.4). | `log_channel_failed` once per outage. |
| `capture_evidence` | `{evidence_id}` | `capture_evidence:<evidence>` | 4 | 120 s | 10 s, 1 min, 5 min | 4 | The snapshot is stored once under the case lock; each file has its own state and nonce (§7.2). | Pending files → `copy_failed`; the item → `saved` (snapshot stored) or `failed` (`discord_error`). |
| `honeypot_case` | `{member_id}` | `honeypot_case:<guild>:<member>` | 2 | 60 s | 5 s, 30 s, 2 min, 10 min | 5 | Case creation uses the batch's operation key; hit updates are guarded on `pending` (§13.4.2). | The batch's hits → `failed` (`discord_unavailable`); log `honeypot_case_failed`. |
| `honeypot_cleanup` | `{channel_id, message_id}` | `honeypot_cleanup:<guild>:<message>` | 2 | 30 s | 1 s, 2 s, 4 s, 8 s, 16 s, 32 s, 64 s, 64 s, 64 s | 10 | Delete is idempotent (404 = done). | `cleanup_failed_at`. |
| `open_ticket` | `{ticket_id}` | `open_ticket:<ticket>` | 2 | 60 s | 2 s, 10 s, 30 s | 4 | The thread is looked up by its full name before any create; each step checks saved progress (§13.2.3). | The open fails (§13.2.2). |
| `close_ticket` | `{ticket_id}` | `close_ticket:<ticket>` | 2 | 300 s | 5 s, 30 s, 2 min, 10 min, 1 h | 6 | Each step records its completion column (§13.2.4). | §13.2.4 finalization. |
| `housekeeping` | `{}` | `housekeeping` | 1 | 300 s | `Reschedule` | 1 | Deletes of expired rows. | `Reschedule` in 10 min. |

Every handler first checks that its server is still installed; when it is
not, an action fails `quack_not_in_server`, a tracked message becomes
`gone`, honeypot hits fail (`quack_left`), and a DM is still attempted
(Discord refuses it if Quack shares no server with the member).

### 16.6 Periodic work

- `housekeeping` is ensured at startup (`INSERT … ON CONFLICT (key) DO
  NOTHING`). Each run deletes, in batches of 1,000 for at most 60 seconds:
  expired `sessions` (and sessions under another `key_fingerprint`),
  `oauth_states`, `operation_keys`, `case_drafts`; `jobs` with `dead_at`
  older than 7 days; `ticket_transcripts` and `ticket_messages` past
  `expires_at`; `honeypot_hits` older than 30 days; journal rows over the
  cap (§13.2.5). It marks private receipts whose token expired `gone` and
  clears expired `interaction_token` values. When rows remain it
  reschedules at once, otherwise in 10 minutes.
- Database-backed gauges (§18.2) are observed by a separate 60-second task,
  not a job, using the same predicates as the claim (`jobs.ready` counts
  unclaimed, due, not dead rows). Gauges with no rows for a label report 0.

### 16.7 Shutdown

On shutdown (§17.5) dispatchers stop claiming at once. In-flight handlers
run until the drain deadline. At the deadline the Discord client stops
accepting new calls (they fail before anything is sent), then the remaining
handlers are dropped without settling. Their leases expire, and the next
process resolves each by the rules above. Nothing is lost because every
state change was committed before the Discord call (§5.4, §6.4).

---

## 17. Architecture

### 17.1 One binary

One binary, `quack`, runs as one process per Discord application. All
durable state is in PostgreSQL, so a restart loses nothing.

| Command | What it does | Needs |
| --- | --- | --- |
| `quack serve [--config <file>] [--migrate]` | Gateway, interactions, HTTP API, dashboard files, job workers. With `--migrate`, applies pending migrations first; otherwise refuses to start when migrations are not current. | all serve keys |
| `quack migrate [--config <file>]` | Applies pending migrations, prints the applied list. | `QUACK_DATABASE_URL` |
| `quack import-mysql [--dry-run] [--report <file>] [--config <file>]` | One-time import (§19) from `QUACK_IMPORT_SOURCE_URL`. | `QUACK_DATABASE_URL`, `QUACK_IMPORT_SOURCE_URL` |
| `quack commands sync [--config <file>]` | Registers slash commands (§11.2). | Discord token and application ID |

Exit codes: 0 success, 1 runtime failure, 2 usage or configuration error
(all problems printed at once).

### 17.2 Crates

Workspace in `crates/`, edition 2024, resolver 3, Rust 1.100, pinned
dependencies from the stack audit plus the additions in §20.5.

| Crate | Owns | Depends on | Public surface |
| --- | --- | --- | --- |
| `quack-core` | Pure domain. No I/O, no async, no SQL, no Discord library types. | — | `ids` (snowflake newtypes `GuildId`, `AccountId`, `MemberId`, `ChannelId`, `RoleId`, `MessageId`, `AttachmentId`; UUID newtypes `CaseId`, `RuleId`, `ActionId`, `AppealId`, `AppealEventId`, `DmId`, `EvidenceId`, `FileId`, `TicketId`, `MessageRowId`, `JobId`, `ModLogId`; `to_u22`/`from_u22`/`ulid_to_uuid`; `IdError`); `rule` (`RuleDefinition`, `Field`, `FieldKey`, `FieldKind`, `Level`, `LevelAction`, `Strictness`, `RuleProblem`, `RuleFile`, `starter_rule`, `honeypot_rule`); `escalation` (`select_level`, `LevelChoice`, `counting_cutoff`, `in_window`); `permissions` (`Permissions`, `Permission`, `DiscordPermission`, `Capability`, `Capabilities`, `derive_capabilities`, `Operation`, the `*Facts` types, `check_member`, `check_action_permission`, `action_precheck`, `check_staff_channel`, `MemberRefusal`, `ChannelProblem`); `case` (`CaseSource`, `VoidCause`, `CaseState`, `CaseDecision`, `ContextValues`, `validate_context`, `Decimal`, `MessageLink`, `find_message_links`); `action` (`ActionKind`, `ActionState`, `ActionFailure`, `AttemptOutcome`, `ActionEvent`, `transition`, `may_have_applied`, `controls`, `on_void`, `undo_guard`, `start`/`plan_from_member`/`after_call`/`settle`, case markers, `RETRY_BACKOFF`); `dm`, `evidence`, `appeal`, `messages` (states, events, transition tables, timing constants); `modlog` (`ModLogKind`, `ModLogEntry`, `ModLogLinks`, `redact`); `discord` (the call `Outcome` and payload types shared by `quack-discord` and the engine port, §17.3); `modules`; `settings`; `limits`; `DomainError`, `InvalidTransition`, `UnknownValue`; full map in §17.2.1. |
| `quack-text` | Every user-facing string (§12) and the neutral message model. | core | `Msg`, `Text`, `Inline`, `Block`, `Row`, `Button`, `Select`, `Modal`, `Tone`, `Person`, `Actor`, `ChannelRef`, `RoleRef`, `Command`, `TimeStyle` (from `model`); `ButtonAction`, `SelectAction`, `ModalAction` (from `actions`); `Surface` (`Discord(&CommandIds)` renders mentions, `Plain` renders names) and `CommandIds` (from `render`); one function per message id in `command`, `error`, `case` (incl. `CaseCard`, `ReceiptView`), `action`, `dm`, `evidence`, `appeal`, `log`, `setup`, `help`, `rules`, `ticket`, `activity`, `honeypot`, `api`, `timeline`; shared `word`, `fmt` (`duration`, `plural`, `escape_markdown`, `date`, `datetime`, …), `budget` (Discord limits, `check_msg`, `fit`); full map in §17.2.1. |
| `quack-telemetry` | OTel setup and the instrument catalog (`METRICS.md`, §18). | — | `init(&TelemetryConfig) -> Result<TelemetryGuard, TelemetryError>`, `init_with_exporters`, `Exporters`, `TelemetryConfig`, `LogFormat`, `SHUTDOWN_DEADLINE`, `TelemetryGuard::{is_exporting, with_deadline, force_flush, shutdown}`, `TelemetryError`; `metrics()` / `Metrics` (typed instrument families such as `JobMetrics`, `DbMetrics`, `CaseMetrics`); `catalog` (`INSTRUMENTS`, `ATTRIBUTES`, `render_markdown`); `labels` (`Attribute` and attribute enums); `keys`; `spans` (`interaction`, `gateway`, `http`, `job`/`JobRun`, `use_case`, `db`, `discord`, `record*`, `event`, `SpanEvent`); `current_trace_context`, `trace_context_of`, `parse_trace_context`; full map in §17.2.1. |
| `quack-db` | PostgreSQL: migrations, every query (sqlx macros, offline data in `.sqlx/`), the job queue, sessions, tracked messages. | core, telemetry | `Db` (`connect`, `begin`, `transaction`, `conn`), `Tx`, `TxError`, `DbError`/`Conflict`/`Violation`, `Applied`, `TimeCursor`, `migrate`, `verify_migrations`, `MigrationMismatch`, `Fence` (the singleton lock, §17.5); query modules `guilds`, `settings`, `rules`, `cases`, `actions`, `dms`, `evidence`, `appeals`, `modlog`, `messages`, `operation_keys`, `drafts`, `sessions`, `accounts`, `stats`, `tickets`, `activity_log`, `honeypot`, `import`, `housekeeping`; `jobs` (`JobKind`, `JobKey`, `NewJob`, `RunAt`, `enqueue`, `claim`, `sweep_leases`, `fence`, `settle`, `ClaimedJob`, `JobResult`, the generic `Runner` with the engine's `Handler`); `notify` (`rules_changed`, `settings_changed`, `Notification`, `Listener`); full map in §17.2.1. |
| `quack-discord` | Twilight 0.18: gateway, caches, REST with typed outcomes, rendering, custom IDs, command definitions. | core, text, telemetry | `DiscordClient`, `Gateway` (`run(tx)` yielding Twilight events), `GuildCache`, `MemberCache`, `render(&Msg) -> RenderedMessage`, `custom_id::{CustomId, encode, decode, decode_legacy}`, `commands::{definitions, sync}`, the interaction callbacks on `DiscordClient` (`defer`, `defer_update`, `reply`, `open_modal`, `autocomplete`, `edit_interaction_original`, `followup`), `permissions::{in_channel, in_guild}`. |
| `quack-engine` | Use cases and job handlers; defines the `Discord` port, the `Module` contract, and the neutral input types (`ModuleEvent`, `ComponentCall`, `FetchedMessage`, `SentMessage`). | core, text, db, telemetry | `Engine`, `Ctx`, `Principal`, `Discord` (trait), `Outcome`, `Refusal`, facts loader; `cases::{apply_rule, void_case, edit_context, add_evidence, get, list, preview, member_summary}`; `actions::{run, finalize, retry, dismiss, undo}`; `dms::{send, finalize}`; `evidence::{capture, finalize, resolve_file}`; `appeals::{submit, add_information, decide, get, list}`; `messages::{bump, sync}`; `modlog::{list, post}`; `rules::{create, edit, archive, restore, import, export, get, list}`; `settings::{get, patch, acknowledge_starter_notice}`; `setup::{create_channel, panel, apply}`; `guilds::{bootstrap, left, reconcile_channels}`; `stats::compute`; `drafts`; the job `Handler` (one closure for every kind) that the binary passes to `quack_db::jobs::Runner`, §16.4; `Module`, `ModuleSpec`; `RulesCache`, `ConfigCache`. |
| `quack-tickets`, `quack-activity-log`, `quack-honeypot` | The optional modules (§13). | core, text, db, engine, telemetry | `module() -> Arc<dyn Module>`; service functions and DTOs used by `quack-api` (`settings`, `update_settings`, `list`, `get`, `transcript`, `close` for tickets). |
| `quack-api` | axum HTTP API, OAuth (its own small traced client), sessions, CSRF, rate limits, OpenAPI, static files. | core, text, db, engine, modules, telemetry | `router(ApiState) -> axum::Router`, `ApiState`, `openapi::document()`, extractors (`SessionAccount`, `Staff<Capability>`, `MemberSelf`, `IdempotencyKey`), `ApiError`. |
| `quack-bot` | Interaction and gateway routing into engine calls and replies; hands module components and events to the modules. | core, text, engine, discord, db (draft and 2FA reads), telemetry; modules through its `Module` seam | `Bot::new`, `Bot::on_interaction`, `Bot::on_event`; `Responder` (the initial callback, edits, follow-ups; `DiscordResponder` over the client, a recorder in tests), `CommandMentions` (command ids shared by every renderer); `Module` and `ComponentCall` (the module seam until the module crates are connected). |
| `quack-import` | MySQL → PostgreSQL importer (only crate with sqlx `mysql`). | core, db | `run(ImportOptions) -> Result<Report>`, `Report`. |
| `quack-testkit` | Test helpers (dev-dependency only). | core, engine, db | `FakeDiscord`, `TestDb` (Postgres 18 in Testcontainers, migrated), `fixtures`. |
| `quack` (bin) | Config, CLI, wiring, shutdown, and `DiscordPort`. | everything | `main`; its library: `cli::parse`, `config::{Config, Need, load, validate}`, `serve::{run, Options, DiscordMode, Readiness, sync_commands}`, `DiscordPort`, and `wiring` (where the modules, the API router, and the importer are connected). |

`quack-discord` may not depend on `quack-engine` (adapters never import
each other, and the engine must not see Twilight). The binary therefore
contains `DiscordPort`, a small adapter that implements
`quack_engine::Discord` by calling `quack_discord::DiscordClient` and the
caches, mapping the few types the two shape differently. This placement is a Rust detail the
charter leaves open (§20.5).

Rules: dependencies point toward `quack-core`; adapters never import each
other; `quack-engine` uses Postgres concretely (tests run against real
Postgres) and reaches Discord only through `Discord`. No other traits exist
unless they have two real implementations (`Module` has three).

### 17.2.1 Public surface of the built crates

Generated from the code; private modules re-exported through a parent are
listed under the parent. "CAS" means a compare-and-set write that returns
whether it applied.

#### quack-core

Errors at the root: `DomainError` (the refusal codes: `MissingCapability(Capability)`,
`MfaRequired`, `MemberRefused(MemberRefusal)`, `CaseNotFound`,
`CaseAlreadyVoided`, `CaseVoided`, `ReplacementInvalid`, `RuleNotFound`,
`RuleArchived`, `RuleChanged`, `RuleNameTaken`, `TooManyRules`,
`RuleUsedByHoneypot`, `RequiredFieldOnHoneypotRule`, `ActionNotRetryable`,
`ActionNotDismissable`, `ActionNotUndoable`, `TimeoutWindowPassed`,
`AppealNotFound`, `AppealChanged`, `AppealExists`, `AppealNotAllowed`,
`EvidenceLimitReached { .. }`, `TooManyLinks`, `SettingsChanged`,
`TicketNotOpen`; methods `code()`, `http_status()`), `InvalidTransition {
machine, from, event }` (a state machine refused an event), `UnknownValue {
type_name, value }` (a stored name that is not a variant). Every fieldless
string enum (`str_enum!`) has `ALL`, `as_str()`, `FromStr` (→
`UnknownValue`), and serde by its `snake_case` name.

- `action` — actions (§5): kinds, states, the transition table, running an
  attempt, staff controls, the undo guard, and case markers.
  - Types: `ActionKind::{Timeout, Kick, Ban, RemoveTimeout, Unban}`
    (`purpose`, `is_original`, `undo_kind`, `undoes`, `permission`,
    `reads_member`), `ActionPurpose::{Original, Undo}`,
    `ActionState::{Pending, Running, Done, Failed, Cancelled, Skipped}`
    (`is_terminal`), `ActionFailure::{QuackNotInServer,
    QuackMissingPermission, QuackRoleTooLow, MemberIsOwner,
    MemberNotInServer, MemberIsAdministrator, TimeoutWindowPassed,
    DiscordRefused, RetriesExhausted, Unconfirmed}`,
    `ActionSkipReason::{OtherCaseApplies, ChangedOutsideCase}`, `Failure`
    (`new`, `missing_permission`, `discord_refused`),
    `AttemptOutcome::{Done, NoChange, Refused, NotSent, Unknown,
    Interrupted}` (`may_have_applied`), `ActionEvent::{CaseVoided,
    OtherCaseApplies, StartRefused, AttemptStarted, Settled { .. },
    StaffRetry, StaffDismiss}`; controls: `ActionView`, `Controls { retry,
    dismiss, undo }` (`can_retry`, `can_dismiss`, `can_undo`), `VoidFacts`,
    `VoidEffect`, `GuardCandidate`, `StillApplies`; running: `StartFacts`,
    `StartDecision::{Finish, SettleInterrupted, RetryAt, DeferUntil, Cancel,
    Skip, Fail, Open}`, `ActionTarget`, `CallOutcome::{Done, Refused,
    RetryAfter, NotSent, Unknown}`, `DiscordRefusal`,
    `AttemptResult::{Done, NoChange, ChangedOutsideCase, Refused { .. },
    NotSent { .. }, Unknown, Interrupted}`, `Plan::{Call, CheckMarker,
    Finish}`, `SettleFacts`, `Settlement`.
  - `fn transition(kind: ActionKind, state: ActionState, event: ActionEvent) -> Result<ActionState, InvalidTransition>`
  - `fn may_have_applied(attempts: &[Option<AttemptOutcome>]) -> bool`
  - `fn controls(action: &ActionView, case_valid: bool, now: OffsetDateTime) -> Controls`
  - `fn on_void(facts: &VoidFacts<'_>) -> VoidEffect`
  - `fn undo_guard(undo: ActionKind, own_case: CaseId, candidates: &[GuardCandidate], now: OffsetDateTime) -> Option<StillApplies>`
  - `fn undo_needed(kind: ActionKind, timeout_ends_at: Option<OffsetDateTime>, now: OffsetDateTime) -> bool`
  - `fn start(facts: &StartFacts) -> StartDecision`
  - `fn plan_from_member(target: &ActionTarget, member: Option<&MemberFacts>, now: OffsetDateTime) -> Result<Plan, InvalidTransition>`,
    `fn plan_from_ban(target: &ActionTarget, ban: Option<&BanFacts>) -> Result<Plan, InvalidTransition>`,
    `const fn plan_after_marker(voided_ban_case_of_member: bool) -> Plan`
  - `fn after_call(target: &ActionTarget, outcome: CallOutcome, now: OffsetDateTime) -> AttemptResult`,
    `fn after_read_failure(outcome: CallOutcome) -> AttemptResult`
  - `fn settle(facts: &SettleFacts<'_>, result: AttemptResult) -> Result<Settlement, InvalidTransition>`
  - `fn retry_delay(attempt: u32, retry_after: Option<Duration>) -> Option<Duration>`,
    `fn timeout_ends_at(case_created_at, duration_seconds: u32) -> OffsetDateTime`,
    `fn timeout_window_passed(now, ends_at) -> bool`
  - Markers: `fn case_marker(case_number: u64, rule_name: &str) -> String`,
    `fn undo_marker(case_number: u64) -> String`,
    `fn parse_case_marker(reason: &str) -> Option<u64>`.
  - Constants: `MAX_ATTEMPTS` (8), `RETRY_BACKOFF: [Duration; 7]`,
    `MEMBER_BUSY_DEFER` (2 s), `AUDIT_LOG_REASON_MAX_CHARS` (512).
- `appeal` — appeals (§8): eligibility, states, and the transition table.
  - Types: `AppealState::{Pending, NeedsInformation, Accepted, Rejected,
    Closed}` (`is_open`), `AppealEventKind::{Submitted,
    InformationRequested, InformationProvided, Accepted, Rejected, Closed,
    Reopened}`, `AppealDecision::{RequestInformation, Accept, Reject, Close,
    Reopen}` (`event_kind`), `AppealActor::{Member, Staff}`, `AppealRow`,
    `AppealTransition`, `EligibilityFacts`.
  - `fn transition(row: &AppealRow, case_valid: bool, event: AppealEventKind, actor: AppealActor, expected_version: u32) -> Result<AppealTransition, DomainError>`
  - `const fn close_by_void(row: &AppealRow) -> Option<AppealTransition>`,
    `fn next_version(current: u32, expected: u32) -> Result<u32, DomainError>`,
    `fn eligibility(facts: &EligibilityFacts) -> Result<(), DomainError>`,
    `fn statement_ok(&str) -> bool`, `fn decision_reason_ok(&str) -> bool`.
  - Constants: `CLOSED_BECAUSE_VOIDED`, `APPEAL_ACCEPTED_VOID_REASON`.
- `case` — cases (§4): sources, the stored decision, validity and voiding,
  replacement links, and case context.
  - Types: `CaseSource::{Discord, Dashboard, Honeypot, Imported}`,
    `VoidCause::{Staff, AppealAccepted}`, `CaseState::{Valid, Voided}`
    (`from_voided`, `void`), `CaseDecision` (`decide`, `from_choice`,
    `action_kind`, `timeout_seconds`, `ban_delete_message_seconds`),
    `ContextInput::{Json, JsonNumber, Text}`, `ContextValue::{Text, Boolean,
    Number}`, `ContextValues` (`get`, `with_kinds`, `evidence_links`,
    `message_links`, `changed_keys`), `ContextProblem { key, code }`,
    `ContextProblemCode::{UnknownField, DuplicateField, Required, TooLong,
    NotText, NotYesNo, NotANumber, NumberOutOfRange, NotAMessageLink,
    OtherServer}`, `Decimal` (`parse`, `millionths`, `from_millionths`,
    `ZERO`), `DecimalError::{NotANumber, OutOfRange}`, `MessageLink`
    (`parse`, `parse_in`, `to_url`), `LinkProblem::{NotAMessageLink,
    OtherServer}`.
  - `fn validate_context(fields: &[Field], guild: GuildId, submitted: &[(String, ContextInput)]) -> Result<ContextValues, Vec<ContextProblem>>`
  - `fn fill_message_links(fields: &[Field], submitted: &mut Vec<(String, ContextInput)>, link: &MessageLink)`,
    `fn find_message_links(text: &str) -> Vec<MessageLink>`,
    `const fn check_replacement(voided: bool, already_replaced: bool) -> Result<(), DomainError>`.
- `dm` — DMs (§6): the one direct message per case, appeal decision, or
  closed ticket, sent at most once.
  - Types: `DmKind::{Case, AppealUpdate, TicketClosed}`,
    `DmState::{Pending, Sending, Sent, Failed, Unknown, NotNeeded}`
    (`is_terminal`), `DmFailure::{DmsClosed, NotInSharedServer,
    DiscordUnavailable, DiscordRefused}` (`from_refusal`),
    `DmEvent::{CaseVoided, Started, OpenRefused, Created, CreateRefused { .. },
    RetryAfterBeyondWindow { .. }, WindowEnded, BudgetSpent}`.
  - `fn transition(kind: DmKind, state: DmState, event: DmEvent) -> Result<DmState, InvalidTransition>`,
    `const fn failure_for(event, refusal_code: Option<u32>) -> Option<DmFailure>`,
    `const fn releases_waiting_action(kind, to: DmState) -> bool`,
    `fn in_window(first_attempt_at, now) -> bool`, `fn wait_fits(first_attempt_at, now, wait) -> bool`.
  - Constants: `SEND_ATTEMPTS` (3), `SEND_BACKOFF`, `UNKNOWN_REPEAT_DELAY`,
    `ACTION_WAIT` (120 s).
- `escalation` — choosing a rule's level from the member's case count
  (§3.4).
  - `LevelChoice { index, from_case_count, action, send_dm }`.
  - `fn select_level(def: &RuleDefinition, case_count: u32) -> Option<LevelChoice>`
  - `fn counting_cutoff(now: OffsetDateTime, counting_window_days: Option<u16>) -> Option<OffsetDateTime>`
  - `fn in_window(created_at: OffsetDateTime, cutoff: Option<OffsetDateTime>) -> bool`
- `evidence` — evidence (§7): saved message snapshots and their files.
  - Types: `EvidenceState::{Saving, Saved, Failed}`,
    `EvidenceFailure::{MessageDeleted, ModeratorNoAccess, QuackNoAccess,
    OtherServer, DiscordError}`, `FileCopyState::{Pending, Copied, LinkOnly,
    TooLarge, UnsupportedType, CopyFailed}`, `AddAgain::{AlreadySaved,
    CaptureAgain}`. Capture and retry move the state in SQL.
  - `const fn add_again(state) -> AddAgain`,
    `fn copy_eligibility(size_bytes: u64, content_type: Option<&str>) -> Result<(), FileCopyState>`,
    `fn check_room(existing_items: u32, new_items: usize, links_in_call: usize) -> Result<(), DomainError>`,
    `fn files_within_caps(case_files: u32, files: u32) -> u32`.
- `ids` — typed ids: snowflakes (positive, below 2^63; `bigint` in SQL,
  decimal strings in JSON) and UUIDv7 record ids (`u22` text form).
  - Snowflakes: `GuildId`, `AccountId`, `MemberId`, `ChannelId`, `RoleId`,
    `MessageId`, `AttachmentId` (`new`, `get`, `to_i64`, `TryFrom<i64|u64>`,
    `FromStr`; `MemberId::account`, `AccountId::member`,
    `GuildId::everyone_role`).
  - UUIDs: `CaseId`, `RuleId`, `ActionId`, `AppealId`, `AppealEventId`,
    `DmId`, `EvidenceId`, `FileId`, `TicketId`, `MessageRowId`, `JobId`,
    `ModLogId` (`from_uuid`, `get`, `to_u22`, `from_u22`).
  - `fn to_u22(id: Uuid) -> String`, `fn from_u22(s: &str) -> Result<Uuid, IdError>`,
    `fn ulid_to_uuid(ulid: &str) -> Result<Uuid, IdError>`;
    `IdError::{NotASnowflake, NotU22, NotUlid}`.
- `limits` — every limit of §3.9 as named constants.
  - `CharLimit { min, max }` (`new`, `allows`), `fn char_count(&str) -> usize`,
    `fn truncate_chars(&str, max) -> &str`, `fn idempotency_key_ok(&str) -> bool`.
  - Constants: `RULE_NAME`, `RULE_DESCRIPTION`, `RULE_DESCRIPTION_IN_PICKER`,
    `RULE_REASON`, `MAX_FIELDS`, `FIELD_KEY`, `FIELD_LABEL`, `MIN_LEVELS`,
    `MAX_LEVELS`, `MAX_LEVEL_CASE_COUNT`, `MIN_TIMEOUT_SECONDS`,
    `MAX_TIMEOUT_SECONDS`, `MAX_BAN_DELETE_SECONDS`,
    `MIN_COUNTING_WINDOW_DAYS`, `MAX_COUNTING_WINDOW_DAYS`,
    `MAX_ACTIVE_RULES`, `CONTEXT_SHORT_TEXT`, `CONTEXT_LONG_TEXT`,
    `CONTEXT_DETAILS`, `CONTEXT_MESSAGE_LINK_MAX_CHARS`,
    `CONTEXT_NUMBER_MAX_ABS`, `CONTEXT_NUMBER_MAX_DECIMALS`,
    `MAX_CASE_NUMBER`, `VOID_REASON`, `APPEAL_TEXT`,
    `APPEAL_DECISION_REASON`, `APPEAL_POST_STATEMENT_CHARS`,
    `MAX_EVIDENCE_ITEMS`, `MAX_EVIDENCE_FILES`, `MAX_LINKS_PER_CALL`,
    `EVIDENCE_TEXT_CHARS`, `MAX_EVIDENCE_EMBEDS`,
    `EVIDENCE_EMBED_TITLE_CHARS`, `EVIDENCE_EMBED_DESCRIPTION_CHARS`,
    `MAX_FILES_PER_EVIDENCE`, `MAX_COPY_BYTES`, `MAX_STAFF_ROLES`,
    `DM_INTRO`, `DM_FOOTER`, `INVITE_CODE`, `HONEYPOT_WARNING`,
    `MIN_TICKET_RETENTION_DAYS`, `MAX_TICKET_RETENTION_DAYS`,
    `DEFAULT_TICKET_RETENTION_DAYS`, `MAX_TRANSCRIPT_BYTES`,
    `IDEMPOTENCY_KEY`, `DEFAULT_PAGE_SIZE`, `MAX_PAGE_SIZE`,
    `DISCORD_PAGE_SIZE`, `CASE_DRAFT_LIFETIME`, `NONCE_WINDOW`,
    `MAX_BODY_BYTES`, `MIN_STATS_RANGE`, `MAX_STATS_RANGE`.
- `messages` — tracked messages (§10): kinds, states, and the sync
  routing.
  - Types: `MessageKind::{CaseReceipt, AppealPost, TicketPanel, TicketPost,
    HoneypotWarning}` (`has_owner`), `MessageState::{Active, Gone}`
    (`mark_gone`), `SyncRow`, `SyncRoute::{EditOriginal, MarkGone, Wait,
    EditWithBot, Create}`, `MissingPlan::{Repost(..), WaitForChannel,
    BecomePrivate, MarkGone}`, `StaleCleanup::{EditToMoved, Delete}`.
  - `const fn sync_route(row: SyncRow) -> SyncRoute`,
    `const fn needs_sync(wanted_revision: u32, posted_revision: u32) -> bool`,
    `fn token_expires_at(received_at) -> OffsetDateTime`,
    `fn create_nonce(row: MessageRowId, generation: u32) -> String`,
    `const fn is_missing(code: Option<u32>) -> bool`,
    `fn sync_error_code(status: u16, code: Option<u32>) -> String`,
    `const fn on_missing(kind, current_channel: Option<ChannelId>, create_refused: bool, token_valid: bool) -> MissingPlan`,
    `const fn stale_cleanup(kind) -> StaleCleanup`.
  - Constants: `MAX_STALE` (5), `INTERACTION_TOKEN_LIFETIME`,
    `MAX_SYNC_LOOPS` (3), `SYNC_ATTEMPTS` (6), `SYNC_BACKOFF`.
- `modlog` — the append-only moderation log (§9): kinds, results, typed
  links, typed details, and legacy redaction.
  - Types: `ModLogKind` (one variant per §9 kind, `CaseCreated` …
    `Legacy`; `result`, `is_posted`), `ModLogResult::{Ok, Failed, Refused}`,
    `ModLogSource::{Discord, Dashboard, Quack, Imported}`, `ModLogEntry`
    (one variant per kind with its detail; `kind`, `detail`, `from_detail`,
    `result`, `is_posted`), detail structs `CaseCreatedDetail`,
    `CaseCreateRefusedDetail`, `CaseVoidedDetail`, `CaseContextEditedDetail`,
    `EvidenceAddedDetail`, `ActionDoneDetail`, `ActionFailedDetail`,
    `ActionDetail`, `UndoSkippedDetail`, `DmSentDetail`, `DmFailedDetail`
    (+ `DmFailedState::{Failed, Unknown}`), `CaseNumberDetail`,
    `AppealClosedDetail`, `RuleVersionDetail`, `RuleNameDetail`,
    `FieldsDetail`, `ModuleDetail`, `ModuleDisabledDetail`,
    `ModuleSettingsChangedDetail`, `QuackJoinedDetail`,
    `ChannelSettingClearedDetail`, `LogChannelFailedDetail`,
    `TicketClosedDetail`, `HoneypotCaseFailedDetail`, `LegacyDetail`,
    `Empty`; links `ModLogLinks` (`check`), `Link::{Case, Member, Appeal,
    Action, Rule, Ticket}`, `LinkRule::{Required, Optional, Absent}`,
    `LinkProblem::{Missing { .. }, Unexpected { .. }}`.
  - `fn redact(value: serde_json::Value) -> serde_json::Value`,
    `fn redact_failure_reason(&str) -> String`; constant `REDACTED`.
- `modules` — domain types of the optional modules (§13) and the channel
  settings they share.
  - Types: `ModuleName::{Tickets, ActivityLog, Honeypot}`,
    `ModuleDisabledReason::{ChannelDeleted, QuackLeft}`,
    `ModuleProblem::{TicketEntryMissing, TicketQueueMissing,
    TicketSameChannel, HoneypotChannelMissing, HoneypotRuleMissing,
    HoneypotRuleRequiredFields(..), ActivityNoRoute, ActivityNeedsAuditLog,
    ChannelPermissions { .. }, ChannelNotStaffOnly(..)}` (`code`),
    `ChannelSetting::{LogChannel, AppealChannel, EvidenceChannel,
    TicketEntryChannel, TicketQueueChannel, HoneypotChannel,
    ActivityLogRoute}`, `LogChannelFailure::{NotStaffOnly, DiscordRefused,
    RetriesExhausted}`, `ActivityEvent::{MessageEdited, MessageDeleted,
    MessagesBulkDeleted, MemberJoined, MemberLeft, MemberBanned,
    MemberUnbanned, ServerChanged, ChannelChanged}`,
    `TicketState::{Opening, Open, Closing, Closed}` (`is_live`,
    `transition`), `TicketEvent::{Opened, OpenFailed { .. }, CloseRequested,
    CloseFinished}`, `TicketCloseReason::{OpenFailed}`,
    `TranscriptPartial::{History, Trimmed}`, `HoneypotHitState::{Pending,
    CaseCreated, Failed}` (`settle`), `HoneypotRefusal::{Member(..),
    RuleUnavailable, PolicyChanged, DiscordUnavailable}`,
    `HoneypotFailure::{Refused(..), Exempt, ImportedPending}`.
  - `fn redact_secrets(text: &str) -> Cow<'_, str>`,
    `fn ticket_thread_name(id: TicketId) -> String`; constants
    `HONEYPOT_BURST_WINDOW` (30 s), `REDACTED_SECRET`.
- `permissions` — who may do what (§2): permission bits, capabilities,
  the operation table, member checks, and the staff-only channel check.
  - Types: `Permissions(u64)` (`NONE`, `contains`, `allows`, `missing`),
    `Permission` (17 Discord bits, `bit`), `DiscordPermission::{ModerateMembers,
    KickMembers, BanMembers}`, `Capability::{Moderate, ManageRules,
    ManageSettings, ManageStaffRoles}`, `Capabilities` (`holds`, `check`),
    `AccessRefusal::{MissingCapability(..), MfaRequired}`, `ActorFacts`,
    `StaffRoles` (`existing`), `Operation` (20 operations, `requirement`),
    `Requirement::{Capability(..), MemberSelf, Quack, ServerMember,
    TicketMemberOrModerator}`; facts `RoleFacts`, `GuildFacts`,
    `MemberFacts`, `ChannelFacts`, `BanFacts`, `AccountFacts`,
    `StaffChannelFacts`, `Overwrite`, `OverwriteTarget::{Role, Member}`;
    member checks `PresentMember`, `QuackFacts` (alias), `MemberSubject`,
    `ActorPresence::{Quack, Staff { .. }}`, `ApplyFacts`,
    `CheckPurpose::{ApplyRule, ApplyHoneypotRule, RetryAction(..)}`,
    `MemberRefusal::{QuackNotInServer, ActorNotInServer, MemberNotInServer,
    MemberIsSelf, MemberIsBot, MemberIsOwner, MemberRoleTooHigh,
    QuackRoleTooLow, QuackMissingPermission(..), MemberIsAdministrator}`,
    `MemberRefusalCode`, `ChannelProblem::{NotText, EveryoneCanView,
    RoleCanView(..), MemberCanView(..), QuackMissing(..), Inaccessible,
    Missing}`.
  - `fn derive_capabilities(actor: &ActorFacts, roles: &StaffRoles, mfa_required: bool) -> Capabilities`,
    `fn is_staff_for_access(actor: &ActorFacts, moderator_role_ids: &[RoleId]) -> bool`,
    `fn check_member(facts: &ApplyFacts, purpose: CheckPurpose) -> Result<(), MemberRefusal>`,
    `fn check_action_permission(facts: &ApplyFacts, kind: ActionKind) -> Result<(), MemberRefusal>`,
    `fn action_precheck(facts: &ApplyFacts, kind: ActionKind) -> Result<(), Failure>`,
    `fn check_staff_channel(channel: &StaffChannelFacts, moderator_role_ids: &[RoleId], quack_needs: Permissions) -> Vec<ChannelProblem>`;
    constant `TEXT_CHANNEL`.
- `rule` — the rule definition (§3.2), its validation (§3.3), versioning,
  the rule file (§3.6), and the starter and honeypot rules.
  - Types: `RuleDefinition` (`validate(Strictness)`, `trimmed`,
    `has_required_fields`, `check_honeypot_compatible`, `field`,
    `context_fields`), `Field`, `FieldKey` (`is_valid`, `as_str`),
    `FieldKind::{ShortText, LongText, Boolean, Number, MessageLink}`,
    `Level`, `LevelAction::{Timeout { .. }, Kick, Ban { .. }}` (`kind`),
    `Strictness::{New, Imported { reconstructed }}`, `RuleProblem` (one
    variant per §3.3 row; `code`, `path`), `RuleProblemCode`, `RuleFile`
    (`new`, `into_rule`), `RuleFileError::{WrongFormat,
    UnsupportedVersion(..), Invalid(..)}`.
  - `fn starter_rule() -> RuleDefinition`, `fn honeypot_rule() -> RuleDefinition`,
    `fn details_field() -> Field`,
    `fn next_version(current: u32, expected: u32) -> Result<u32, DomainError>`.
  - Constants: `RULE_FILE_FORMAT`, `RULE_FILE_FORMAT_VERSION`,
    `DETAILS_FIELD_KEY`, `DETAILS_FIELD_LABEL`, `HONEYPOT_RULE_NAME`.
- `settings` — validation of server and module settings values (§13.1,
  §13.2, §13.4) that needs no Discord facts.
  - `SettingsProblem::{TooManyRoles, DuplicateRole, EveryoneRole,
    UnknownRole, InviteUrl, TooLong, RetentionDays}`.
  - `fn check_role_list(guild, roles: &[RoleId], server_roles: &[RoleId], saved: &[RoleId]) -> Result<(), SettingsProblem>`,
    `fn normalize_invite(url: &str) -> Result<Option<String>, SettingsProblem>`,
    `fn normalize_optional_text(text: &str, limit: CharLimit) -> Result<Option<String>, SettingsProblem>`,
    `fn check_retention_days(days: u16) -> Result<(), SettingsProblem>`.

#### quack-text

No error type: message functions are total; `budget::check_msg` /
`check_modal` return problem strings for tests.

Root re-exports: `Msg`, `Text`, `Inline`, `Block`, `Row`, `Button`,
`Select`, `Modal`, `Tone`, `Person`, `Actor`, `ChannelRef`, `RoleRef`,
`Command`, `TimeStyle` (from `model`); `ButtonAction`, `SelectAction`,
`ModalAction` (from `actions`); `Surface`, `CommandIds` (from `render`).

- `model` — the neutral message model. `Msg` (`new`, `text`, `block`,
  `para`, `buttons`, `select`, `plain`), `Tone::{Neutral, Success, Warning,
  Danger}`, `Block::{Heading, Text, Quote, Field { .. }, Subtext, Divider,
  File}`, `Row::{Buttons, Select}`, `Button` (`new`, `link`),
  `ButtonStyle::{Primary, Secondary, Success, Danger}`,
  `ButtonTarget::{Link, Action}`, `Select`, `SelectOption`,
  `SelectOptions::{Strings, TextChannels}`, `Modal`, `Input`,
  `InputStyle::{Short, Paragraph}`, `InputKind::{Text { .. }, YesNo { .. }}`,
  `Text(Vec<Inline>)` (builder: `new`, `fixed`, `s`, `user`, `bold`,
  `bold_user`, `push`, `then`, `sentence`, `person`, `actor`, `channel`,
  `role`, `command`, `time`, `link`, `plain`), `Inline::{Fixed, User,
  ServerMarkdown, Bold, Code, Link { .. }, Person, Channel, Role, Command,
  Time}`, `Person` (`new`, `unnamed`), `Actor::{Quack, Person}`,
  `ChannelRef`, `RoleRef`, `Command::{CaseAdd, CaseView, CaseList, CaseVoid,
  CaseHistory, Appeals, Rules, Setup, Help}` (`top`, `path`),
  `TimeStyle::{Full, Relative, FullRelative, Date}`.
- `actions` — what a button, select, or modal does, as typed values (ids,
  never authority). `ButtonAction` (25 variants, `OpenCase` …
  `FixTicketAccess`), `SelectAction::{OpenCase, PickRule, Setup}`,
  `SelectValue::{Case, Rule, Yes, No}`, `ModalAction::{EditContext { .. },
  AddEvidence, VoidCase, DraftPage, Appeal, AcceptAppeal { .. },
  RejectAppeal { .. }, SetupInvite, SetupWarning}`, `InputId::{Field,
  Reason, Links, Statement, Invite, Warning}`, `SetupArea::{Home, Log,
  Appeals, Evidence, Tickets, Honeypot, ActivityLog}`, `SetupOp` (13 ops),
  `DraftPage`.
- `render` — rendering to Discord Markdown or plain text.
  `Surface::{Discord(&CommandIds), Plain}`, `CommandIds` (`EMPTY`);
  `text(&Text, Surface) -> String`, `plain(&Text)`,
  `markdown(&Text, &CommandIds)`, `block_markdown`, `plain_msg(&Msg)`,
  `msg_markdown(&Msg, &CommandIds)`.
- `budget` — Discord's limits, a checker, and trimming to the text budget
  (§11.11). Constants `MODAL_TITLE`, `BUTTON_LABEL`, `BUTTONS_PER_ROW`,
  `SELECT_OPTION_LABEL`, `SELECT_OPTION_DESCRIPTION`, `SELECT_PLACEHOLDER`,
  `SELECT_OPTIONS`, `INPUT_LABEL`, `INPUT_DESCRIPTION`, `INPUT_PLACEHOLDER`,
  `INPUT_MAX_LENGTH`, `MODAL_INPUTS`, `COMMAND_NAME`, `COMMAND_DESCRIPTION`,
  `CHOICE_NAME`, `CHANNEL_NAME`, `CHANNEL_TOPIC`, `AUDIT_LOG_REASON`,
  `LINK_URL`, `TEXT_BUDGET`, `COMPONENT_BUDGET`; `text_len`,
  `component_count`, `check_msg`, `check_modal`, `cut_text`, `fit(&mut Msg)`.
- `fmt` — shared formatting: `clean`, `one_line`, `escape_markdown`,
  `defuse_mentions`, `safe_url`, `cut`, `count`, `plural`, `case_ref`,
  `level`, `list`, `duration(seconds: u64) -> String`, `last`, `date`,
  `datetime`, `transcript_time`, `lower_first`, `upper_first`.
- `word` — shared words (§12.2): `action`, `action_noun`,
  `Outcome::{InProgress, Done, Failed, CouldntConfirm, NotNeeded, Cancelled,
  LeftInPlace}`, `outcome`, `yes_no`, `VOIDED`, `IMPORTED`, `page`, `pager`,
  `open_case_button`, `dashboard_button`, `failed_actions_button`,
  `CaseOption`, `open_case_select`, `render_trimmed`, `level_outcome`,
  `level_summary_item`, `levels_summary`.
- `command` — command, option, and context-menu names (§11.1, §12.1):
  `Named`, `Choice`, constants `CASE`, `CASE_ADD`, `CASE_ADD_MEMBER`,
  `CASE_ADD_RULE`, `CASE_ADD_PRIVATE`, `CASE_VIEW`, `CASE_NUMBER`,
  `CASE_LIST`, `CASE_LIST_MEMBER`, `CASE_LIST_RULE`, `CASE_VOID`,
  `CASE_VOID_REASON`, `CASE_HISTORY`, `CASE_HISTORY_MEMBER`, `APPEALS`,
  `RULES`, `SETUP`, `HELP`, `HELP_TOPIC`, `HELP_TOPICS`, `ADD_CASE`, `ALL`.
- `error` — errors and refusals (§12.3): `missing_capability`,
  `mfa_required`, `actor_not_in_server`, `quack_not_in_server`,
  `discord_unavailable`, `internal`, `unconfirmed`, `render_failed`,
  `case_not_found`, `action_not_found`, `context_changed`, `rule_changed`,
  `rule_not_found`, `rule_archived`, `case_already_voided`, `case_voided`,
  `action_not_retryable`, `action_not_dismissable`, `action_not_undoable`,
  `timeout_window_passed`, `appeal_not_found`, `appeal_changed`,
  `evidence_limit_reached`, `too_many_links`, `replacement_invalid`,
  `honeypot_policy_changed`, `busy`, `expired_button`, `legacy_button`;
  `refusal`, `refusal_reason`; `for_domain(&DomainError, &ErrorFacts) ->
  Text`, `ErrorFacts`, `Scene::{Staff, AddingCase, EditingRule, Member}`.
- `case` — case messages (§12.4–§12.6, §12.12): `CaseCard`, `CardAction`,
  `EvidenceSummary`, `Voided`, `Imported`, `card`; `ReceiptView`,
  `receipt`, `receipt_posted`, `receipt_post_failed`, `receipt_replayed`,
  `saved_unconfirmed`; adding: `PickerRule`, `FormField`, `AlreadySaved::{Continue,
  Case}`, `picker`, `picker_no_rules`, `picker_fill_form`, `form_title`,
  `form_modal`, `draft_page_saved`, `draft_page_invalid`,
  `draft_page_already_saved`, `draft_expired`, `draft_rule_changed`,
  `draft_save_failed`, `context_problem`; editing: `context_modal`,
  `context_saved`, `context_too_many_fields`; voiding: `VoidResult`,
  `void_modal`, `void_done`, `void_reason_appeal_accepted`; lists:
  `CaseRow`, `ListFilter`, `ListPage`, `HistoryTotals`, `list`, `history`;
  `ContextLine`, `context_value`, `member_link_saved`, `CONTEXT_CUT`,
  `DETAILS_LABEL`.
- `action` — action sentences (§12.6): `ActionView`, `Detail::{Full,
  Receipt}`, `outcome`, `none`, `retry_scheduled`, `unconfirmed`,
  `dismissed_suffix`, `permission`, `failure_reason`, `sentence`,
  `member_none`, `member_sentence`; controls `Undo::{RemoveTimeout, Unban}`,
  `undo_from_original`, `retry_done`, `dismiss_done`, `undo_confirm`,
  `undo_button`, `undo_queued`.
- `dm` — DM outcome sentences (§12.7) and DMs (§12.8): `DmView`, `outcome`,
  `because`, `sentence`, `member_sentence`, `CaseDm`, `case`, `RejoinAfter::{Ban,
  Kick}`, `AppealDmKind`, `AppealDm`, `appeal`.
- `evidence` — §12.6 evidence: `modal`, `added`, `failure`, `state_text`,
  `file`, `EvidenceItem`, `view`, `view_none`.
- `appeal` — §12.9: `form`, `submitted`, `submit_exists`,
  `submit_not_allowed`, `submit_voided`, `submit_not_yours`,
  `information_saved`, `information_not_requested`, `information_outdated`,
  `AppealPost`, `post`, `post_moved`, `ModalDecision::{Accept, Reject}`,
  `decision_modal`, `decided`, `closed_by_void`, `event`, `AppealRow`,
  `list`.
- `log` — moderation log entries (§12.10): `LogFacts`, `text`, `post`,
  `lead`, `quote`, `setting_name`, `module_setting_name`, `fields`.
- `setup` — `/setup` (§11.8, §12.11): `SetupState`, `ActivityRoutes::{None,
  One, Several}`, `home`, `area`, `module_name`; `SetupResult` (25
  results), `result`, `create_failed_permission`, `create_failed_discord`,
  `create_unconfirmed`, `channel_problem`,
  `quack_missing_channel_permissions`, `channel_not_staff_only`,
  `module_problem`, `module_not_ready`, `invite_invalid`,
  `settings_problem`; `ChannelKind`, `channel_name`, `channel_topic`,
  `first_message`, `invite_modal`, `warning_modal`.
- `help` — `Topic::{Cases, Rules, Appeals, Setup}`, `help`.
- `rules` — `RuleRow`, `list`, `problem(RuleProblemCode) -> Text`.
- `ticket` — §12.13 tickets: `panel`, `panel_moved`, `open_opening`,
  `open_exists`, `open_closing`, `open_disabled`, `open_failed`, `welcome`,
  `close_button`, `TranscriptPlace::{NotYet, Attached, Dashboard, Expired}`,
  `TicketPost`, `post`, `close_closing`, `close_not_open`,
  `close_not_allowed`, `fix_done`, `fix_failed`, `closed_dm`,
  `thread_name`, `transcript_header`, `transcript_line`,
  `transcript_file_line`, `transcript_partial`.
- `activity` — activity log posts (§12.13): `ActivityPost` (13 variants),
  `MessageFacts`, `FileLink`, `Content::{Text, Empty, Unknown, Hidden}`,
  `post`, `text_hidden`, `BULK_ROWS`, `MAX_IDS`.
- `honeypot` — `Warning::{Off, On { .. }}`, `warning`, `outcome`.
- `api` — API-only messages (§12.14): `ApiCode` (20 transport codes;
  `code`, `message`), `sign_in_required`, `session_expired`,
  `origin_rejected`, `csrf_rejected`, `not_found`, `server_not_found`,
  `file_unavailable`, `validation_failed`, `invalid_json`,
  `body_too_large`, `idempotency_key_required`, `idempotency_key_reused`,
  `rate_limited`, `rule_changed`, `rule_name_taken`,
  `rule_used_by_honeypot`, `required_field_on_honeypot_rule`,
  `too_many_rules`, `settings_changed`, `appeal_exists`,
  `appeal_not_allowed`, `module_not_ready`, `unavailable`,
  `internal_error`, `sign_in_failed`, `sign_in_denied`.
- `timeline` — member timeline (§4.11): `case_added`, `case_voided`.

#### quack-telemetry

Error: `TelemetryError::{InvalidLogFilter { filter, message }, Exporter {
signal, message }, SubscriberAlreadySet(String), Shutdown(Vec<String>),
ShutdownTimedOut(Duration)}`.

- Root (private modules re-exported):
  - `fn init(config: &TelemetryConfig) -> Result<TelemetryGuard, TelemetryError>`
    — installs the subscriber, the W3C propagator, OTLP/HTTP export when
    `otlp_endpoint` is set, and builds the global `metrics()`.
  - `fn init_with_exporters<S, M, L>(config: &TelemetryConfig, exporters: Exporters<S, M, L>) -> Result<TelemetryGuard, TelemetryError>`
    (`S: SpanExport`, `M: PushMetricExporter`, `L: LogExport`);
    `Exporters { spans, metrics, logs }`.
  - `TelemetryConfig { service_name, service_version, service_instance_id,
    environment, otlp_endpoint, log_filter, log_format }` (`new`),
    `LogFormat::{Text, Json}`, `SHUTDOWN_DEADLINE` (10 s).
  - `TelemetryGuard` (`is_exporting`, `with_deadline(Duration) -> Self`,
    `force_flush() -> Result<(), TelemetryError>`,
    `shutdown(self) -> Result<(), TelemetryError>`).
  - `fn metrics() -> &'static Metrics`; `Metrics { interactions, cases,
    actions, dms, appeals, evidence, modlog, messages, jobs, discord,
    gateway, http, db, caches, queues, tickets, activity_log, honeypot,
    runtime }` (`new(&Meter, version)`), `SCOPE`; families
    `InteractionMetrics`, `CaseMetrics`, `ActionMetrics`, `DmMetrics`,
    `AppealMetrics`, `EvidenceMetrics`, `ModLogMetrics`,
    `TrackedMessageMetrics`, `JobMetrics`, `DiscordMetrics`,
    `GatewayMetrics`, `HttpMetrics` (+ `InFlightRequest`), `DbMetrics`,
    `CacheMetrics`, `QueueMetrics`, `TicketMetrics`, `ActivityLogMetrics`,
    `HoneypotMetrics`, `RuntimeMetrics`; observed-gauge samples
    `JobQueueSample`, `PoolSample` (installed with
    `JobMetrics::observe_backlog`, `DbMetrics::observe_pools`, …).
  - `fn current_trace_context() -> Option<String>`,
    `fn trace_context_of(span: &Span) -> Option<String>`,
    `fn parse_trace_context(traceparent: &str) -> Option<SpanContext>`.
  - Re-export `OpenTelemetrySpanExt` (from `tracing-opentelemetry`).
- `catalog` — the static instrument catalog that drives instruments,
  views, and `METRICS.md`: `InstrumentKind::{Counter, UpDownCounter,
  Histogram, Gauge, ObservableCounter, ObservableUpDownCounter}`,
  `InstrumentSpec`, `AttributeSpec`, `INSTRUMENTS`, `ATTRIBUTES`,
  `find(name) -> Option<&InstrumentSpec>`, `view(&Instrument) ->
  Option<Stream>`, `render_markdown() -> String`.
- `labels` — typed metric/span attributes (ids can never be labels):
  trait `Attribute` (`KEY`, `as_str`, `kv`); closed enums
  `InteractionKind`, `InteractionOutcome`, `Outcome`, `Reason`,
  `ActionKind`, `CaseSource`, `AttemptOutcome`, `ActionState`, `VoidCause`,
  `DmKind`, `DmOutcome`, `AppealTransition`, `MessageKind`, `ModLogResult`,
  `JobKind`, `JobOutcome`, `RestOutcome`, `RateLimitScope`,
  `GatewayConnectionEvent`, `HttpMethod`, `SignInOutcome`, `DbOperation`,
  `DbErrorClass`, `TransactionOutcome`, `ConnectionState`, `CacheResult`,
  `ActivityStage`, `HoneypotOutcome`, `CpuMode` (each with `ALL`,
  `VALUES`); static-name labels `CommandName`, `HttpRoute`, `UrlTemplate`,
  `GatewayEvent`, `ModLogKind`, `DbCollection`, `PoolName`, `CacheName`,
  `QueueName`, `RateLimitClass`, `ActivityEvent` (`new(&'static str)`).
  These mirror, but do not import, quack-core's types.
- `keys` — attribute keys without a typed label: `HTTP_RESPONSE_STATUS_CODE`,
  `DB_SYSTEM_NAME`, `SERVICE_VERSION`, `GUILD_ID`, `ACTOR_ID`, `JOB_ID`,
  `JOB_ATTEMPT`, `JOB_MAX_ATTEMPTS`, `INTERACTION_ID`, `JOB_QUEUE_WAIT`,
  `RATE_LIMIT_WAIT`.
- `spans` — one span constructor per unit of work (§18.4):
  `interaction(kind, command, guild_id, actor_id, interaction_id) -> Span`,
  `gateway(event, guild_id)`, `http(method, route)`,
  `job<Id: Display>(run: &JobRun<'_, Id>) -> Span` (new root linked to
  `JobRun::enqueued_by`), `JobRun { kind, job_id, attempt, max_attempts,
  queue_wait, guild_id, enqueued_by }`, `use_case(name, guild_id)`,
  `db(operation, table)`, `discord(method, route)`; recorders
  `record<A: Attribute>`, `record_status`, `record_rate_limit_wait`,
  `record_error`, `event(span, SpanEvent, attributes)`;
  `SpanEvent::{Acked, Replied, Refused, CaseCreated, CaseVoided,
  ActionAttempt, DmOutcome, AppealTransition, EvidenceCaptured,
  MessageSynced, ModLogEntry, RateLimited}`.

#### quack-db

Errors at the root: `DbError::{NotFound, Conflict(Conflict, String),
Stale, Contention(DbErrorClass, sqlx::Error), Unavailable(DbErrorClass,
sqlx::Error), Rejected { violation, constraint }, Invalid(String),
Decode(String), Migrate(MigrateError), Other(sqlx::Error)}` (`class`,
`is_contention`, `conflict`); `Conflict::{OperationKeyUsed, RuleNameTaken,
CaseNumberTaken, CaseAlreadyReplaced, ActionExists, AttemptOpen,
EvidenceExists, AppealExists, DmExists, TrackedMessageExists,
LiveTicketExists, ThreadTaken, AlreadyExists, Other}` (`from_constraint`);
`Violation::{Check, ForeignKey, NotNull, AppendOnly}`; trait `Applied`
(`bool::or_stale() -> Result<(), DbError>`, `false` → `Stale`);
`MigrationMismatch::{NotMigrated, Pending(Vec<i64>), Unknown(Vec<i64>),
Changed(i64)}`. Query functions are `async fn(conn: &mut PgConnection, …)
-> Result<_, DbError>` unless noted; `PgConnection` is re-exported and
`Tx` derefs to it.

- Root: pool, transactions, migrations, fence (private modules
  `db`, `migrate`, `fence`, `pg`, `error` re-exported).
  - `Db` (cheap clone): `async fn connect(url: &str, max_connections: u32) -> Result<Db, DbError>`,
    `async fn connect_with(options: PgConnectOptions, max_connections: u32) -> Result<Db, DbError>`,
    `fn pool(&self) -> &PgPool`, `fn connect_options(&self) -> &PgConnectOptions`,
    `async fn conn(&self) -> Result<PoolConnection<Postgres>, DbError>`,
    `async fn begin(&self) -> Result<Tx, DbError>` (READ COMMITTED),
    `async fn transaction<T, E: TxError, F>(&self, work: F) -> Result<T, E>`
    where `F: for<'t> FnMut(&'t mut Tx) -> Pin<Box<dyn Future<Output = Result<T, E>> + Send + 't>>`
    (reruns up to `CONTENTION_RETRIES` on serialization failure/deadlock),
    `async fn close(&self)`.
  - `Tx` (`commit`, `rollback`, `set_timeouts(statement, lock)`); trait
    `TxError: From<DbError>` (`db_error`).
  - Constants `ACQUIRE_TIMEOUT` (5 s), `STATEMENT_TIMEOUT` (5 s),
    `IDLE_IN_TRANSACTION_TIMEOUT` (60 s), `CONTENTION_RETRIES` (3).
  - `static MIGRATOR: Migrator`, `MigrationInfo { version, description }`,
    `async fn migrate(db: &Db) -> Result<Vec<MigrationInfo>, DbError>`,
    `async fn verify_migrations(db: &Db) -> Result<Result<(), MigrationMismatch>, DbError>`.
  - `Fence` (session advisory lock on its own connection):
    `async fn try_acquire(options: &PgConnectOptions) -> Result<Option<Fence>, DbError>`,
    `async fn check(&mut self) -> bool`, `async fn lost(&mut self)`,
    `async fn release(self) -> Result<(), DbError>`; `FENCE_KEY`
    (`0x517561636B`), `FENCE_CHECK_INTERVAL` (5 s).
  - `TimeCursor { created_at, id }` (keyset cursor, newest first).
- `jobs` — the job queue (§16): keyed rows inserted in the change's
  transaction, claim/fence/settle, and the generic runner.
  - Types: `JobKind::{RunAction, SendDm, SyncMessage, PostModLog,
    CaptureEvidence, HoneypotCase, HoneypotCleanup, OpenTicket, CloseTicket,
    Housekeeping}` (`ALL`, `as_str`, `policy`), `JobPolicy { concurrency,
    lease, max_attempts }` (`deadline`, `backoff`), `DEADLINE_MARGIN`
    (15 s), `JobKey` (`as_str`; built only by `NewJob`'s constructors),
    `RunAt::{Now, In(Duration), At(OffsetDateTime)}`, `NewJob`
    (`run_action`, `send_dm`, `sync_message`, `post_mod_log`,
    `capture_evidence`, `honeypot_case`, `honeypot_cleanup`, `open_ticket`,
    `close_ticket`, `housekeeping`, `at`, `kind`, `key`), `ClaimedJob { id,
    kind, key, guild, payload, attempt, max_attempts, claim_token,
    locked_until, trace_context, run_at, created_at }` (`payload<T>`,
    `budget_spent`, `is_last_attempt`, `queue_wait`), `JobResult::{Done,
    Retry { at, error }, Defer { at }, Continue, Dead { error },
    Reschedule { at }}`, `JobBacklog { kind, ready, oldest_ready_age, dead }`
    (`sample() -> quack_telemetry::JobQueueSample`).
  - `async fn enqueue(tx: &mut Tx, job: &NewJob) -> Result<JobId, DbError>`
  - `async fn ensure_housekeeping(conn: &mut PgConnection) -> Result<(), DbError>`
  - `async fn claim(conn: &mut PgConnection, kind: JobKind, limit: usize) -> Result<Vec<ClaimedJob>, DbError>`
  - `async fn sweep_leases(conn: &mut PgConnection, kind: JobKind) -> Result<u64, DbError>`
  - `async fn fence(conn: &mut PgConnection, job: &ClaimedJob) -> Result<(), DbError>`
  - `async fn settle(conn: &mut PgConnection, job: &ClaimedJob, result: &JobResult) -> Result<(), DbError>`
  - `async fn wake(conn: &mut PgConnection, job: &NewJob) -> Result<bool, DbError>`
  - `async fn next_due_in(conn: &mut PgConnection, kind: JobKind) -> Result<Option<Duration>, DbError>`
  - `async fn backlog(conn: &mut PgConnection) -> Result<Vec<JobBacklog>, DbError>`
  - **Runner (lives here, not in quack-engine).** `Runner` (cheap clone):
    `fn new(db: Db, config: RunnerConfig, handler: Handler) -> Runner`,
    `fn start(&self)` (one dispatcher per kind), `fn wake(&self, kind: JobKind)`,
    `fn wake_all(&self)`, `fn on_notification(&self, notification: Notification)`,
    `fn heartbeats_fresh(&self, max_age: Duration) -> bool`,
    `async fn shutdown(&self, drain: Duration)`.
    `RunnerConfig { concurrency, kinds, poll, claim_batch }`
    (`new(concurrency)`: every kind, 2 s poll, batch 8).
    The engine supplies one handler for every kind:
    `type Handler = Arc<dyn Fn(ClaimedJob, Phase) -> HandlerFuture + Send + Sync>`,
    `type HandlerFuture = Pin<Box<dyn Future<Output = Result<Handled, DbError>> + Send>>`,
    `Phase::{Run, Finalize}` (finalize when the budget is spent or a
    `Retry` came on the last attempt), `Handled::{Settled(JobResult),
    Settle(JobResult)}` (already settled in the handler's own transaction,
    or the runner settles it fenced). `Err(DbError::Stale)` = lost lease
    (nothing settled); any other `Err` is retried with the kind's backoff.
    The runner owns claiming, permits, deadlines, spans, and metrics, not
    payloads; it does not own the `LISTEN` connection.
- `notify` — `NOTIFY` channels and the listener.
  - Constants `JOBS_CHANNEL` (`quack_jobs`), `RULES_CHANNEL`
    (`quack_rules`), `SETTINGS_CHANNEL` (`quack_settings`).
  - `async fn rules_changed(conn: &mut PgConnection, guild: GuildId) -> Result<(), DbError>`
  - `async fn settings_changed(conn: &mut PgConnection, guild: GuildId) -> Result<(), DbError>`
  - `Notification::{Job(JobKind), Rules(GuildId), Settings(GuildId),
    Reconnected}`.
  - `Listener` (one dedicated connection outside the pool, listening on
    all three channels): `async fn connect(options: &PgConnectOptions) -> Result<Listener, DbError>`,
    `async fn recv(&mut self) -> Result<Notification, DbError>` (reconnects
    itself and then yields `Reconnected`). The caller loops on `recv` and
    routes jobs to `Runner::on_notification` and `Rules`/`Settings`/
    `Reconnected` to its caches.
- `accounts` — dashboard accounts and their 2FA state: `Account`; `upsert`,
  `get`.
- `actions` — actions, attempts, and the member-serialized section (§5):
  `Action`, `NewAction`, `Attempt`, `ActionListItem`; `insert`, `get`,
  `lock`, `lock_original`, `for_case`, `undo_of`; attempts `start`,
  `finish_attempt`, `open_attempt`, `attempts`, `attempts_for_case`; state
  (CAS) `lock_member`, `member_running`, `guard`, `voided_ban_case`,
  `mark_done`, `mark_failed`, `mark_cancelled`, `mark_skipped`,
  `back_to_pending`, `retry`, `dismiss`, `cancel_pending_original`; lists
  `list`, `count_failed`, `failed_by_kind`.
- `activity_log` — activity log settings and routes (§13.3):
  `ActivityLogSettings`, `ActivityFlags`; `settings`, `update_settings`,
  `remove_channel`.
- `appeals` — appeals and their event timeline (§8): `Appeal`,
  `AppealEvent`, `Decision`, `AppealSummary`; `insert`, `insert_event`,
  `get`, `for_case`, `lock_for_case`, `transition`, `events`, `event`,
  `list`, `pending`, `count_pending`, `count_open`.
- `cases` — cases (§4): `Case` (`is_valid`), `CaseVoid`, `NewCase`,
  `CaseRef`, `CaseFilter`, `CaseSummary`, `MemberCaseSummary`,
  `RuleTally`; `insert`, `count_counted`, `void`, `set_context`,
  `replaced_by`, `by_id_ref`, `count_honeypot`, `get`, `get_by_number`,
  `get_for_member`, `lock`, `find_by_operation_key`, `list`,
  `member_cases`, `member_summary`.
- `dms` — DMs (§6): `Dm`, `DmOwner::{Case(CaseId), AppealUpdate(CaseId,
  AppealEventId), TicketClosed(TicketId)}`; `insert`, `get`, `lock`,
  `for_case`, `start_sending`, `mark_sent`, `mark_failed`, `mark_unknown`,
  `back_to_pending`, `cancel_pending_case_dm`, `mark_not_needed`.
- `drafts` — multi-page `/case add` drafts (§11.6): `Draft`, `NewDraft`,
  `DRAFT_TTL`; `create`, `lock`, `save_page`, `set_rule`, `delete`.
- `evidence` — snapshots and file copies (§7): `Evidence`
  (`snapshot_stored`), `EvidenceFile`, `EmbedSummary`, `Snapshot`,
  `NewFile`, `Admission::{Added(EvidenceId), Recapture(EvidenceId),
  Already(EvidenceId, EvidenceState)}`; `admit`, `count_for_case`,
  `count_files_for_case`, `get`, `lock`, `for_case`, `save_snapshot`,
  `files_for_case`, `pending_files`, `file`, `start_copy`, `mark_copied`,
  `set_file_state`, `fail_pending_files`, `mark_saved`, `mark_failed`.
- `guilds` — servers and their child rows: `Guild`, `GuildFacts` (the
  `GUILD_CREATE` facts; distinct from `quack_core::permissions::GuildFacts`),
  `Upserted { new, returned }`; `upsert`, `ensure_children`, `mark_left`,
  `get`, `get_many`, `installed`, `next_case_number`.
- `honeypot` — honeypot settings and hits (§13.4): `HoneypotSettings`,
  `Hit`; `settings`, `lock_settings`, `update_settings`, `clear_channel`,
  `insert_hit`, `pending_batch`, `burst_case`, `mark_case_created`,
  `mark_failed`, `mark_cleanup`, `hit_counts`.
- `housekeeping` — periodic deletes of expired rows (§16.6): `Report`,
  `BATCH` (1,000); `run(conn, key_fingerprint: &[u8; 8], budget: Duration)
  -> Result<Report, DbError>`, `pass(conn, key_fingerprint)`.
- `import` — importer bookkeeping (§19): `ImportMode::{DryRun, Apply}`;
  `start_run`, `finish_run`, `imported_from`, `mark_imported`,
  `raise_case_counter`.
- `messages` — tracked messages (§10): `Owner::{Case(CaseId),
  Appeal(AppealId), Ticket(TicketId), Server(MessageKind)}` (`kind`),
  `Location`, `TrackedMessage`, `NewTrackedMessage`; `insert(tx: &mut Tx,
  …)`, `bump(tx: &mut Tx, guild: GuildId, owner: Owner) ->
  Result<Option<MessageRowId>, DbError>`, `get`, `lock`, `by_owner`,
  `by_location`; sync writes (CAS on generation) `start_create`,
  `record_posted`, `set_attachment`, `push_stale`, `remove_stale`,
  `mark_gone`, `set_error`, `make_private`, `clear_token`, and settings
  moves taking `&mut Tx`: `relocate`, `reactivate`,
  `bind_waiting_appeal_posts`, `move_open_appeal_posts`.
- `modlog` — the append-only moderation log (§9): `NewEntry`, `Written`,
  `StoredEntry`, `ModLogFilter`; `insert(tx: &mut Tx, new: &NewEntry) ->
  Result<Written, DbError>`, `get`, `list`, `for_case`,
  `last_log_channel_failure`.
- `operation_keys` — idempotency stored with the write (§4.3):
  `OperationKey` (`case_scoped`), `StoredOperation`, `OPERATION_KEY_TTL`;
  `reserve`, `complete`, `lookup`.
- `rules` — rules and immutable versions (§3.1): `Rule`, `RuleVersion`,
  `VersionInfo`, `RuleLock`, `NewRule`; `create`, `lock_for_share`,
  `lock_for_update`, `add_version`, `archive`, `restore`, `count_active`,
  `get`, `list`, `load_cache`, `version`, `versions`.
- `sessions` — dashboard sessions and OAuth states (§14.4): `NewSession`,
  `Session`, `OauthState`; `create`, `get`, `touch`, `delete`,
  `count_active`, `create_oauth_state`, `consume_oauth_state`.
- `settings` — core server settings (§13.1): `GuildSettings`,
  `SettingsPatch`; `get`, `lock`, `for_guilds`, `update`,
  `set_starter_rule`, `acknowledge_starter_notice`, `clear_channel`,
  `disable_modules_on_leave`, `set_module_enabled`.
- `stats` — server statistics (§14.3): `Statistics`;
  `compute(db: &Db, guild, from, to) -> Result<Statistics, DbError>` (its
  own REPEATABLE READ transaction).
- `tickets` — tickets (§13.2): `TicketSettings`, `Ticket`,
  `JournalMessage`, `Transcript`; `settings`, `lock_settings`,
  `update_settings`, `clear_channel`, `insert`, `get`, `lock`,
  `live_for_member`, `by_thread`, `list`, `count_open`; steps (CAS)
  `set_thread`, `begin_welcome`, `mark_welcome_posted`, `mark_open`,
  `fail_open`, `request_close`, `mark_frozen`, `mark_closed`,
  `mark_thread_deleted`; journal `add_journal_message`,
  `mark_journal_gaps`, `journal`, `save_transcript`, `transcript`.
  (`JournalFile` is public in a private module and not re-exported.)

### 17.3 The `Discord` port

```rust
/// Everything the engine and modules need from Discord. Implemented by the
/// binary's `DiscordPort` (over quack-discord) and by quack-testkit's `FakeDiscord`.
/// Every method borrows its arguments for the future's lifetime `'a`.
pub trait Discord: Send + Sync + 'static {
    // Facts (cache only under the freshness rules of §2, else REST; "absent" is never cached).
    fn guild_facts<'a>(&'a self, guild: GuildId) -> BoxFuture<'a, Outcome<GuildFacts>>;
    fn member<'a>(&'a self, guild: GuildId, member: MemberId, fresh: Fresh) -> BoxFuture<'a, Outcome<Membership>>;
    fn channel_facts<'a>(&'a self, guild: GuildId, channel: ChannelId) -> BoxFuture<'a, Outcome<Option<ChannelFacts>>>;
    fn guild_channels<'a>(&'a self, guild: GuildId) -> BoxFuture<'a, Outcome<Vec<ChannelFacts>>>;
    fn ban<'a>(&'a self, guild: GuildId, member: MemberId) -> BoxFuture<'a, Outcome<Option<BanFacts>>>;
    fn is_thread_member<'a>(&'a self, thread: ChannelId, account: AccountId) -> BoxFuture<'a, Outcome<bool>>;
    fn search_members<'a>(&'a self, guild: GuildId, query: &'a str, limit: u8) -> BoxFuture<'a, Outcome<Vec<MemberFacts>>>;
    fn accounts<'a>(&'a self, ids: &'a [AccountId]) -> BoxFuture<'a, Outcome<Vec<AccountFacts>>>;

    // Actions (§5.1). `reason` is the X-Audit-Log-Reason value.
    fn timeout<'a>(&'a self, guild: GuildId, member: MemberId, until: OffsetDateTime, reason: &'a str) -> BoxFuture<'a, Outcome<()>>;
    fn remove_timeout<'a>(&'a self, guild: GuildId, member: MemberId, reason: &'a str) -> BoxFuture<'a, Outcome<()>>;
    fn kick<'a>(&'a self, guild: GuildId, member: MemberId, reason: &'a str) -> BoxFuture<'a, Outcome<()>>;
    fn ban_member<'a>(&'a self, guild: GuildId, member: MemberId, delete_message_seconds: u32, reason: &'a str) -> BoxFuture<'a, Outcome<()>>;
    fn unban<'a>(&'a self, guild: GuildId, member: MemberId, reason: &'a str) -> BoxFuture<'a, Outcome<()>>;

    // Messages. Creates follow §10.4.
    fn open_dm<'a>(&'a self, account: AccountId) -> BoxFuture<'a, Outcome<ChannelId>>;
    fn send<'a>(&'a self, channel: ChannelId, msg: &'a Msg, files: &'a [OutgoingFile], nonce: &'a Nonce) -> BoxFuture<'a, Outcome<SentMessage>>;
    fn edit<'a>(&'a self, channel: ChannelId, message: MessageId, msg: &'a Msg, files: &'a FileEdit) -> BoxFuture<'a, Outcome<SentMessage>>;
    fn edit_interaction_original<'a>(&'a self, token: &'a InteractionToken, msg: &'a Msg) -> BoxFuture<'a, Outcome<()>>;
    fn delete_message<'a>(&'a self, channel: ChannelId, message: MessageId) -> BoxFuture<'a, Outcome<()>>;
    fn fetch_message<'a>(&'a self, channel: ChannelId, message: MessageId) -> BoxFuture<'a, Outcome<FetchedMessage>>;
    fn fetch_history<'a>(&'a self, channel: ChannelId, before: Option<MessageId>, limit: u8) -> BoxFuture<'a, Outcome<Vec<FetchedMessage>>>;

    // Files.
    fn download_attachment<'a>(&'a self, url: &'a Url, max_bytes: u64) -> BoxFuture<'a, Outcome<Bytes>>;

    // Channels and threads.
    fn create_channel<'a>(&'a self, guild: GuildId, spec: &'a ChannelSpec) -> BoxFuture<'a, Outcome<ChannelId>>;
    fn create_private_thread<'a>(&'a self, parent: ChannelId, name: &'a str) -> BoxFuture<'a, Outcome<ChannelId>>;
    fn find_threads<'a>(&'a self, guild: GuildId, parent: ChannelId, name: &'a str) -> BoxFuture<'a, Outcome<Vec<ChannelId>>>;
    fn add_thread_member<'a>(&'a self, thread: ChannelId, member: MemberId) -> BoxFuture<'a, Outcome<()>>;
    fn remove_thread_member<'a>(&'a self, thread: ChannelId, member: MemberId) -> BoxFuture<'a, Outcome<()>>;
    fn thread_members<'a>(&'a self, thread: ChannelId) -> BoxFuture<'a, Outcome<Vec<MemberId>>>;
    fn set_thread_state<'a>(&'a self, thread: ChannelId, archived: bool, locked: bool) -> BoxFuture<'a, Outcome<()>>;
    fn delete_channel<'a>(&'a self, channel: ChannelId) -> BoxFuture<'a, Outcome<()>>;
}

/// The typed result of one Discord call.
pub enum Outcome<T> {
    /// Discord confirmed it.
    Done(T),
    /// Discord definitely refused; nothing changed.
    Refused(Refusal),
    /// Rate limited before anything was sent; try again after the delay.
    RetryAfter(Duration),
    /// The request was never sent (connection refused, shutting down): nothing changed.
    NotSent(NotSentCause),
    /// The request may or may not have taken effect (deadline, 5xx, connection reset after sending).
    Unknown(UnknownCause),
}

/// A member lookup: `Absent` only for Discord's 404 Unknown Member.
pub enum Membership { Present(MemberFacts), Absent }

/// `Cached` allows the member cache under §2's rules; `Rest` always asks Discord (action reads, §5.1).
pub enum Fresh { Cached, Rest }

/// What a create or edit returned: where the message is and the attachment IDs Discord assigned.
pub struct SentMessage { pub channel_id: ChannelId, pub message_id: MessageId, pub attachments: Vec<(String, AttachmentId)> }

/// Files on an edit: keep the existing attachment, or replace it.
pub enum FileEdit { None, Keep(AttachmentId), Replace(OutgoingFile) }

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
pub enum NotSentCause { Connection, ShuttingDown }
```

Every REST call has a 10-second deadline (30 seconds for file downloads and
uploads), runs inside a `{method} {route}` client span, and records
`http.client.request.duration` and the rate-limit instruments (§18).
Twilight's rate limiter is used; when it reports a wait longer than 10
seconds the call returns `RetryAfter`. Twilight's own retries are off; the
per-kind policies decide.

`GuildFacts`, `MemberFacts` (roles, highest position, Administrator flag,
bot flag, `joined_at`, timeout end, display names), `ChannelFacts` (type,
parent, overwrites, Quack's computed permissions), `BanFacts` (reason), and
`AccountFacts` are plain structs in `quack-core::permissions`, because the
pure checks take them. `Outcome`, `Refusal`, `RefusalKind`, the two causes,
`Membership`, `Fresh`, `SentMessage`, `OutgoingFile`, `FileEdit`, and
`InteractionToken` are plain types in `quack-core::discord`, shared by
`quack-discord` and the port so the adapter passes them through unchanged.
`FetchedMessage` (author, content, timestamps, embeds, attachments with
IDs, server), `Nonce`, and `ChannelSpec` live in `quack-engine`, which
shapes them differently from the client. Dashboard OAuth calls (`/oauth2/token`, `/users/@me`,
`/users/@me/guilds`) are not part of this port: `quack-api` makes them
with its own small client, traced the same way.

### 17.4 Dependency diagram

Drawn from the table in §17.2 (`A ──> B` means A depends on B):

```
quack (bin)          ──> every crate below
quack-bot            ──> quack-discord, quack-tickets, quack-activity-log, quack-honeypot, quack-engine, quack-db, quack-text, quack-core
quack-api            ──> quack-tickets, quack-activity-log, quack-honeypot, quack-engine, quack-db, quack-text, quack-core
quack-tickets,
quack-activity-log,
quack-honeypot       ──> quack-engine, quack-db, quack-text, quack-core
quack-import         ──> quack-db, quack-core
quack-discord        ──> quack-text, quack-core
quack-engine         ──> quack-db, quack-text, quack-core
quack-db             ──> quack-core
quack-text           ──> quack-core
quack-testkit (dev)  ──> quack-engine, quack-db, quack-core
quack-telemetry      (depends on nothing; used by every crate except quack-core, quack-text, quack-import)
```

### 17.5 Startup and shutdown

**Startup (`quack serve`):**

1. Parse CLI; load the TOML file (if given) and `QUACK_*` variables;
   validate every key, reporting all problems; exit 2 on any.
2. Initialize telemetry (§18.6). From here on, everything is traced.
3. Connect the pool (`QUACK_DATABASE_MAX_CONNECTIONS`, acquire timeout 5 s,
   and per connection `statement_timeout = 5s`,
   `idle_in_transaction_session_timeout = 60s` (a stuck task can never
   hold locks for long), `timezone = UTC`, and `plan_cache_mode =
   force_custom_plan` (list queries take optional filters written as
   `$n IS NULL OR …`; a custom plan per execution folds them so the
   filter's index is used). With `--migrate`, apply
   migrations; then verify they are current (else exit 1).
4. **Singleton fence.** On a dedicated connection outside the pool, take
   `pg_try_advisory_lock(<the constant 0x517561636B>)`. Not acquired (an
   older instance is still running during a rolling deploy) → retry every
   5 seconds, logging, with readiness false. If that connection is ever lost,
   the process stops all work and exits 1, so two instances never act at
   once.
5. Bind `QUACK_LISTEN_ADDR` and serve HTTP (`/livez` at once; `/readyz`
   false until step 10). API requests are served; their facts come from
   REST until the gateway has loaded the server (§2).
6. Build the Discord REST client; `GET /users/@me` to validate the token and
   learn Quack's own account ID; `GET /gateway/bot` (more than one shard
   recommended → exit 1, §11.9).
7. Sync commands (§11.2).
8. Connect the one `quack_db::notify::Listener` (a dedicated connection
   that listens on `quack_jobs`, `quack_rules`, and `quack_settings`), load
   the rules and config caches, and start the loop that routes
   `Rules`/`Settings` notifications to the caches (`Reconnected` reloads
   them).
9. Connect the gateway shard; start the event dispatch queues. When `READY`
   has arrived and every server in it has sent `GUILD_CREATE` (or 30
   seconds passed), start the job runner: ensure the `housekeeping` job
   (`jobs::ensure_housekeeping`), `Runner::start` (one dispatcher per
   kind), route `Job` and `Reconnected` notifications to
   `Runner::on_notification` from then on, and start the 60-second gauge
   task.
10. `/readyz` turns ready when the fence is held, the gateway session is
    `READY` or `RESUMED`, and every dispatcher has completed a pass.

**Server bootstrap** (`guilds::bootstrap`, on `GUILD_CREATE`, one
transaction):

1. Upsert `guilds` (name, icon, owner, MFA, `installed = true`,
   `left_at = NULL`).
2. Insert each child row that is missing, independently: `case_counters`,
   `guild_settings`, `ticket_settings`, `activity_log_settings`,
   `honeypot_settings` (`INSERT … ON CONFLICT DO NOTHING`). When this
   inserted `guild_settings` (a first join, not an imported server), also
   create the starter rule (§3.7) with version 1, set `starter_rule_id`, and
   log `quack_joined` (`first_join`).
3. Returning after leaving: log `quack_joined` (`first_join = false`).
4. Reconcile channel settings against the event's channel list (§13.1.5).

**Leaving a server** (`GUILD_DELETE` without `unavailable`, or missing from
`READY`, one transaction): `installed = false`, `left_at = now()`; every
enabled module is turned off with `disabled_reason = quack_left` (logged,
not posted), because its channels and settings may be stale when Quack
comes back; log `quack_left`; drop the server's caches. Settings and
history are kept. After a re-add, modules stay off until an administrator
turns them on.

**Shutdown** (SIGTERM or SIGINT). Every step shares one deadline, 30
seconds after the signal; set the platform's grace period to at least 35
seconds.

1. Mark not ready (`/readyz` → 503).
2. Stop admission: dispatchers stop claiming; the gateway stops handing out
   new events and interactions (Discord shows unacknowledged interactions
   as failed); HTTP stops accepting new connections.
3. Drain, concurrently and until 5 seconds before the deadline: in-flight
   interaction handlers, HTTP requests, the server activity log's delete
   batches and queue, and job handlers.
4. At that point the Discord client stops accepting new calls; remaining
   handlers are dropped (§16.7). Close the gateway with a normal close frame,
   close the pool, and close the fence connection.
5. Flush telemetry with the time left (`TelemetryGuard::with_deadline`,
   at most 5 seconds). Exit 0.

### 17.6 Caches and invalidation

- **Rules cache**: loaded for all installed servers at startup in one
  query, then per server on `NOTIFY quack_rules '<guild_id>'` (sent in every
  transaction that creates, edits, archives, restores, or imports a rule).
  Used by autocomplete, pickers, `/case add` modals, `/setup` rule select,
  and early validation; never to decide which version a case uses (§3.1).
- **Config cache**: `guild_settings` plus the three module settings rows
  per server; reloaded on `NOTIFY quack_settings '<guild_id>'`. Used by
  autocomplete capability checks, setup modals, honeypot filtering, activity
  routing.
- Every reload carries the rows' `version` (rules: `current_version` and
  `updated_at`), and a reload never replaces a newer entry, so a slow reload
  cannot undo a faster one. After the listener reconnects, both caches
  reload fully under the same rule.
- Caches never grant access by themselves for writes: every write re-checks
  capabilities from current facts (§2) and reads rows inside its
  transaction.

### 17.7 Configuration

Environment variables (`QUACK_*`) or an optional TOML file
(`--config <file>`) using the same names in lowercase without the prefix
(`database_url = "…"`). Environment variables win over the file. Unknown
keys in the file are an error. Secrets belong in the environment. Quack
never reads `.env` itself; the development commands export it (§20.2).

| Variable | Type | Default | Purpose |
| --- | --- | --- | --- |
| `QUACK_DATABASE_URL` | Postgres URL | required | Connection string. |
| `QUACK_DATABASE_MAX_CONNECTIONS` | integer 4–200 | 20 | Pool size for queries and transactions. The listener and the fence each use one more dedicated connection outside the pool; member locks are transaction-scoped and need none. |
| `QUACK_DISCORD_TOKEN` | secret | required for `serve`, `commands sync` | Bot token. |
| `QUACK_DISCORD_APPLICATION_ID` | snowflake | required for `serve`, `commands sync` | Application ID (also the OAuth client ID). |
| `QUACK_DISCORD_CLIENT_SECRET` | secret | required for `serve` | OAuth client secret. |
| `QUACK_PUBLIC_URL` | absolute URL, `https`, or `http` only for `localhost`, `127.0.0.1`, or `[::1]`; no credentials, path, query, or fragment | required for `serve` | Dashboard links, OAuth redirect (`<url>/api/auth/callback`), CSRF origin, cookie names and `Secure`. |
| `QUACK_LISTEN_ADDR` | socket address | `0.0.0.0:8080` | HTTP listen address. |
| `QUACK_SESSION_SECRET` | base64, exactly 32 bytes | required for `serve` | Encrypts Discord tokens stored in sessions. Rotating it signs everyone out (§14.4). |
| `QUACK_DASHBOARD_DIR` | path | `dashboard` | Directory with the built dashboard (`index.html`, `assets/`); checked at startup. |
| `QUACK_OTLP_ENDPOINT` | URL | unset | OTLP/HTTP base URL (`/v1/traces`, `/v1/metrics`, `/v1/logs` are appended). Unset: no export; logs still go to stderr. |
| `QUACK_SERVICE_NAME` | string | `quack` | `service.name`. |
| `QUACK_ENVIRONMENT` | string | `development` | `deployment.environment.name` resource attribute. |
| `QUACK_LOG_LEVEL` | `tracing` filter directive | `info` | Log filter (traces use their own fixed `info` filter, §18.5). |
| `QUACK_WORKER_CONCURRENCY` | integer 1–64 | 16 | Jobs running at once across all kinds. |
| `QUACK_DEV_GUILD_ID` | snowflake | unset | Register commands to this server only. |
| `QUACK_TRUSTED_PROXY_HOPS` | integer 0–5 | 0 | Proxies in front of Quack, for the client IP (§14.7). |
| `QUACK_IMPORT_SOURCE_URL` | secret MySQL URL | required for `import-mysql` only | The Go database to import (§19). Kept out of command lines and logs. |

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
0.33.1, `tracing` 0.1.44, and `tracing-opentelemetry` 0.34.0, all set up by
`crates/quack-telemetry`. Logs go through `tracing`.

### 18.1 Rules

- **Cardinality.** Metric attributes are small closed enums only (the
  crate's `labels`); server, account, channel, case, appeal, action,
  message, and job IDs never appear on metrics. They go on spans and logs.
  Labels documented as "static names chosen in code" (command, route, table,
  cache, queue, module-log kind) take values only from `&'static str`
  constants in Quack's code, never from input; a test feeds raw routes,
  malformed methods, and unknown event names and checks they map to a
  fallback value.
- **Catalog.** `crates/quack-telemetry/METRICS.md` is generated from the
  crate's catalog (`UPDATE_METRICS_DOC=1 cargo test -p quack-telemetry
  metrics_doc`) and is the **source of truth** for every instrument's name,
  type, unit, attributes, attribute values, and histogram buckets. This
  section does not repeat those; it says where each instrument is recorded
  (§18.2) and lists the changes the crate must make for this design
  (§18.3). After those changes, METRICS.md and this section agree, and a
  test checks that every instrument named here exists in the catalog.
- **Durations are seconds**, histograms included.
- **Failure isolation.** A slow or absent collector never blocks moderation:
  exporters use bounded batches and drop on overflow.

### 18.2 Where each instrument is recorded

| Instrument (METRICS.md) | Recorded in |
| --- | --- |
| `quack.interaction.ack.duration`, `quack.interaction.duration`, `quack.interaction.errors` | `quack-bot` router: on ack; after the final reply; on a refused or failed answer. |
| `quack.cases.created`, `quack.case.create.duration`, `quack.case.level` | engine `apply_rule`, after commit. |
| `quack.cases.refused` | engine `apply_rule`, on every refusal (Discord, dashboard, honeypot). |
| `quack.cases.voided` | engine `void_case`, after commit. |
| `quack.action.attempts`, `quack.action.retries`, `quack.actions.finished`, `quack.action.completion.duration`, `quack.action.undo.skipped` | engine `actions::run` and `actions::finalize`, in their settle. |
| `quack.actions.failed` | gauge task (failed, not dismissed). |
| `quack.dm.outcomes`, `quack.dm.send.duration` | engine `dms::send`, `dms::finalize`, and void (`not_needed`). |
| `quack.appeal.transitions`, `quack.appeal.decision.duration` | engine `appeals`, after commit. |
| `quack.appeals.open` | gauge task (`pending` plus `needs_information`). |
| `quack.evidence.captures`, `quack.evidence.files`, `quack.evidence.bytes` | engine `evidence::capture`. |
| `quack.modlog.entries` | `quack-db` `modlog::insert`, after commit. |
| `quack.modlog.posts` | engine `modlog::post`. |
| `quack.message.syncs`, `quack.message.sync.lag` | engine `messages::sync`. |
| `quack.jobs.enqueued` | `quack-db` `jobs::enqueue`, after commit. |
| `quack.jobs.started`, `quack.job.queue.wait`, `quack.job.duration`, `quack.job.lease_expired` | job runner. |
| `quack.jobs.ready`, `quack.jobs.oldest_ready.age`, `quack.jobs.dead` | gauge task, with the claim's predicates (§16.6). |
| `http.client.request.duration`, `quack.discord.rate_limit.hits`, `quack.discord.rate_limit.wait` | `quack-discord` client wrapper and `quack-api`'s OAuth client. One rate-limit observation per limiter wait; a 429 response is counted by the request's status. |
| `quack.gateway.event.duration`, `quack.gateway.heartbeat.latency`, `quack.gateway.connection.events`, `quack.gateway.connected`, `quack.gateway.guilds` | gateway loop and `quack-bot`. |
| `quack.queue.depth`, `quack.queue.dropped` | every bounded in-memory queue: the 4 event queues, the interaction pool, the activity post queue. |
| `quack.cache.size`, `quack.cache.lookups`, `quack.cache.evictions` | each cache of §11.10. |
| `http.server.request.duration`, `http.server.active_requests` | `quack-api` Tower layer (the in-flight count is decremented by a drop guard, so cancelled requests are counted). |
| `quack.auth.sign_ins`, `quack.http.rate_limited` | `quack-api`. |
| `quack.sessions.active` | gauge task. |
| `db.client.operation.duration`, `db.client.connection.*`, `quack.db.transaction.duration`, `quack.db.transaction.retries` | `quack-db` query wrapper and `Db::begin` guard (rollbacks included). A CTE is recorded by its business verb (`UPDATE` for the claim). |
| `quack.tickets.opened`, `quack.tickets.closed`, `quack.tickets.open`, `quack.ticket.transcript.size` | tickets module; `open` from the gauge task. |
| `quack.activity_log.events` | activity log module. |
| `quack.honeypot.hits`, `quack.honeypot.cleanups` | honeypot module. |
| `quack.runtime.*`, `process.*`, `quack.build.info` | telemetry crate, every 30 s. |

Mapping of domain values to the crate's generic labels:

| Domain value | Label value |
| --- | --- |
| `ActionFailure` | `quack.reason`: `quack_missing_permission` → `missing_permission`; `quack_role_too_low` → `role_hierarchy`; `member_is_owner` → `member_is_owner`; `member_not_in_server` → `member_not_found`; `quack_not_in_server`, `member_is_administrator` → `not_allowed`; `timeout_window_passed` → `invalid`; `discord_refused` → `discord`; `retries_exhausted` → `retries_exhausted`; `unconfirmed` → `timeout` |
| `DmFailure` | `quack.reason`: `dms_closed` → `dms_closed`; `not_in_shared_server` → `no_shared_server`; `discord_unavailable` → `retries_exhausted`; `discord_refused` → `discord` |
| `MemberRefusal`, missing capability, 2FA | `quack.reason`: `role_hierarchy` (role checks), `member_not_found`, `member_is_owner`, `missing_permission` (Quack's permission, capability), `not_allowed` (self, bot, administrator, 2FA) |
| Appeal events `submitted` / `information_requested` / `information_provided` / `accepted` / `rejected` / `closed` (staff or by void) / `reopened` | `quack.appeal.transition`: `submit` / `request_info` / `reply` / `accept` / `reject` / `close` / `reopen` |
| Honeypot: case created or burst joined / ignored (staff, bot, exempt) / failed (refusal, policy changed, Discord) | `quack.honeypot.outcome`: `actioned` / `exempt` / `failed`; dropped events are `quack.queue.dropped` |
| Log channel post: posted / moved, no channel / refused, exhausted / unknown after the window | `quack.outcome`: `done` / `skipped` / `failed` / `unknown` |
| Tracked message sync: edited or posted / gone / exhausted or refused | `quack.outcome`: `done` / `skipped` / `failed` |

### 18.3 Changes `quack-telemetry` must make

Each is old → new. METRICS.md is regenerated with them.

1. Span names (OTel semantic conventions): `http {method} {route}` →
   `{method} {route}`; `db {operation} {table}` → `{operation} {table}`;
   `discord {method} {template}` → `{method} {template}`. Unchanged:
   `interaction {kind} {command}`, `gateway {event}`, `job {kind}`.
2. `labels::JobKind`: `run_action, case_dm, appeal_dm, sync_message,
   post_log_entry, capture_evidence, honeypot_cleanup, ticket_transcript,
   housekeeping` → `run_action, send_dm, sync_message, post_mod_log,
   capture_evidence, honeypot_case, honeypot_cleanup, open_ticket,
   close_ticket, housekeeping`.
3. `labels::JobOutcome`: `done, retry, dead` → `done, retry, deferred,
   continued, rescheduled, finalized, dead, lost_lease, timed_out`.
4. `labels::CaseSource` (`quack.case.source`): `slash_command,
   message_command, user_command, dashboard, module` → `discord, dashboard,
   honeypot`. The interaction kind stays on the interaction instruments.
5. `labels::DmKind` (`quack.dm.kind`): `case, appeal` → `case,
   appeal_update, ticket_closed`.
6. `labels::MessageKind` (`quack.message.kind`): add `ticket_post`.
7. `labels::ActivityStage` (`quack.activity.stage`): `seen, posted,
   dropped` → `seen, posted, dropped, failed`.
8. `quack.action.attempts`: attribute `quack.outcome` → new
   `quack.action.attempt.outcome` (`done, no_change, refused, not_sent,
   unknown, interrupted`).
9. `quack.modlog.entries`: attribute `quack.outcome` → new
   `quack.modlog.result` (`ok, failed, refused`).
10. New instruments: `quack.job.lease_expired` (counter, `{job}`,
    `quack.job.kind`); `quack.jobs.dead` (observed gauge, `{job}`,
    `quack.job.kind`); `quack.actions.finished` (counter, `{action}`,
    `quack.action.kind`, new label `quack.action.state` with `done, failed,
    cancelled, skipped`); `quack.cases.refused` (counter, `{refusal}`,
    `quack.case.source`, `quack.reason`).
11. `spans::interaction`: add `quack.interaction.id`. `spans::job`: add
    `quack.job.max_attempts` and `quack.guild.id`. New `spans::use_case(name:
    &'static str, guild_id)` (`use_case {name}`, INTERNAL).
12. `SpanEvent`: add `CaseVoided` (`case.voided`), `ModLogEntry`
    (`mod_log.entry`), `Refused` (`refused`).

Everything else in this design uses the crate as it is: its instrument
names, its attribute keys (`quack.guild.id`, `quack.actor.id`,
`quack.job.queue_wait` in seconds, `quack.discord.rate_limit.wait` in
seconds), its buckets, its stderr logging, its defaults in §18.6.

### 18.4 Spans

Every unit of work is a span; each DB query and each Discord REST call is a
child span.

| Span name | Kind | Attributes | Events |
| --- | --- | --- | --- |
| `interaction {kind} {command}` (new root) | SERVER | `quack.interaction.kind`, `quack.command`, `quack.guild.id`, `quack.actor.id`, `quack.interaction.id`, `quack.interaction.outcome` (recorded at the end) | `acked`, `replied`, `refused` |
| `gateway {event}` | CONSUMER | `quack.gateway.event`, `quack.guild.id` | — |
| `{method} {route}` (HTTP server, e.g. `GET /api/guilds/{guild_id}/cases`; new root, an incoming `traceparent` is ignored) | SERVER | `http.request.method`, `http.route`, `http.response.status_code` | — |
| `job {kind}` (new root with a **link** to the enqueuer's trace) | CONSUMER | `messaging.operation.type = process`, `quack.job.kind`, `quack.job.id`, `quack.job.attempt`, `quack.job.max_attempts`, `quack.job.queue_wait` (s), `quack.guild.id`, `quack.job.outcome` (recorded at the end) | — |
| `{operation} {table}` (DB, e.g. `UPDATE case_counters`) | CLIENT | `db.system.name`, `db.operation.name`, `db.collection.name`, `db.query.summary`; `error.type` on failure | — |
| `{method} {url.template}` (HTTP client, e.g. `PUT /guilds/{guild_id}/bans/{user_id}`) | CLIENT | `http.request.method`, `url.template`, `server.address`, `http.response.status_code`, `quack.discord.outcome`, `quack.discord.rate_limit.wait` (s) | `rate_limited` |
| `use_case {name}` (e.g. `use_case apply_rule`) | INTERNAL | `quack.guild.id`, IDs relevant to the use case | domain events below |

Domain events (on the current span): `case.created` (`case.id`,
`case.number`, `level_index`, `action_kind`), `case.voided` (`case.id`,
`cause`), `action.attempt` (`action.id`, `kind`, `attempt`, `outcome`,
`http_status`, `discord_code`), `dm.outcome` (`dm.id`, `kind`, `state`,
`failure`), `appeal.transition` (`appeal.id`, `from`, `to`),
`evidence.captured` (`evidence.id`, `state`), `message.synced`
(`message_row.id`, `kind`, `outcome`), `mod_log.entry` (`entry.id`, `kind`,
`result`). Span status and events never carry Discord or SQL error text,
only status codes and error classes.

**Propagation into jobs.** `enqueue` stores `current_trace_context()` (a W3C
`traceparent`) in `jobs.trace_context`; a coalesced enqueue keeps the first
one. The `job {kind}` span is a new root with a span link to that context,
so a retry hours later stays a short trace while still pointing at the
request that caused it. Interactions and HTTP requests start new traces.

### 18.5 Logs

- Emitted with `tracing`, written to **stderr** (JSON lines when stderr is
  not a terminal, text otherwise; the binary chooses), and exported over
  OTLP (`opentelemetry-appender-tracing`), where every record carries the
  current trace and span IDs. JSON stderr lines carry the span fields.
- Filters: `QUACK_LOG_LEVEL` filters logs (stderr and OTLP); spans are
  exported at `info` regardless, so lowering log verbosity never drops
  traces. The export bridges exclude `opentelemetry*`, `hyper`,
  `hyper_util`, `reqwest`, `h2`, `tonic`, and `tower` events to avoid
  feedback loops; those still reach stderr.
- Fields: `timestamp`, `level`, the Rust module path, `message`, and
  structured IDs (`guild_id`, `case_id`, `action_id`, `appeal_id`,
  `job_kind`, `job_id`, `account_id`) as span fields.
- Levels: `error` for failures needing an operator; `warn` for degraded but
  handled conditions (dropped events, exhausted retries, exporter
  failures); `info` for lifecycle (startup, shutdown, gateway connect);
  `debug` for per-request detail.
- **Never logged** (and never span attributes): the bot token, the client
  secret, the session secret, the import source URL, OAuth codes and
  states, Discord access tokens, session IDs, CSRF tokens, interaction
  tokens, message or evidence content, case reasons, context values, appeal
  text, transcripts, DM text, IP addresses, and Discord error bodies (only
  status and error code). Secrets are held in a `Secret<T>` newtype whose
  `Debug` and `Display` print `[redacted]`. Request logs use the route
  pattern, never the raw path or query.

### 18.6 Resource and export

These are the crate's defaults; the binary passes the configuration.

- Resource attributes: `service.name` (`QUACK_SERVICE_NAME`),
  `service.version` (the binary's crate version), `service.instance.id`
  (`<hostname>-<pid>`), `deployment.environment.name`
  (`QUACK_ENVIRONMENT`). Dashboards sum over instances.
- Export: OTLP/HTTP protobuf to `QUACK_OTLP_ENDPOINT`, 5-second export
  timeout. Spans and logs: the SDK's batch processors (bounded queues that
  drop when full). Metrics: periodic reader every 30 seconds, cumulative
  temporality, explicit-bucket views from the catalog.
- Sampling: parent-based, always on (moderation volume is low; every root
  is Quack's own).
- Shutdown flushes all three providers within the time left of the shutdown
  deadline, at most 5 seconds (§17.5). Export errors are reported by the
  SDK on stderr.

### 18.7 Dashboards and alerts to build

Dashboards:

1. **Moderation**: cases by source and action kind; level distribution;
   refusals; actions by attempt outcome and reason; completion time
   (p50/p95); DM outcomes by reason; appeals by transition, open, and time to
   decision.
2. **Reliability**: job ready depth and oldest-ready age by kind; job
   outcomes; lease expirations; dead jobs; failed actions waiting for staff;
   tracked message sync lag; log channel post outcomes.
3. **Discord**: REST requests by route and outcome (count of
   `http.client.request.duration`); rate-limit hits and wait; gateway
   connected, connection events, heartbeat latency; event rate and drops;
   interaction ack latency (p50/p99) and errors.
4. **API and database**: request rate, latency, and 5xx by route (from
   `http.server.request.duration`); sign-ins; rate-limit rejections; pool
   usage, wait, and timeouts; query and transaction latency; DB errors (by
   `error.type`) and retries.
5. **Runtime**: CPU, memory, file descriptors, threads, Tokio busy time,
   parks, and global queue depth, cache sizes and hit rates.

Alerts (rates and increases over counters; gauges report explicit zeros):

| Alert | Condition |
| --- | --- |
| Not ready | `/readyz` failing for 2 minutes. |
| Telemetry missing | No `quack.build.info` series for 5 minutes (the collector or exporter is down, which is different from healthy zeros). |
| Gateway down | `quack.gateway.connected` = 0 for 2 minutes. |
| Slow acknowledgements | p99 `quack.interaction.ack.duration` > 1.5 s for 10 minutes, or any observation above 3 s. |
| Job backlog | `quack.jobs.oldest_ready.age{quack.job.kind="run_action"}` > 120 s, or any kind > 600 s, for 5 minutes. |
| Leases expiring | `quack.job.lease_expired` increases on 3 consecutive 5-minute windows. |
| Dead jobs | `quack.jobs.dead` > 0 for `run_action`, `send_dm`, or `close_ticket`. |
| Failed actions | `quack.actions.failed` grows by 10 or more in an hour. |
| Unconfirmed effects | `quack.dm.outcomes{quack.dm.outcome="unknown"}` grows by 5 or more in an hour, or `quack.action.attempts{quack.action.attempt.outcome="unknown"}` by 20 or more. |
| Discord rate limits | `quack.discord.rate_limit.hits{quack.discord.rate_limit.scope="global"}` increases in 5 minutes. |
| Dropped events | `quack.queue.dropped` increases in 5 minutes. |
| API errors | 5xx share of `http.server.request.duration` count > 2% for 10 minutes. |
| Database | p95 `db.client.connection.wait_time` > 100 ms, any `db.client.connection.timeouts`, or failed operations > 10/min, for 5 minutes. |

---

## 19. MySQL to PostgreSQL import

Go table names, column names, and state values appear verbatim in this
section because they are the source data.

`quack import-mysql` copies the Go system's MySQL database into an empty or
partly imported PostgreSQL database once, at cutover. It lives in
`quack-import`, the only crate with sqlx's `mysql` feature. Three rules
govern it:

1. **Nothing uncertain is replayed.** A Discord effect that may already have
   happened (a running execution, a notification being sent) is imported as
   a terminal "couldn't confirm" outcome for staff to review, never retried.
2. **Legacy data is imported as it was, or reported, never silently
   changed or dropped.** Values over the new limits are kept and listed;
   rows that cannot be imported are listed with their keys.
3. **The import is deterministic.** It reads one consistent snapshot,
   evaluates every time comparison against one fixed `evaluated_at`, and
   maps IDs by a fixed function, so a rerun produces the same rows.

### 19.1 Preconditions

- The Go bot and its dashboard server are stopped and fenced (§19.12 step
  3). The importer reads the source over `QUACK_IMPORT_SOURCE_URL` with a
  read-only account, in one `REPEATABLE READ` snapshot (`START TRANSACTION
  WITH CONSISTENT SNAPSHOT`), session time zone `+00:00`; every
  `datetime(3)` is read as UTC.
- The source's `quack_schema_migrations` must be one of the accepted
  ledgers: versions `1–3`, `1–4`, or `1–5` with the names `baseline`,
  `audit_mirror_deliveries`, `launch_announcement`, `staff_roles`,
  `discord_user_mfa` in order.
- **Source manifest.** `quack_import::manifest` lists, for each accepted
  ledger, every source table and column with its disposition: mapped (to
  which column, by which rule in this section) or dropped (with the reason).
  Before reading data the importer compares it with
  `information_schema.columns`; a missing or unexpected column aborts. The
  report includes the manifest, so every source column's fate is on record.
- The PostgreSQL schema is migrated (`quack migrate`), and `quack serve` is
  not running against it: the importer takes the singleton fence (§17.5)
  for the whole run and refuses to start when it is held.

### 19.2 Command and modes

```
QUACK_IMPORT_SOURCE_URL=mysql://… quack import-mysql [--dry-run] [--report report.json]
```

- `--dry-run` reads everything, runs every transformation, and writes every
  server's rows inside transactions that are rolled back, so every
  constraint is checked for real. It keeps only its `import_runs` row (mode
  `dry_run`) and prints the report.
- Without `--dry-run` the import writes one PostgreSQL transaction per
  server, including its `import_guilds` row, plus one for global tables
  (`discord_accounts`), so a failure leaves each server either fully
  imported or untouched. Jobs for a server are inserted only in that
  server's transaction.
- The report goes to stdout (human summary) and, with `--report`, to a JSON
  file; it is also stored in `import_runs.report`.

### 19.3 Identifier mapping

- **ULID → UUID.** Deterministic and order-preserving. A ULID is 128 bits:
  a 48-bit millisecond timestamp `T` and 80 random bits `R`. The UUID is
  `T (48 bits) ‖ 0b0111 (version 7) ‖ R[0..12) ‖ 0b10 (variant) ‖ R[12..74)`;
  bits `R[74..80)` are dropped, so 122 bits of the ULID are kept. The
  importer checks that every mapped key is unique per table and aborts on a
  collision. `quack_core::ids::ulid_to_uuid` is this function; the runtime
  uses it to resolve legacy ULIDs in custom IDs and URLs (§11.5, §14.1),
  always followed by the normal ownership or capability check. Guessing a
  legacy ULID that maps to someone's record requires knowing that record's
  ULID.
- **Snowflake strings → `bigint`.** Trimmed decimal of 1–20 digits, value in
  `1..=i64::MAX`. Empty string and `quack-system` → NULL (Quack). Anything
  else → the row is reported (`invalid_snowflake`) and, for a required
  value, the row and everything that depends on it are reported and skipped;
  a server whose own ID is invalid aborts.
- **Server.** The internal `guilds.id` ULID disappears; every child maps its
  `guild_id` ULID to the server's Discord ID through the `guilds` table.
- **Booleans** `tinyint(1)` must be 0 or 1. **Unsigned** values must fit
  the new column type (case numbers ≤ 2^53 − 1; versions ≤ 2^31 − 1);
  otherwise abort with the offending table and key.
- **Text** sentinels `''` → NULL for optional fields. Text longer than a new
  limit is imported unchanged and listed in the report (`over_limit`, per
  column); the limit applies at the next edit. Invalid UTF-8 or control
  characters other than tab and newline are reported and replaced with
  U+FFFD (`normalized`). Invalid JSON aborts the server with the key.
- **Ties** are broken by source timestamp, then source ID.

### 19.4 Table mapping

| MySQL table | PostgreSQL | Transformation |
| --- | --- | --- |
| `guilds` | `guilds`, `case_counters` | `discord_guild_id` → `guild_id`; `name` (Discord caps it at 100); `icon_url` → `icon_hash` (the hash segment of the CDN URL, else NULL); `owner_discord_user_id` → `owner_id`; `is_active` → `installed`; `created_at` → `first_joined_at`; `updated_at` kept; `left_at` NULL (unknown); `mfa_required = false` until the next `GUILD_CREATE`. `case_counters.last_number = max(cases.case_number)` for the server (0 if none). |
| `guild_settings` | `guild_settings` | `moderator_role_ids`, `rules_manager_role_ids` (comma-separated) → `bigint[]` (deduplicated; each invalid entry reported; more than 25 → the server aborts); `audit_mirror_channel_discord_id` → `log_channel_id`; `appeal_queue_channel_discord_id` → `appeal_channel_id`; `managed_evidence_channel_discord_id` → `evidence_channel_id`; `appeal_rejoin_url` → `appeal_invite_url` (re-validated; invalid → NULL, reported); `notification_introduction` → `dm_intro`, `notification_footer` → `dm_footer` (as they are); `starter_policy_template_id` → `starter_rule_id`; `starter_policy_notice_acknowledged_at` → `starter_notice_acknowledged_at`; `updated_at` kept. Dropped: `id`, `created_at`, `appeal_review_reason_required`, `starter_policy_notice_pending` (derived), `launch_announced_at`. `version = 1`. |
| `staff_members` | — | Dropped (attribution comes from Discord; counts reported). |
| `discord_user_mfa` | `discord_accounts` | `discord_user_id` → `account_id`; `mfa_enabled`, `mfa_checked_at`, `updated_at` kept; `created_at` dropped. Absent table → nothing. |
| `case_templates` + `case_template_context_fields` + `case_template_levels` + `case_template_level_actions` | `rules`, `rule_versions` | §19.5. |
| `cases` | `cases` | §19.6. |
| `case_action_executions` | `actions` | §19.7. |
| `case_action_attempts` | `action_attempts` | `execution_id` → `action_id` (the guild comes from the action); `attempt_number` → `number`; `started_at`, `finished_at`; status `succeeded` → `done` (`no_change` when the response says `reversal_noop`), `failed` → by `error_code`: suffix `rate_limited` → `not_sent`; `network_error`, `discord_server_error`, `context_cancelled`, `discord_error` → `unknown`; `lease_expired` → `interrupted`; any other → `refused`. `running` with no finish → `interrupted`, `finished_at = started_at`. The response payload is read for §19.7 first, then dropped with `worker_id`, `duration_ms`, `error_message`, and the attempt row's own ID and timestamps; HTTP status and Discord code NULL. |
| `case_notifications` | `dms` (kind `case`) | One per case. `pending`, `prepared`, `claimed` → `pending` (sent after cutover). `sending` → `unknown` (`first_attempt_at` and `settled_at` = the row's `updated_at`). `sent` with a `prepared_channel_discord_id` and `delivery_message_discord_id` → `sent` (`channel_id`, `message_id`; `first_attempt_at` and `settled_at` = `sent_at`); `sent` without both → `unknown`, reported. `failed` with `last_error_code = 'case_voided'` → `not_needed`; other `failed` → `failed` with `failure`: code ending `permission_or_hierarchy_denied` → `dms_closed`, `dm_prepare_failed` → `discord_unavailable`, else `discord_refused`; `settled_at` = `updated_at`. A case whose level sent a DM but has no notification row gets no DM row (counted, never invented). Dropped after classification: `attempt_count`, `last_error`, `lease_token`, `lease_expires_at`, rendered text. |
| `case_evidence_snapshots` | `evidence` | `capture_outcome`: `captured`, `uploaded` → `saved` (`saved_at` = `updated_at`); `deleted` → `failed`/`message_deleted`; `inaccessible` → `failed`/`quack_no_access`; `unavailable` → `failed`/`discord_error`. `channel_discord_id`, `message_discord_id`, `author_discord_user_id`, message timestamps mapped. Uploaded synthetic snapshots (`uploaded`, the file ID as message ID) keep their IDs. `embeds_json` → summaries `{type, title, description, url}` (first 10; the rest counted). `content` (Go kept at most 4,000 characters). Duplicate `(case, message)` rows: the earliest is kept and its files merged by attachment ID; the rest are reported. Dropped: `capture_warning`, `message_url` (derived). |
| `case_evidence_attachments` | `evidence_files` | `position` by creation order (1–10; extras counted in `files_not_saved` and reported); `filename`, `content_type`, `size_bytes`, `url` → `original_url`; `attachment_id` from the URL path. `copy_outcome` `preserved` → `copied` only when the copy channel, message, and attachment IDs can all be read from `preserved_url` and agree with the preserved ID columns; otherwise `link_only`, reported. `metadata_only` → `link_only`. `warning` dropped after classification. |
| `case_events` | — | Not imported; the case timeline is rebuilt from cases, actions, attempts, DMs, appeals, and the moderation log, which hold the same facts. Counts per event type reported. |
| `case_publications` | `messages` (kind `case_receipt`) | One per case: the latest row with a message ID. `message_id`, `channel_id`; `wanted_revision = posted_revision = 1` (old receipts are not re-rendered; their old buttons resolve through §11.5). Dropped: `presentation_json` (after checking its server ID), digest, retry fields, `revision`, `refresh_requested`. |
| `appeals` | `appeals` | Same states. `target_discord_user_id` must equal the case's member (else reported and skipped); `statement`; `reviewed_by_discord_user_id` → `decided_by`; `reviewed_at` → `decided_at`; `decision_reason`; `version` kept; `first_decided_at` = earliest accepted/rejected/closed event time. NULL `case_id` → skipped and reported. `review_message_discord_id` is a candidate for the appeal post. `metadata_json` dropped. |
| `appeal_events` | `appeal_events` | `event_type`: `submitted`, `information_requested`, `information_submitted` → `information_provided`, `reopened`, `accepted`, `rejected`, `closed`; `actor_discord_user_id` → `actor_id` (`actor_type = staff` with empty actor → NULL); `created_at` kept; `body` → `text` (empty → "(no text)"). A close caused by a void (`metadata_json` says so) → `closed_by_void`. Dropped: `actor_type`, `metadata_json`, `updated_at`. |
| `appeal_notifications` | `dms` (kind `appeal_update`) and `messages` (kind `appeal_post`) | `audience = member`: one DM per `event_id` (the guild, appeal, and event must agree with the event row), mapped like case notifications (`delivery_outcome_unknown`, `sending` → `unknown`; `delivery_deferred`, `pending` → `pending`); `target_discord_user_id` must equal the appeal's member. `audience = staff`: the latest row per appeal with a `delivery_message_id` (or the appeal's `review_message_discord_id`) becomes the appeal's `appeal_post`, with `wanted_revision = 2`, `posted_revision = 1` for `pending` or `needs_information` appeals so the new buttons replace the old ones (`1`/`1` otherwise). An appeal with no staff row gets an appeal post with no channel (§8.3). Bodies and leases dropped. |
| `audit_log_entries` | `mod_log` | §19.8. |
| `audit_mirror_deliveries` | — | Not imported; old entries are never re-posted. Counts by state reported. |
| `module_configurations` (`tickets`) | `ticket_settings`, `messages` (`ticket_panel`) | `enabled`, `updated_at`, and from `config_json`: `entry_channel_discord_id`, `queue_channel_discord_id`, `transcript_retention_days`. The panel (`entry_panel_channel_id`, `entry_panel_message_id`) becomes a tracked message with `wanted_revision = 2`, `posted_revision = 1`. An enabled configuration missing a channel becomes disabled, reported. |
| `module_configurations` (`general_logging`) | `activity_log_settings`, `activity_log_routes` | `enabled`, `updated_at`; `channels` map (NULL → empty): `message_edit` → `message_edited`, `message_delete` → `message_deleted`, `message_bulk_delete` → `messages_bulk_deleted`, `member_join` → `member_joined`, `member_leave` → `member_left`, `discord_ban` → `member_banned`, `discord_unban` → `member_unbanned`, `guild_change` → `server_changed`, `channel_change` → `channel_changed` (an invalid channel ID drops that route, reported). `include_message_content`, `include_attachment_metadata` → `include_file_names`, `include_embed_metadata` → `include_embed_types`. Dropped: `cache_entries_per_guild`, `max_delivery_attempts`. Enabled with no route → disabled, reported. |
| `module_configurations` (`honeypots`) | `honeypot_settings`, `messages` (`honeypot_warning`) | `enabled`, `updated_at`; `channel_discord_id`, `template_id` → `rule_id`, `warning_text` (the legacy default text → NULL); a non-empty `disabled_reason` → `enabled = false`, `disabled_reason = channel_deleted` when it says the channel was deleted, else NULL. `warning_channel_discord_id` + `warning_message_id` become the tracked warning (`wanted_revision = 2`, `posted_revision = 1`). |
| `tickets` | `tickets` | `open` → `open` (`opened_at` = `created_at`); `resolved`, `cancelled` → `closed` (`closed_at` = `resolved_at`, else `updated_at`); `owner_discord_user_id` → `member_id`; `thread_discord_channel_id` → `thread_id`; `resolved_by_discord_user_id` → `closed_by`; `entry_channel_id` from the settings at import; step columns set to `closed_at` for closed tickets. Two live tickets for one member: the newest stays `open`, the others become `closed`, reported. `log_channel_discord_id` + `log_message_discord_id` → a `ticket_post` tracked message (`1`/`1`). A closed ticket whose `close_notice` shows a member DM was attempted gets a `ticket_closed` DM row in its mapped state, so it is never sent again. Dropped after checking: `queue_delivery_attempt_id`, `transcript_url`, other metadata. |
| `ticket_events` | — | Not imported (lifecycle timestamps are on the ticket). Counts reported. |
| `ticket_member_states` | — | Not imported (the slot is derived, §13.2.2). |
| `ticket_message_journal` | `ticket_messages` | `message_discord_id`, `author_discord_user_id`, `body` → `message_id`, `author_id`, `text`; `author_name` NULL → "unknown"; `files = []`; the thread ID must match the ticket's. |
| `ticket_transcripts` | `ticket_transcripts` | Content as stored; `complete` from the Go row's completeness flag when present, else `false` with `partial_cause = history`; rows already expired at `evaluated_at` are skipped and counted. |
| `honeypot_triggers` | `honeypot_hits` | `channel_discord_id`, `message_discord_id`, `target_discord_user_id` mapped; `settings_version = 1`; `template_id` → `rule_id`. `created` → `case_created`; `failed`, `exempt` → `failed` (`failure` = old `failure_code` or `exempt`); `pending` → §19.9. |
| `honeypot_message_cleanups` | `honeypot_hits.cleaned_up_at` | `completed_at` copied to the matching hit; incomplete cleanups for created hits enqueue `honeypot_cleanup` (deleting the bait is idempotent). |
| `honeypot_warning_refreshes` | — | Not imported (the tracked message replaces it). |
| `v4_import_batches`, `v4_import_sources` | — | Not imported; each v4-imported case keeps its source ID in `cases.imported`. |
| `quack_schema_migrations` | `import_runs.report` | Recorded in the report only. |

Every mapped table's source `id` (ULID) becomes the row's UUID by §19.3,
and its timestamps are kept where a target column exists; the manifest
lists every other column.

### 19.5 Rules and their versions

For each `case_templates` row:

1. `rules`: `id` = mapped ULID; `guild_id`; `name` (kept; when two
   non-archived rules in a server have the same lowercase name, the later
   one by §19.3's tie-break gets " (2)", " (3)" …, checked against every
   name in the server, and is reported); `current_version` = `version`;
   `archived_at`; `created_by`/`updated_by` from the Discord ID columns.
   Dropped: `slug`, `default_template_id` (after checking it), child IDs and
   timestamps, level `name`, `max_retries`.
2. The **current version** is built from the child rows and validated with
   `Strictness::Imported { reconstructed: false }` (§3.3):
   - `name`, `description`, `reason` = `reason_template`, `appealable`,
     `counting_window_days` = `case_decay_days` (0 → null).
   - `fields`: context fields by `position`, keeping `key` and `required`;
     `field_type` `boolean` → `boolean`, `discord_message_link` →
     `message_link`, others same.
   - `levels`: the default level (`is_default`, threshold 0) becomes
     `from_case_count = 1`; other levels use `trigger_case_count`. If a
     non-default level also has threshold 1, it replaces the default level
     (the Go code never chose the default in that case) and the dropped
     default is reported. `notify_user` → `send_dm`. Actions from
     `config_json`: `timeout_user {duration_seconds}` → `timeout`,
     `kick_user` → `kick`, `ban_user {delete_message_seconds}` → `ban`
     (missing → 0).
   - Size-limit violations (more than 10 levels or fields, long texts) are
     kept and reported (`over_limit`); a structural failure aborts that
     server's import with every problem listed.
3. **Older versions** referenced by cases (`cases.template_version <
   case_templates.version`) are reconstructed from those cases'
   `template_snapshot_json`: name, reason, appealable, decay, and fields
   (`type`, `key`, `required`) from `template`/`context_fields`; levels =
   the distinct `selected_level`s seen across those cases, each at its
   snapshot position, with its `timeout_duration_seconds` and actions. They
   are stored with `reconstructed = true` and validated with
   `Strictness::Imported { reconstructed: true }`. When cases disagree about
   the same version, the earliest case's snapshot wins and the others are
   reported (`conflicting_snapshots`). Versions no case used are not created
   (version numbers may have gaps). The report lists every reconstructed
   version and how many levels it knows.

### 19.6 Cases

| Column | Source |
| --- | --- |
| `id`, `number`, `member_id`, `created_at`, `updated_at` | mapped `id`, `case_number`, `target_discord_user_id`, timestamps |
| `member_joined_at` | NULL (unknown; a kick then compares with the case's `created_at`, §5.1) |
| `source` | `dashboard`, `discord`, `honeypot` same; `v4_import` → `imported` |
| `moderator_id` | `moderator_discord_user_id` (NULL for honeypot and `quack-system`) |
| `rule_id`, `rule_version` | `template_id`, `template_version` (imported cases: NULL) |
| `case_count` | snapshot `selected_level.matched_case_count` |
| `level_index`, `level_from_case_count` | the snapshot level's position in the version it used (`trigger_case_count` 0 → 1), even when the normalized current version no longer has that level |
| `action_kind`, `timeout_seconds`, `ban_delete_message_seconds` | snapshot `actions[0]` (`timeout_duration_seconds` for timeouts) |
| `sends_dm` | snapshot `selected_level.notify_user` |
| `context` | `context_values_json` array → object `key → value`; numbers → canonical decimal strings |
| `operation_key` | `idempotency_key` → `legacy:<key>` (kept for uniqueness only; the new scopes never produce it) |
| `replaces_case_id` | `replaces_case_id` (checked: same server, the replaced case voided, not itself, not replaced twice; otherwise dropped and reported) |
| `voided_at`, `voided_by`, `void_reason`, `void_cause` | `voided_at`, `voided_by_discord_user_id`, `voided_reason`; `void_cause = appeal_accepted` when an accepted appeal exists for the case, else `staff`; empty reason → "(no reason recorded)". `validity` is compared with `voided_at` and disagreements are reported. |
| `imported` | v4 cases: `{v4_action, v4_moderator, v4_reason, v4_source_id}` from `action_type`, `moderator_display_name`, `reason`, `source_id`; `sends_dm = false`, no action |

A non-imported case whose snapshot lacks `selected_level` or `template`
aborts that server's import with the case number listed (an operator must
decide). Dropped columns: `correlation_id`, `context_*` coordinates (their
messages are evidence rows), `metadata_json` (except v4), `source_name`,
`target_departed`, `target_missing`, `action_expires_at`, `historical`, and
`template_snapshot_json` (after use).

### 19.7 Actions

| Column | Source |
| --- | --- |
| `id`, `case_id`, `guild_id`, `member_id` | mapped execution ID; the case, its server, and its member |
| `kind` | `timeout_user` → `timeout`, `kick_user` → `kick`, `ban_user` → `ban`, `remove_timeout` → `remove_timeout`, `unban_user` → `unban`; `send_dm` rows and their attempts are skipped and counted |
| `undo_of_action_id` | `reversal_of_execution_id` (the original must be in the same case) |
| `timeout_ends_at` | timeouts only: for `done`, the last successful attempt's `response_payload_json.timeout_until`; when that is missing, `case.created_at + duration_seconds`, reported (`derived_timeout_end`) |
| `delete_message_seconds` | bans only: `config_snapshot_json.delete_message_seconds`, else the case's `ban_delete_message_seconds`, else 0; NULL for every other kind |
| `requested_by` | `config_snapshot_json.requested_by` |
| `state` | `succeeded` → `done`; `cancelled` → `cancelled`; `failed` → the table below; `pending`, `retrying`, `running` → §19.9 |
| `attempts` | 0: the current round's budget starts at cutover (the attempt rows keep the history) |
| `dismissed_at`, `dismissed_by` | `dismissed_at`, `dismissed_by_discord_user_id` |
| `done_at`, `no_change` | `finished_at` for `done`; `no_change` when the last successful attempt's payload says `reversal_noop` |

Dropped after reconciliation: `template_action_id`, `position`,
`idempotency_key`, `notify_user`, `notification_type`, `max_retries`,
`retry_backoff_ms`, `safe_for_retry`, `last_error`, `correlation_id`,
`lease_token`, `lease_expires_at`, `reversal_appeal_id`, `next_retry_at`.

Failed executions, by `last_error_code`:

| Old code (suffix) | New state and reason |
| --- | --- |
| `permission_or_hierarchy_denied` | `failed`, `discord_refused`, HTTP 403 |
| `unknown_member_or_resource` | timeout: `failed`, `member_not_in_server`; others: `failed`, `discord_refused`, HTTP 404 |
| `validation_failed`, `discord_failure`, `invalid_action_config`, `unsupported_action` | `failed`, `discord_refused`, HTTP 400 |
| `guild_not_found` | `failed`, `quack_not_in_server` |
| `rate_limited`, `discord_unavailable`, `guild_lookup_failed` (when no attempt may have applied) | `failed`, `retries_exhausted` |
| `lease_expired_review_required`, `network_error`, `discord_server_error`, `context_cancelled`, `discord_error`, or any code after an attempt that may have applied | `failed`, `unconfirmed` |
| `reversal_ownership_conflict`, `reversal_provenance_unavailable` | `skipped`, `skip_reason = changed_outside_case` (the Go system could not show the effect was its own; nothing is evaluated against live Discord during import) |
| `reversal_authorization_unavailable`, `reversal_permission_denied` | `failed`, `discord_refused`, HTTP 403 |
| `case_voided` | `cancelled` |
| any other | `failed`, `unconfirmed` |

### 19.8 Moderation log

`audit_log_entries` are append-only history, so every row is imported,
none re-posted, and **every imported row has kind `legacy`**. A Go entry
rarely holds everything a typed kind's `detail` needs (§9.2), and
`ModLogEntry::from_detail` reads `detail` strictly by kind, so mapping Go
rows onto typed kinds would leave rows the reader cannot load. Keeping them
`legacy` means every row reads back, and nothing in Quack depends on an
imported entry's kind: statistics count cases and actions from their own
rows (§14.2), and failed actions come from `actions`.

- `detail`: §9.5's redacted `{action, resource_type, resource_id,
  failure_reason}` (`LegacyDetail`). `action` is the Go action name; the
  dashboard and `log.legacy` (§12.10) show it in today's words
  (`case_template.create` → "case rule create").
- `result`: `success` → `ok`, `failure` → `failed`, `denied` → `refused`
  (`ModLogKind::Legacy` has no fixed result; the imported one is kept).
- `source`: `imported`. `actor_id` from `actor_discord_user_id`.
- Typed links are filled from `resource_type`/`resource_id` and from
  metadata keys `case_id`, `target_discord_user_id`/`member_discord_user_id`,
  `appeal_id`, then checked against each other (the appeal's case, the
  case's member); a link to a row that was not imported, or one that
  disagrees, is left NULL and reported. So a case's or member's history on
  the dashboard includes its imported entries; the `kind` filter (§9.6)
  finds them only as `legacy`.
- `created_at` kept; `id` = mapped ULID; `trace_id` NULL (a Go
  `correlation_id` is not a W3C trace ID). Dropped: `actor_permission_bits`,
  the old `source`, `correlation_id`, `request_id`, `updated_at`, and every
  other metadata key.

### 19.9 In-flight work at cutover

The Go bot is stopped before the import; work it left unfinished is
resolved like this:

| Old state | New state |
| --- | --- |
| Execution `pending` or `retrying` on a valid case, with no attempt that may have applied | Action `pending` + `run_action` job: runs after cutover (every attempt reads Discord first, §5.1). |
| Execution `running`, or `pending`/`retrying` after an attempt that may have applied (§5.2) | Action `failed` (`unconfirmed`); an open attempt becomes `interrupted`. Never replayed: staff see it on the failed actions page and decide. |
| Original execution on a voided case, `pending`/`retrying` with no attempt that may have applied | `cancelled`. Undo executions on voided cases are never cancelled (§4.7); they follow the rows above. |
| Notification `pending`, `prepared`, `claimed` (never sent) | DM `pending` + `send_dm` job: sent after cutover. For a kick or ban whose action is also pending, the action waits for the DM (§6.3). |
| Notification `sending` | DM `unknown`. Never sent. |
| Appeal notification (member) `pending` or deferred | DM `pending` + job. `sending` or delivery unknown → `unknown`. |
| Appeal staff post with `refresh_requested` | Appeal post re-rendered (`wanted_revision = 2`). |
| Audit mirror delivery not delivered | Dropped (reported). |
| Publication with `refresh_requested` | Not re-rendered; old buttons resolve through §11.5. |
| Honeypot trigger `pending` | A case with the Go operation key `honeypot:<guild ULID>:<message ID>` exists → the hit becomes `case_created` with that case (and a cleanup job). Otherwise `failed` (`imported_pending`), reported: Quack never acts after cutover on a hit admitted under Go's configuration. |
| Ticket member slot reservation without a ticket | Dropped. |
| Ticket `open` whose thread no longer exists | `open`; the first `GUILD_CREATE` reconciliation (§13.1.5) finds the thread gone and closes the ticket. |

### 19.10 Validation report

`Report` (JSON; the summary prints the same sections):

| Section | Contents |
| --- | --- |
| `source` | MySQL server version, ledger rows, snapshot time, `evaluated_at`, per-table row counts and digests, the source manifest. |
| `written` | Per-table rows written (or that would be written, in dry-run), per server. |
| `skipped` | Per table: count and reason code, with up to 20 example keys each (IDs only, no content). Reason codes: `dm_action_row` (Go `send_dm` executions), `null_case_appeal`, `member_mismatch`, `expired_transcript`, `duplicate_evidence`, `extra_attachment`, `invalid_snowflake`, `missing_parent`, `invalid_replacement`, `invalid_invite`, `invalid_route`, `duplicate_live_ticket`. |
| `over_limit` | Per column: count of values longer or larger than the new limit, with example keys. Imported unchanged. |
| `normalized` | Per column: count of values whose invalid characters were replaced. |
| `derived` | Values the importer had to derive: `derived_timeout_end`, `unconfirmed_dm` (sent rows without handles), `copy_unavailable`. |
| `renamed_rules` | Rules renamed for name uniqueness (old name, new name). |
| `reconstructed_versions` | `(rule, version, known levels)`; `conflicting_snapshots`. |
| `dropped_default_levels` | Rules whose default level was replaced (§19.5). |
| `legacy_mod_log` | Count of imported entries by old action name, and of links left NULL by link column. |
| `in_flight` | Counts per row of §19.9. |
| `checks` | Relationship checks: every case's rule version exists; every action's case exists and matches server and member; undo actions match their original's case; replacement links are same-server and point at voided cases; appeal case and member match; DM members match their case or ticket; counters ≥ max case number; mapped UUID uniqueness. Each `ok` or the failing keys. |
| `problems` | Errors that aborted a server, per server. |

The import exits 0 when every server imported and every check passed, 1
otherwise. Dry runs exit the same way.

### 19.11 Re-running

- The **source fingerprint** is SHA-256 over the ledger, the manifest
  version, the transform version, and, per table, a digest of every row in
  primary-key order. It changes when any source row changes.
- A server is imported in one transaction together with its
  `import_guilds` row. A rerun skips servers already in `import_guilds` with
  the same fingerprint and imports the rest, so a rerun after a partial
  failure completes the missing servers. A server already imported with a
  different fingerprint makes the importer refuse: to import changed source
  data, drop and re-create the PostgreSQL database and run `quack migrate`
  again.
- Inserts never use `ON CONFLICT DO NOTHING`: a row that already exists
  must be identical to the one the importer would write, or the import
  fails naming both keys. Unique-constraint collisions (active rule names,
  case numbers, live tickets, one DM per event) are errors with both source
  keys, not skipped rows.
- Counters use `GREATEST(last_number, $max)`; jobs are inserted only in
  their server's transaction.

### 19.12 Cutover runbook

1. **Rehearse.** Restore a production backup into a uniquely named staging
   MySQL container; import into an empty staging Postgres; run `quack
   serve` with a **staging Discord application that is in no production
   server**, so every Discord effect on production coordinates is refused
   (Unknown Guild, no mutual server) and nothing reaches real members. Walk
   through §20.3's checklist, then delete the staging databases.
2. **Announce** a maintenance window (expected under 15 minutes for
   current data sizes).
3. **Fence the Go system** at the deployment layer: scale the Go bot and
   its dashboard server to zero; confirm no Go process is running and that
   MySQL shows no connections from them (`SHOW PROCESSLIST`); make the
   MySQL account read-only.
4. **Back up MySQL** (`mysqldump --single-transaction`) and verify the dump
   restores.
5. **Prepare Postgres**: create the database; `quack migrate`.
6. **Dry run** against production: `QUACK_IMPORT_SOURCE_URL=… quack
   import-mysql --dry-run --report dry.json`; review `problems`, `checks`,
   `in_flight`, `over_limit`.
7. **Import**: `QUACK_IMPORT_SOURCE_URL=… quack import-mysql --report
   import.json`; it must exit 0. Review the report.
8. **Decision.** Until now nothing Rust did has reached Discord (neither
   `migrate` nor `import-mysql` calls it). Rolling back here means: start
   the Go system again against the untouched MySQL with its original
   configuration. Record the decision.
9. **Point of no return: start `quack serve`** with the production
   configuration (same Discord application and token as Go). Record the
   time. From this moment Rust syncs commands (removing Go-only commands),
   connects the gateway, re-renders appeal posts, ticket panels, and
   honeypot warnings, and runs imported pending work; Discord reflects
   Rust's actions and PostgreSQL holds state MySQL does not.
10. **Verify**: `/readyz` is ready; `quack.jobs.ready` drains; spot-check
    three servers (`/case view` of the newest case, `/case history`, the
    dashboard case page, a member's appeal page from an old DM link, an old
    appeal button, the failed actions page); confirm `quack.actions.failed`
    and `quack.dm.outcomes` look normal.
11. **After step 9, recovery is roll-forward only**: fix Rust and redeploy.
    Never restart Go against MySQL: its case counters, deliveries, and
    actions are stale, and it would repeat or contradict what Rust did.
    Keep MySQL read-only for 30 days for reference, then archive the dump.

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
| `quack-core` | Rule validation (every problem code), escalation (proptest: monotonic level choice, first level at count 1, last level for large counts, window cutoff), capability derivation (owner, Administrator, Manage Server, moderator roles with and without deleted roles, fallback, 2FA), member checks in order, staff-channel check, every state machine (proptest: random event sequences never leave the transition tables; `done`, `cancelled`, `skipped`, `sent`, `unknown`, `not_needed`, `accepted`, `closed` stay terminal; the only way out of `failed` is a staff retry), `may_have_applied`, `controls`, `parse_case_marker`, context validation per kind (decimals at the 10^15 and 6-decimal edges), message link parsing and `find_message_links`, `redact_secrets`, `ulid_to_uuid` (order preserved, deterministic), u22 round trip. | Unit tests and proptest (regression seeds committed). |
| `quack-text` | Every message id and every listed variant, both surfaces (`Discord`, `Plain`); Markdown escaping; trimming to 4,000 characters; durations and plurals. | `insta` snapshots. |
| `quack-db` | `0001_initial.sql` applies to an empty database in one transaction; every CHECK, composite key, and trigger (`mod_log` and `rule_versions` reject UPDATE, DELETE, TRUNCATE; for `rule_versions` the test uses `TRUNCATE … CASCADE`, since a plain one fails on the foreign key first); job queue: 8 concurrent claimers never claim the same row, the lease sweep, a fenced transaction with a stale token rolls back its domain writes, a claim beyond the budget is finalized without running, coalescing and `rerun_at`, dead-job revival; the claim uses `jobs_runnable` (`EXPLAIN` with 20,000 leased rows). | `TestDb` (Postgres 18). |
| `quack-engine` | §20.1.1 scenarios. | `TestDb` + `FakeDiscord` (records calls, scripts outcomes per call, can "crash" a handler between steps). |
| `quack-discord` | Components V2 JSON for every `Msg` shape, including maximal inputs against the 40-component and 4,000-character budget; deferred callbacks carry only the ephemeral flag; custom ID encode/decode (proptest: round trip, ≤ 100 characters); every legacy shape of §11.5; REST outcome classification for every `RefusalKind` code, 429, 5xx, timeouts, connection refused (`NotSent`) and reset after sending (`Unknown`); command definitions (snapshot) and the sync diff; captured interaction and gateway payloads decode. | `insta`, proptest, `httpmock` on loopback with Twilight's proxy setting. |
| `quack-api` | OAuth login and callback against a mocked Discord (state single use, state consumed only with its browser nonce, error param, redirect_to canonicalization); session expiry and key rotation; CSRF and origin on every mutation including logout; rate limits; every error code's status and envelope; idempotency replay, reuse, scoping per actor and operation, and replay refused after a capability is lost; member isolation (another member's case, file, or appeal is 404; replacement links across members hidden); capability per route; static file rules; `openapi_is_current`. | `axum` `oneshot`, `TestDb`, `FakeDiscord`, `httpmock`. |
| Modules | Tickets: open adopts an existing thread after a lost create (active and archived); an open failure frees the slot and deletes a created thread; close resumes after a crash at each step; finalization on budget exhaustion; one live ticket per member. Server activity log: redaction, filtering and tombstones, edit diffing with known-empty text, uncached delete batching, drops counted. Honeypot: a settings change between hit and case fails the hits (`policy_changed`); burst joins one case; refusal keeps messages; staff and bots ignored; cleanup waits for evidence and is idempotent. | `TestDb` + `FakeDiscord`. |
| `quack-import` | A MySQL 8.4 container loaded with the Go schema (versions 3 and 5 ledgers) and fixtures covering every table and every mapping row in §19; the manifest rejects an unexpected column; dry run runs every constraint and keeps nothing; apply matches expected rows; uncertain executions and notifications become `unconfirmed`/`unknown` and no job runs them; re-run is a no-op; a changed source row is refused; checks catch planted inconsistencies. | Testcontainers MySQL + `TestDb`. |

#### 20.1.1 Engine scenarios (each one test)

`FakeDiscord` scripts outcomes per call and keeps the state Discord would
have (members, timeouts, bans with reasons, messages by nonce), so a test
can make a call succeed on Discord while its answer is lost. Each scenario
below is run at every crash boundary it names.

1. Two concurrent `apply_rule` calls for the same member and rule get
   consecutive case numbers and consecutive case counts.
2. Operation keys: the same key twice (sequential and concurrent) creates
   one case; a different request with the key is refused; two staff using
   the same key create two cases; a replay after the actor lost `moderate`
   is refused.
3. Each member refusal; Quack-caused refusals write `case_create_refused`,
   others write nothing.
4. Rule archived or edited between the cache read and the case transaction:
   archived → `rule_archived`; edited → the case uses the new version, or
   `rule_changed` when the version was pinned.
5. Kick with DM: the action waits for the DM; a failed DM still releases it;
   a DM stuck in `sending` delays it by at most 120 seconds.
6. Void against an action, on both sides of the start commit: void first →
   `cancelled`, no Discord call; start first → the undo is queued and runs
   only after the attempt settles. The ban applies on Discord but its answer
   is lost: the retry reads the ban (this case's marker) and records `done`;
   the void's undo reads the marker and unbans.
7. Crash after the attempt opens, before and after Discord applied it: the
   next run settles `interrupted` and retries, reading Discord first; 8
   unknown attempts end `unconfirmed`; a claim beyond the budget is
   finalized without any Discord call.
8. A handler that lost its lease: its settle transaction rolls back and
   leaves the newer state untouched.
9. Timeout: the end time is identical on every attempt; a longer current
   timeout is left alone (`no_change`); an attempt after the end fails with
   `timeout_window_passed`.
10. Kick: the member left and rejoined before the retry → `no_change`, no
    `DELETE` call.
11. Undo provenance: another valid case's ban is in force → `skipped`
    (`other_case_applies`); the timeout end differs → `skipped`
    (`changed_outside_case`); a ban without a marker → `skipped`; a ban with
    a voided case's marker → unbanned; nothing in place → `no_change`.
12. Staff controls: retry of an original on a voided case is refused; retry
    of an undo works; dismiss hides from the failed list; concurrent retry
    and dismiss → exactly one wins; buttons match `controls`.
13. DM: a crash while `sending` inside the window resumes with the same
    nonce and Discord holds one message; after the window → `unknown`; a
    refusal after an unknown create → `unknown`; a rate limit before any
    unknown create → back to `pending`; a void before the start →
    `not_needed`; after the start the DM proceeds.
14. Appeals: every legal transition, every illegal one (409); a submit
    racing a void never leaves an appeal on a voided case; an old
    information reply (stale version) is refused; accept voids with the
    fixed reason and keeps the decision reason on its event; reopen on a
    voided case is refused; two quick decisions send two DMs, each with its
    own event's text.
15. Tracked messages: a bump during a sync is synced again; a move during a
    create cleans up the created message; a private receipt past its token
    is `gone` and never created; a create whose answer is lost is not
    duplicated inside the window and is created once more after it.
16. Evidence: caps under concurrent adds; a failed item captured again on
    re-add; a crash between file copies resumes at the next file; deleted
    message, moderator without access (including a private thread), Quack
    without access, other server, file too large, unsupported or missing
    type, copy failure.
17. Drafts: double submit of a page; expired draft; another moderator's
    draft; rule edited mid-draft; the final apply fails transiently and Try
    again creates exactly one case.
18. Settings: stale `expected_version` with role lists → 409; moderator role
    change without `manage_staff_roles` → 403; channel validation uses the
    locked roles.
19. Context edits: two editors at the same revision → the second gets
    `context_changed`; links inside text become evidence.
20. Leaving and rejoining a server turns modules off; queued work for an
    uninstalled server settles by §16.5.

### 20.2 Development

Requirements: Rust 1.100 (pinned in `rust-toolchain.toml` with rustfmt and
clippy), Docker, Bun, `sqlx-cli 0.9.0`, `cargo-nextest`, and a separate
development Discord application with both privileged intents enabled and
`http://localhost:3000/api/auth/callback` registered as a redirect.

`compose.yaml` at the repository root (project name `quack-dev`, so it never
touches other Compose projects or their containers):

| Service | Image | Ports | Profile |
| --- | --- | --- | --- |
| `postgres` | `postgres:18` (pinned digest), database `quack`, user `quack`, password `quack`, named volume | `54418:5432` | default |
| `otel` | `grafana/otel-lgtm` (pinned digest): OTLP receiver, Tempo, Prometheus, Loki, Grafana | `4318:4318`, `3001:3000` | `otel` |

`.env` (not committed; `.env.example` is). Quack does not read it; export it
into the shell first (`set -a; . ./.env; set +a`) or use a tool such as
direnv:

```
QUACK_DATABASE_URL=postgres://quack:quack@localhost:54418/quack
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
  `quack serve` instance. Exactly one instance per Discord application acts
  at a time: the singleton fence (§17.5) makes a new instance wait until the
  old one has exited.
- Database role: run `migrate` as the schema owner and, preferably, `serve`
  as a role that owns nothing, with `SELECT, INSERT, UPDATE, DELETE` on the
  tables it changes and only `SELECT, INSERT` on `mod_log` and
  `rule_versions`.
- Probes: liveness `GET /livez`, readiness `GET /readyz`. Termination grace
  period ≥ 35 seconds.
- Discord application: privileged intents Server Members and Message
  Content (approval from Discord once the application is in 100 or more
  servers); OAuth redirect `<QUACK_PUBLIC_URL>/api/auth/callback`; install
  scopes `bot applications.commands`; bot permissions View Channel, Send
  Messages, Send Messages in Threads, Embed Links, Attach Files, Read
  Message History, Moderate Members, Kick Members, Ban Members, Manage
  Messages, Manage Channels, Manage Threads, Create Private Threads, View
  Audit Log. `GET /api/guilds/{guild_id}/health` reports what a server is
  missing.
- Backups: Postgres continuous archiving (point-in-time recovery) plus a
  daily `pg_dump`; restore drill every quarter.

**Health and monitoring.** Use the dashboards and alerts in §18.7.
`quack.jobs.ready`, `quack.jobs.oldest_ready.age`, and `quack.jobs.dead`
show background work; `quack.actions.failed` shows actions waiting for
staff.

**Incidents.**

| Situation | What happens | What to do |
| --- | --- | --- |
| Postgres down | `/readyz` fails; interactions answer `error.internal`; jobs stop; nothing committed is lost. | Restore Postgres. Quack resumes on its own. |
| Discord down or rate limiting | Jobs retry within their budgets; actions may end `retries_exhausted` or `unconfirmed`; log posts are retried for about an hour. | After recovery, staff review the failed actions page; check `unconfirmed` ones in Discord before retrying. |
| Dead jobs | Rows with `dead_at` in `jobs` (only for missing or malformed records), visible in `quack.jobs.dead`. | Inspect `last_error`; fix the cause; `UPDATE jobs SET dead_at = NULL, attempts = 0, run_at = now() WHERE id = …` is the only sanctioned manual edit, and safe because every handler re-checks its record's state first. |
| A stuck "in progress" | Cannot persist past the budgets in §16.5: every exhausted job is finalized. | Check `quack.jobs.oldest_ready.age` and `quack.job.lease_expired`; check the gateway and Discord status. |
| Two instances started | The second waits for the fence and stays not ready. | Stop the extra instance. |
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
decision, the appeal post, DMs); each module, including ticket thread
freezing and deletion; the log channel; private receipts after 15 minutes;
a ban and a timeout applied by hand in Discord, then voiding the case (the
undo leaves them in place); a forced lost response (block the network after
a request) for a DM and a log post, checking Discord's nonce returns the
first message; `SIGTERM` with jobs in flight, confirming nothing runs twice
after restart; a second instance waiting on the fence; and the full import
runbook (§19.12) on a production copy.

### 20.4 Known gaps

1. **One process, one shard.** Quack supports one gateway shard (about
   2,500 servers; `serve` refuses to start when Discord recommends more)
   and one acting instance (the singleton fence).
2. **2FA is only as fresh as the last dashboard sign-in.** Quack cannot read
   2FA with the bot token.
3. **Staff channel overwrites are not re-synced** when staff roles change;
   new moderator roles must be added to existing staff channels by hand.
4. **Discord permissions are not a way to take things back.** Quack checks
   that a staff channel is staff-only before each post, but appeal
   statements, evidence copies, transcripts, and log posts already in a
   channel stay visible to anyone who can see it later.
5. **Private ticket threads** are visible only to members added to them and
   to people with Manage Threads; a moderator without Manage Threads cannot
   open a ticket thread from the staff post.
6. **Server activity log is best effort.** Posts are in memory and lost on
   a crash or a full queue (counted).
7. **One duplicate post is possible after a long outage.** A message create
   whose answer was lost, retried after the 60-second nonce window, posts
   again: a staff post, a log channel post, an evidence copy, a public
   receipt, or a ticket welcome can appear twice. Member DMs never do: an
   unconfirmed DM is reported and not resent. Quack does not reconcile by
   reading channel history.
8. **A kick or ban DM is sent before the action.** It describes the
   decision; if the action then fails, the member was told about a decision
   Quack couldn't carry out (the case page shows the outcome).
9. **Native moderation is recognized only by what Discord shows.** An undo
   removes a timeout only when its end equals the case's, and a ban only
   when its reason carries the marker of a voided Quack case. A moderator's
   own timeout with exactly the same end, or a ban whose reason copies
   Quack's marker, is indistinguishable and would be removed.
10. **Overlapping timeouts.** When two valid cases time out the same member
    and the later one is voided while the earlier still applies, the
    current (later) timeout stays until it ends; Quack does not shorten it
    to the earlier case's end.
11. **A honeypot ban that deletes messages** can remove the post before
    Quack saves it as evidence (the default honeypot rule deletes nothing).
12. **Reconstructed rule versions are partial** (only the levels cases
    used).
13. **A worker that outlives its lease** (a Discord answer slower than the
    15-second margin) can land an older tracked-message edit after a newer
    one; the next bump corrects it.
14. **No live-server rehearsal yet** (§20.3).

### 20.5 Decisions for the owner

Each item is a choice between plausible behaviors. Overrule any by editing
this document first.

1. **Server key.** `guilds.guild_id` is the Discord server ID (`bigint`)
   instead of a UUID: it removes the internal-ID mapping layer and makes
   every tenant foreign key readable.
2. **Fixed retry budget.** 8 attempts per round over about 4.6 hours for
   every action replaces the per-level `max_retries` (0–10) setting; old
   values are dropped on import. Rationale: actions read Discord before
   acting, so a knob adds nothing but configuration.
3. **Level names dropped.** Levels are shown as "level N (starts at case
   M)".
4. **Rule slugs dropped.** Names must be unique among a server's active
   rules (case-insensitive); the importer renames clashes and reports them.
5. **New and changed limits** (for new writes; imported values over them are
   kept and reported): at most 10 levels per rule and 100 active rules per
   server (new, needed to bound caches and pickers); rule names 100
   characters (Go stored 191); description 500; void reason 1,000 (was
   unbounded); DM introduction 1,000 and footer 500 (were unbounded); file
   copies up to 10 MiB (was 25 MiB; Discord's base upload limit is 10 MiB);
   case numbers up to 2^53 − 1. Kept from Go: level thresholds up to
   1,000,000, decision reasons 2,000, statements and replies 4,000, message
   link input 4,000, invite codes with `_`.
6. **Decision reasons are always required**; the per-server
   `appeal_review_reason_required` switch is dropped. Discord modals are
   prefilled with default text.
7. **Appeal reversal offers are dropped.** Accepting voids the case, which
   queues guarded undo actions (§5.6). A skipped undo is final: whatever
   Discord shows then belongs to the case that still applies, whose own void
   removes it.
8. **Queued actions check only Quack's own access when they run.** Staff
   authority (including role hierarchy for a remove-timeout retry or undo)
   is checked when staff ask.
9. **Timeouts on members with Administrator are refused before the case
   exists** (new member check 10), instead of failing afterwards.
10. **A timeout whose fixed end has passed fails** with
    `timeout_window_passed` rather than counting as done, and Quack never
    shortens a longer timeout that is already in place.
11. **Every case DM describes the decision** ("The moderators decided to …")
    because kick and ban DMs are sent before the action and a timeout DM
    may arrive before Discord confirms it.
12. **DM failure reasons**: `discord_unavailable` for a DM channel that
    could not be opened or a DM Discord kept rate limiting; error 50007 is
    reported as "Discord didn't let Quack message them", not as "no shared
    server" (only 50278 says that).
13. **All staff reads are ephemeral**, including `/case view`, `/case
    list`, `/case history`, `/appeals`, and `/help` (some were public).
14. **Guided `/setup`** (one panel) replaces the subcommands `/setup
    appeals`, `audit`, `evidence`, `tickets`, `honeypot`, and `logging`.
15. **`/case add` has no `message_link` or `file` options** (the charter's
    option list). Evidence comes from the "Add case" message command, from
    `message_link` fields and links inside text answers, and from Add
    evidence. File uploads are no longer possible from Discord except as
    attachments of a linked message.
16. **Edit context in Discord is limited to rules with at most 5 fields**;
    larger ones are edited on the dashboard. The staff card does not page;
    long details continue on the dashboard.
17. **Prefilled-modal exception.** Edit context, Try again on a draft page,
    and the setup invite and warning modals authorize and read their one row
    in a single query (300 ms deadline) before responding with the modal,
    because Discord needs the prefilled values in the first response.
18. **Message-link auto-fill.** A case started from a message fills empty
    `message_link` fields with that message's link.
19. **Modals open only for rules with a required field**; optional context
    is added afterwards (kept from Go). A rule with no fields takes free-text
    context (`details`, kept from Go).
20. **Activity log defaults**: message text, file names (with links), and
    embed types on; per-event routing kept; secret redaction kept; cache
    size and delivery-attempt settings dropped (constants).
21. **Activity log posts are not durable jobs** (in-memory queue with drop
    counting), because they are high volume and best effort.
22. **Honeypot rule protection.** Archiving the honeypot's rule or making
    one of its fields required is refused, instead of the Go behavior of
    turning the honeypot off afterwards; the setting can be cleared while
    the honeypot is off. Quack never adopts an existing rule by its name.
23. **Honeypot ignores every bot** (cases are never about bots); refused
    hits keep their message; hits admitted under older settings fail
    instead of applying.
24. **Honeypot bait deletions** appear in the server activity log as
    ordinary deletions.
25. **Tickets**: one live ticket per member is a partial unique index (the
    slot table is gone); opening and closing are durable jobs that always
    finish; closing freezes (archives and locks) the thread, then deletes
    it; transcripts read at most 1,000 messages of history (was 10,000; the
    journal covers what Quack saw). The open reply does not wait for the
    thread. "Fix access" is kept and not logged.
26. **Moderation log scope**: reads and exports are not recorded; `dm_sent`
    and `evidence_added` are recorded but not posted; every imported Go
    entry is kept as `legacy` (§19.8).
27. **Log channel retries** run up to about an hour (6 attempts) instead of
    the Go schedule of 1 minute to 6 hours; past the nonce window a retry
    can post a duplicate (§20.4).
28. **Not imported**: `case_events`, `ticket_events`, `staff_members`,
    `audit_mirror_deliveries`, `ticket_member_states`,
    `honeypot_warning_refreshes`, `v4_import_*` (timelines are rebuilt from
    rows that hold the same facts; all counts are reported). Uncertain
    in-flight effects are imported as `unconfirmed`/`unknown`, and pending
    honeypot hits fail, rather than being replayed.
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
    `QUACK_DATABASE_MAX_CONNECTIONS` (sizing), `QUACK_DASHBOARD_DIR` (where
    the image puts the built files), `QUACK_ENVIRONMENT` (the
    `deployment.environment.name` resource attribute),
    `QUACK_TRUSTED_PROXY_HOPS` (client IP behind a proxy, default off), and
    `QUACK_IMPORT_SOURCE_URL` (the import's source, kept off command lines).
33. **The `Discord` port's implementation lives in the binary**
    (`DiscordPort`), because `quack-discord` may not depend on
    `quack-engine` where the trait is defined.
34. **Module HTTP routes live in `quack-api`**, calling each module's
    service functions, so modules never depend on the API crate.
35. **Span names** follow OTel semantic conventions for HTTP server, HTTP
    client (Discord), and database spans (`GET /api/…`, `PUT /guilds/…`,
    `UPDATE cases`) instead of the charter's `http …`, `discord …`, `db …`
    prefixes; the telemetry crate changes to match (§18.3).
36. **Operation-key replays** return the current, authorized
    representation of the original result (the stored reference), with the
    original status code, rather than a byte copy of the first response.
37. **Settings PATCH** requires `expected_version` only when it changes role
    lists.
38. **Legacy Discord buttons**: old appeal buttons in DMs, old appeal
    review buttons, and old ticket panel and ticket buttons keep working;
    old case buttons open the case card; everything else answers with
    `interaction.legacy_button`. Old receipts are not re-rendered; appeal
    posts, ticket panels, and honeypot warnings are.
39. **Dashboard paths** `/audit` → `/mod-log` and `/modules/logging` →
    `/modules/activity-log`, with redirects.
40. **Job order is FIFO per kind** with round-robin permits across kinds and
    no per-server fairness (actions are serialized per member).
41. **Server health** (`GET /api/guilds/{guild_id}/health`) replaces the
    per-server ops status and is available to `manage_settings` (was owner
    and Administrator only).
42. **Members see evidence** on their own case page (text and files, never
    authors, links, or message-link context values), reading the charter's
    "staff-only unless shown in the appeal view" as the member's case/appeal
    page.
43. **Members see the void reason** of their case (kept from Go).
44. **Receipts**: the public receipt shows only its fixed fields (member,
    rule, case number, the action line, the official reason) with one Open
    case button; a private receipt lives only as the interaction's
    ephemeral reply for 15 minutes. `/case add` always acknowledges
    privately and Quack posts the public receipt itself. The member command
    "Add case" posts a public receipt; the message command's receipt is
    private (kept from Go).
45. **Ticket transcripts are capped at 8 MiB** (marked partial when cut).
46. **Statistics** default to the calendar month before `to` (kept) and are
    computed in one read-only snapshot.
47. **Leaving a server turns its modules off**, and they stay off after
    Quack is added again (Go turned only the honeypot off).
48. **A kick acts only on the membership the case was about**; a member who
    left and rejoined is not kicked by a retry.
49. **Context numbers are exact decimals**, strings on the API.
50. **Setup channel creation adopts its own fresh channel** (same name,
    under 10 minutes old, with Quack's overwrite) instead of creating a
    second one.
51. **The dashboard CSP is the Go server's**, including the visitors.now
    analytics origins.

---

## Implementation notes

Where the built system differs from the text above, as of the Rust
integration pass. Each entry is *section → what the code does*; a designer
folds these into the sections (or changes the code) and removes them here.

**Architecture and crates**

1. §17.2, §11 (`quack-discord`) → REST goes through a small `reqwest`
   transport driven by Twilight's rate limiter (`twilight-http-ratelimiting`)
   instead of `twilight-http`: it can send string nonces with
   `enforce_nonce`, and it never retries on its own.
2. §17.2 (`quack-discord`) → `Gateway::run` yields Quack's typed `Event`s
   (not Twilight events), with an `on_dropped` hook. Fact reads go through
   `quack_discord::facts::Facts` (guild and member caches under §2's
   freshness rules, else REST), which the binary's `DiscordPort` uses.
3. §13.1.6, §17.2 → there is no `Module` trait, `ModuleSpec`, `Ctx`, or
   `ModuleEvent` in the engine. Each module crate exports plain service
   functions, `jobs() -> ModuleJobs` (job kinds and one handler), and
   `hooks() -> ModuleHooks` (enable check and change writes for
   `settings::set_enabled`). The bot holds the module seam:
   `quack_bot::Module` (component ids it owns, gateway events it wants), with
   one adapter per module crate in `quack_bot::modules`.
4. §11.8, §13.1.4 → the module areas of `/setup` are rendered and handled by
   the bot's setup handler, which calls each module crate's settings
   functions, `use_default_rule`, `route_all`, and `set_enabled` with its
   hooks; channel creation ("Create …", including the public ticket entry
   and honeypot channels with their overwrites) lives in `quack-bot`, not
   `quack_engine::setup`. Each module change reloads the config cache at
   once so the re-rendered panel is current.
5. §17.2 → `Responder` (interaction callbacks, edits, follow-ups) lives in
   `quack-bot`; `quack-bot` depends on `quack-db` (draft and 2FA reads).
6. §17.5, §17.2 → `quack::compose::App::compose` builds the engine over a
   `Discord` port, the modules, the bot, and the HTTP application; `serve`
   uses it over the REST client and the whole-system tests over the fake.
   Startup also runs `quack_honeypot::recover`, starts the activity log's
   senders, the open-tickets gauge, and the journal-gap recorder; shutdown
   calls `ActivityLog::shutdown` within the drain.
7. §17.1 → `quack serve --discord-offline <url>` runs without the gateway
   or any startup call to Discord and sends REST calls to `<url>` (tests and
   smoke runs; the image's smoke run uses it).
8. §17.2 (`quack-testkit`) → `TestDb` uses the PostgreSQL server named by
   `QUACK_TEST_DATABASE_URL`, which must be set (no fallback, so tests never
   create databases on a server they guessed), and copies a migrated
   template database per test, not Testcontainers. The testkit also has an engine-agnostic queue driver
   (`settle_with` and friends).
9. §17.2 → the engine exposes `cases::staff_card` (the one staff card
   builder for receipts, `/case view`, and Open case),
   `cases::{actions, action}` (route 28 and control answers, built from the
   case's records), `guilds::{installed_among, summary}` (routes 5 and 6),
   and `evidence::resolve_file` (routes 26 and 53). `void_case` returns the
   void's effects (`VoidEffects`, the member, the rule version's name), so
   the bot doesn't read the case back.

**Discord surface**

10. §11.6, §11.3 → `/case add`'s first form page writes its draft row
    (member, rule version, receipt choice, channel) with `drafts::start`
    *before* it opens, under the prefilled-modal exception (bounded to
    300 ms, else `interaction.busy` and no form), so a submit never finds
    the row missing, however fast or after a crash (application review
    MAJOR 2); the page's submit reads it. There is no
    in-memory form map, so a restart doesn't expire an open form. Saving a
    page of a draft that already became a case answers with that case.
11. §11.9 → a dropped `MESSAGE_CREATE` only remembers its channel in a
    bounded set (10,000) in the shard loop; a task records the ticket's
    `journal_gaps`. The ticket journal tells threads from channels with the
    guild cache and looks a thread up in the database only when the cache
    doesn't know the channel.
12. §13.4.2 → a honeypot post's actor facts come from the message's role
    ids and the guild's facts; without the facts only the roles are known,
    and the case job checks the poster again.
13. §13.3.2 → Quack's own account (for "never posted") comes from `READY`,
    else from the guild's facts.
14. §11.10 → the dashboard's directory cache is not implemented.

**Engine and jobs**

15. §5.6 → the undo's provenance check for an unban accepts a ban whose
    marker names this case or any voided case of the member with a ban
    action; the "left in place" wording names the case still applying.
16. §5.9 → a `done` ban recorded `no_change` still queues its unban on void
    (the provenance check decides), and so does a `done` timeout (the code
    now follows the table: "timeout with its end in the future").
17. §6.4 → a DM found `sending` after a crash checks the DM channel's
    history for its nonce (`SendHistory`) and records it `sent` when the
    message is there, else `unknown`.
18. §13.2.4 step 2 → the close job doesn't wait for the server's event
    queue (up to 5 s) before reading history. `complete` is true when the
    history reached the first message *or* the journal has no gaps.
19. §16 → the runner fences leases on the database's real clock
    (`clock_timestamp()`), bounds a claim by a deadline, sleeps through idle
    passes (a handful of queries a second), hands kind permits out first in,
    first out, and retries a failing finalize `FINALIZE_TRIES` (3) times
    before keeping the job dead.
20. §9.2, §12.10 → `ActionFailedDetail` carries `may_have_applied` (absent
    in older entries, read as false); `log.action_failed` then starts with
    "Quack couldn't confirm whether Discord …".
21. §14.1 → every list fetches one row more than the page
    (`quack_db::Page { rows, more }`), so `next_cursor` is null on the last
    page even when it is exactly full.

**HTTP API**

22. §14.3 → the timeline and appeal summary wire shapes are those of the
    committed `contracts/http/openapi.json`, which differ in detail from the
    TypeScript sketch in §14.3.
23. §14.3 (routes 41–46) → each module page carries its settings'
    `version`, and the update bodies (`TicketSettingsUpdate`,
    `ActivityLogSettingsUpdate`, `HoneypotSettingsUpdate`) require it back
    as `expected_version`; a newer save is `409 settings_changed`, as with
    the core settings `PATCH`. `ModuleState.problems` is empty on the module
    pages (it is filled only by `409 module_not_ready`).
24. §14.3 (route 43) → `posted_since_start` and `dropped_since_start` are
    per server, in process.
25. §14.3 (routes 47–50) → a ticket's `transcript` summary comes from its
    saved transcript row; once housekeeping deletes an expired transcript
    it is `null` rather than `expired`. Closing takes the operation key
    (the `Idempotency-Key`, or the button's interaction), scoped to the
    server and account and hashed with the ticket: a replay answers `200`
    with the ticket, another key for the same ticket is
    `409 ticket_not_open`, and the key reused for another ticket is
    `idempotency_key_reused` (application review MAJOR 5).
26. §14.8 → the 10-second request deadline applies to `GET` and `HEAD`
    only; the header-read and keep-alive timeouts are axum's defaults
    (`axum::serve` doesn't expose them).
27. §14.9 → the API checks the database itself on `/readyz` (1 second);
    the other checks are the binary's readiness flags.
28. §14.6, §12.3 → invalid IDs and filter values answer
    `api.value_invalid`; the statistics range, rule-file format, reason, and
    appeal text problems have their own messages (§12.3, §12.14); dashboard
    pages use `error.case_not_found_dashboard` and
    `error.appeal_not_found_dashboard`; adding evidence with no link is
    `error.no_links`.

**Data and import**

29. §15.10 → migration `0003_historical_rows.sql` relaxes four constraints
    for imported history: a `sent` DM may have no channel (it needs
    `first_attempt_at`; only `sending` needs the channel), a dismissal may
    lack `dismissed_by`, a Discord or dashboard case may lack its moderator
    when its operation key is absent or `legacy:` (Quack's own cases always
    have a key), and `tickets.entry_channel_id` may be NULL except while
    `opening`. Text renders an unrecorded dismisser as "A moderator".
30. §19.7, §19.10 → the importer keeps a Go DM `sent` when it knows the
    message (the channel may be unknown); `unconfirmed_dm` is only a sent
    row without its message. It keeps an unattributed dismissal
    (`dismissed_by_unknown`), imports a Discord or dashboard case without
    its moderator (`case_moderator_unknown`) instead of refusing the
    server, and leaves a ticket's entry channel unknown without ticket
    settings (`ticket_entry_channel_unknown`) instead of guessing it.
31. §19 → other importer deviations reported by its implementer: evidence
    copies become jump links to the copy message, Go-uploaded files are
    imported as files of the copy message, the action-state mapping and the
    rule reconstruction follow the cases the Go data actually has, a void's
    state comes from its case events, appeal staff posts are imported as
    tracked messages, and the honeypot warning keeps the Go warning channel.
    Imported rule versions hold at most 32,767 levels.
32. §20.3 (packaging) → the image is distroless `cc-debian13` (glibc, for
    aws-lc and rustls), nonroot, 138 MB; Compose's `telemetry` profile runs
    `grafana/otel-lgtm` (OTLP/HTTP on 4318, Grafana on 3001).
33. §18 → `error.type` gains `decode` (a row Quack couldn't read back) and
    `quack.discord.outcome` gains `not_sent` (a request that never left
    Quack); static labels have an inherent `as_str`.
34. §5.6, §5.5, §16.1 (engine review BLOCKER 1) → an undo learns whether
    its original's change is **unresolved**: the original never reached
    `done` and one of its attempts may have applied (open, `unknown`,
    `interrupted`). While it is, an undo read that finds no ban (unban) or
    no timeout ending in the future (remove timeout) is not `no_change`:
    the attempt is recorded `not_sent` and the undo reads again on the
    normal backoff, removing the ban or timeout once it shows. If the round
    runs out first, the undo is `failed` with the new reason
    `original_unconfirmed` (`action.failure.original_unconfirmed`, a
    PostgreSQL enum value added by migration `0004`), which staff see on
    the failed-actions page and can retry. So an absent effect never
    finishes an undo while the original's request may still land.
35. §6.4 steps 3 and 5 (engine review BLOCKER 2) → the DM window is
    checked right before every create (after the render and after every
    wait, however late the task wakes) against the persisted
    `first_attempt_at` on the database's clock (`dms::send_window_open`),
    not the process clock. Past it: `unknown` when a create may have reached
    Discord, else back to `pending`.
36. §7.3 (engine review MAJORs 5, 16, 17) → the evidence channel is read
    from settings and checked staff-only again right before **every**
    upload, not once per run; a channel that stops being staff-only gets no
    further upload and the item's pending files fail with
    `copy_skipped_not_staff_only`. Each upload try is reserved durably
    (`evidence_files.copy_attempts`, migration `0005`) under the case and
    item locks with the job's fence, so the three-try budget holds across
    runs; a rate-limited or unsent upload gives its try back and a rate
    limit ends the run with Discord's delay. A run starts a download only
    with room for the download, the destination check, the upload, and a
    5-second margin before its deadline, else it continues in a new run.
37. §13.2.3, §10.4 (Discord review BLOCKER 2, MAJOR 13) → the ticket
    thread lookup returns only **private** threads (type 12) of the server
    under the entry channel, with the exact name, **created by Quack's
    account** (`owner_id`); the open job adopts the oldest and deletes the
    other matches, so it never adopts a public thread or deletes one Quack
    didn't make. Archived pages (newest archived first) are read until
    Discord has no more or a page reaches threads archived before the
    ticket was created; hitting the 10-page cap first, or a page with more
    to come but no cursor, is `Unknown` and the job retries instead of
    creating a thread.
38. §14.2, §14.3 (dashboard needs) → wire changes: `MemberAppealEvent`
    has `closed_by_void`; route 47's `state` takes a comma-separated set
    (`opening,open,closing`); `RuleSummary` has `appealable`,
    `counting_window_days`, and `levels`; `Health.channels` lists the
    channels of the modules that are on (`tickets.queue_channel_id`,
    `tickets.entry_channel_id`, `activity_log.routes` once per channel,
    `honeypot.channel_id`) with `staff_only` (the ticket entry and honeypot
    channels are only checked for Quack's access); route 23 answers
    `CaseVoided {case, message}` and route 25 `EvidenceAdded {case, new,
    retrying, already_saved, message}`, whose `message` is the bot's
    sentence (`case.void_done`, `evidence.added`); `Case.imported` is the
    typed `ImportedCase {v4_action, v4_moderator, v4_reason,
    v4_source_id}`; the honeypot page's `eligible_rules` carry each rule's
    `levels`, so an administrator without rule access can preview the
    warning.
39. §11.2 (Discord review MAJORs 6, 7) → command definitions are shaped per
    route before they are compared or sent: a guild overwrite (the
    development server) leaves out the global-only `contexts` and
    `integration_types`, and the User and Message "Add case" commands leave
    out `description`. A modal's selects are sent without `disabled`, which
    Discord forbids in modals.
40. §5.1 ban, §5.2 (engine review MAJOR 13) → a ban sends its
    `delete_message_seconds` only when no earlier attempt of the action may
    have reached Discord; a retry after a lost answer or a crash sends 0, so
    messages are deleted at most once even if the ban was lifted outside
    Quack in between (a missed deletion is preferred to a second one).
41. §17.3 outcomes (Discord review MAJOR 5) → only a 4xx (other than 429)
    or Discord's documented 304 is `Refused`; any other non-success answer
    (a redirect from an intermediary, an undocumented status) is
    `Unknown`, like a 5xx. Modify Guild Member, Modify and Delete Channel,
    and the interaction webhook edits are `Done` only when their success
    body is a JSON object; otherwise `Unknown(Malformed)`.
42. §10.3 step 5 (engine review MAJOR 9) → an earlier location is removed
    only when its cleanup is `Done` or Discord says the message or channel
    is gone; any other refusal keeps it in `stale`, writes
    `last_error = stale_cleanup_<code>`, and goes on with the others, so a
    later sync tries again (for example once access is back).
43. §11 REST, §17.3 (Discord review MAJOR 3) → a 429 whose scope is global
    (`X-RateLimit-Scope: global`, `X-RateLimit-Global`, or `"global": true`
    in the body) holds every bot-authenticated request back until its
    `retry_after` has passed: each such request checks before and after its
    route permit, waits when the end is within the rate-limit budget, and
    otherwise answers `RetryAfter` without sending. Interaction callbacks
    and webhooks are not held.
44. §13.3.2 (application review BLOCKER 1, MAJOR 7, MINOR 2) → the
    activity log keeps no destination cache: before **every** try of a
    post (the first and each retry) it reads the route and privacy flags
    from the config cache and checks the destination is staff-only from
    current facts, failing closed when the facts are unavailable. The post
    is rendered with the flags as they are then, so a flag turned off
    while a post waited hides its text, file names, or embed types.
    Leaving a server also retires its posted/dropped counters. A
    permission change can still race the REST call itself.
45. §17.5 step 4 and shutdown (application review BLOCKER 2, MAJOR 12) →
    the REST client is built before the fence (building it calls
    nothing), and the fence is watched from the moment it is acquired,
    startup included. Losing it **closes the Discord client to new calls
    at once** (a call still waiting for its rate-limit permit is not sent
    either), then runs the shutdown sequence, whose drain can no longer
    start a Discord call; the process exits 1. A gateway that ends on its
    own (a terminal close such as 4014) also runs the shutdown sequence
    and exits 1, so a supervisor starts a working process instead of one
    that is live but never ready.
46. §19.7, §19.9 (application review BLOCKER 3, MAJOR 14) → whether an
    execution may have applied is decided from **every** source attempt
    before any is filtered: an attempt with no `finished_at` is
    `interrupted` unless it succeeded, an attempt with an invalid number
    is not written but still counts, and a failed attempt is `not_sent`
    or `refused` only for a known Go error class that shows it (an
    allowlist); a missing or unrecognized code is `unknown`. So a
    `pending`/`retrying` execution after such an attempt imports as
    `failed` (`unconfirmed`) with no `run_action` job. A timeout duration
    outside 1 second to 28 days has no safe end: the case's creation is
    used and `timeout_without_duration` is reported (the importer never
    panics on it).
47. §17.7 (application review MAJOR 17) → a config file that isn't valid
    TOML is reported by its path and the parser's line and column only;
    the parser's own message (which quotes the offending line, possibly a
    token) is never printed.
48. §11.9, §11.3 (application review MAJOR 1, Discord review MAJOR 9) →
    the interaction pool runs up to **256** handlers at once, and the bot
    lets at most **64** of them past their acknowledgement
    (`quack_bot::MAX_WORK`): a handler acknowledges first, then waits for
    a work slot. So slow work holds up other interactions' work, never
    their acknowledgement, until 256 are in flight. Quack doesn't drop an
    interaction by its snowflake's age (a skewed clock would drop every
    one); a late acknowledgement is still logged.
49. §14.10 (application review MAJOR 6) → a dashboard file (or the SPA's
    `index.html`) is served only when its real path, with symlinks
    resolved, is inside the real `QUACK_DASHBOARD_DIR`; a symlink that
    leads out is a 404.
50. §13.4.2 steps 3.3 and 4 (application review MAJORs 8, 9) → a burst
    attach (step 3.3) checks, under the same `FOR SHARE` lock on
    `honeypot_settings` that case creation takes, that the module is still
    on with the version, channel, and rule the hits were admitted under;
    otherwise the hits fail `policy_changed` and their messages are kept.
    The cleanup deletes a post only when the case's evidence for it is
    `saved` (a burst's later posts have no evidence row and are deleted
    as before); a capture that `failed`, or is still `saving` after 10
    minutes, records `cleanup_failed_at` and keeps the post.
51. §19.10–19.12 (application review MAJOR 13) → a server whose checks
    don't all hold is rolled back before its `import_guilds` marker
    (reported `Failed` with the failing checks and their example keys), so
    nothing of it is runnable and a repaired rerun imports it. A case whose
    moderator ID is not a snowflake is written without its moderator and
    reported under `derived` as `case_moderator_invalid` (it was counted as
    a skipped case, which failed `every_case_accounted_for`).
52. §19.4, §19.9 (application review MAJOR 15) → as the `appeal_notifications`
    row of §19.4 says, an appeal the Go bot never posted for gets an
    `appeal_post` with no channel and no job even while it is open
    (reported `appeal_post_unposted`); it is bound when the appeal channel
    is next saved. Only a known existing post is re-rendered at cutover.
53. §17.5 shutdown (application review MAJOR 16) → a signal during startup
    (while waiting for the fence, say) stops it and `serve` exits 0. At
    the drain cutoff, tasks still running are aborted rather than left
    detached, and closing the pool and the fence connection gets only what
    is left of the 30-second deadline. Telemetry is flushed after that
    with at most 5 seconds (hence the 35-second grace period).
54. §11.9 dispatch (application review MAJOR 3) → `READY`, `GUILD_CREATE`,
    `GUILD_DELETE`, and `CHANNEL_DELETE` go to their own ordered queue of
    4,096 (`quack.queue.*{gateway.lifecycle}`), so a flood of messages
    never drops a server joining or leaving or a channel's removal; they
    are ordered among themselves, not against a server's other events.
    `READY`'s "missing servers left" pass goes on past a server it couldn't
    update (logged).
55. §13.2.4 steps 1–2 (application review MAJOR 10, Discord review
    MAJOR 15) → the freeze counts as done only on success or a gone
    thread. An archived thread (Discord's 400 for changing one) is
    unarchived and locked in one request, then archived; any other
    refusal is retried, and the job's finalization keeps the thread
    (`thread_delete_failed`) and closes with a partial transcript. A
    message sent before `frozen_at` whose event arrives after the freeze
    is still journaled until the transcript is saved.
56. §10.3 (engine review MAJOR 7) → when the channel itself is gone or
    unreadable (Unknown Channel, Missing Access, or the facts say it is
    missing) and it is still where the message belongs, the sync ends with
    `last_error = destination_unavailable` instead of posting "again" in
    the same channel under a new generation every run; the next bump (a
    settings change, say) tries again.
57. §10.3 (engine review MAJOR 8) → `MESSAGE_DELETE` in a server calls
    `messages::message_deleted`: when the message is a tracked row's
    current location (same generation), the §10.3 table's plan runs in
    one transaction (post again where it belongs, wait for a channel, or
    `gone`) and its sync is queued; the gateway handler does one indexed
    lookup per delete.
58. §4.2 step 2, §4.3 (engine review MAJOR 20) → the request hash of a
    case creation or context edit takes the context sorted by field key,
    each answer tagged with its kind (JSON, number, text), so the same
    request with its fields in another order replays, and `5` and `"5"`
    are different requests.
59. §9.4 (engine review MAJOR 19) → a `post_mod_log` job reads the log
    channel again right before the create, after its staff-only check and
    render, and posts nothing when it changed; `log_channel_posted_at` is
    stamped only while the pinned channel is still the log channel. A
    switch committing after that read can still race the request itself.
60. §4.2 steps 7 and 22 (engine review MAJOR 21) → the 3-second deadline
    starts before the operation key's reservation (which can wait on a
    concurrent request holding the same key), the reservation goes
    through the same contention retry, and COMMIT gets exactly what is
    left (none left: it is never sent, `Deadline`).
61. §5.8 undo, §5.6 (engine review MAJOR 15) → the undo control is
    offered only when the undo guard, read under the write's locks as the
    undo's start would, lets it run: with another valid case's timeout or
    ban still in force the write is `409 action_not_undoable` instead of
    creating an undo that is then skipped (a property test checks
    "offered ⇔ the undo lifts it"). A void-driven undo still runs and is
    skipped with its reason (§5.6). A manual undo of a valid case's own
    ban passes the provenance check because the marker names that case
    (note 15); a voided marker is required only for other cases.
62. §16.1 (engine review MAJOR 16, the rest) → before opening an action
    attempt (a read and a write), before a tracked message's write (staff
    check, write, confirmation), and before each stale cleanup, the run
    checks it has 10 seconds per call plus a 5-second margin left before
    its deadline; without it, it ends `Continue` (nothing opened, nothing
    counted) and a new claim starts over, so the hard deadline never cuts
    off a mutation in flight.
63. §4.11, §10.2 (engine review MINOR 31) → an action's start
    (`running`) and a retry's settlement back to `pending` don't bump the
    case's receipt: every surface renders `pending` and `running` with the
    same in-progress outcome, the receipt's sentence ("Quack will …" /
    "Quack is …") is true in both, and the retry time and reason are
    staff detail that `/case view` and the dashboard read live; terminal
    transitions bump as before. The member's timeline dates a skipped
    undo by its `action_undo_skipped` entry, not by when it was requested.
64. §2, §7.2, §11.10 (Discord review MAJOR 12) → one permission
    calculation, `quack_core::permissions::in_channel`, serves the Discord
    facts and evidence access (the engine's copy is gone). In a thread it
    uses the parent's overwrites and Send Messages in Threads as the send
    permission; the timeout restriction comes after every overwrite (so an
    overwrite can't give a timed-out member Manage Threads back); a thread
    whose parent is unknown gets no permissions (fail closed). Voice
    channels' Connect rule isn't applied: Quack never posts in them.
65. §2, §11.10 (Discord review MAJOR 10) → the member cache keeps a
    revision per server that every gateway member change (add, update,
    remove, clear) bumps. A REST read or search notes it before the
    request and writes its answer (a member, or its absence) only if no
    change came in between, under the same lock, so a late answer never
    resurrects a departed member or erases a rejoin.
66. §11.9, §11.10 (Discord review MAJOR 11) → a gateway frame Twilight
    can't read distrusts the guild cache: an unreadable event other than a
    message or interaction may have been a cache update a resume won't
    replay, so the cache is untrusted until the next `READY` (facts come
    from REST meanwhile); a compression or reconnect failure marks the
    session disconnected until it resumes; the shard stopping untrusts the
    cache too. (A transport EOF that Twilight resumes on its own is not
    visible to Quack; the resume replays what was missed.)
67. §11.3, §11.5 (Discord review MAJOR 8) → an interaction Twilight can't
    read even with defaults for missing fields is answered from its
    envelope (id, token, type, server, channel) as
    `UnknownReason::Malformed`: the expired-button answer, or for an
    autocomplete an empty choice list (type 8), instead of never being
    acknowledged.
68. §17.3 Edit Message (Discord review MAJOR 14) → every Components V2
    edit of a channel message also sends `"content": null` and
    `"embeds": []`, so an edit that converts a message posted before V2
    (the Go bot's appeal posts, re-rendered at cutover) is accepted.
    Interaction webhook edits are unchanged: they only ever edit Quack's
    own V2 responses.
69. §11 REST (Discord review MAJOR 4, in part) → the rate limiter buckets
    a route by its path with only the major ids (channel, server, webhook)
    kept and every other id replaced, so after one message route's bucket
    is learned, another message of the same channel waits in it instead
    of going out through an unmapped endpoint. Publishing a shared or
    body-only `retry_after` as a cooldown to queued peers is not done:
    those requests still answer `RetryAfter` with the right delay, but a
    peer queued behind them may be sent and get its own 429.
70. §17.3 outcomes (Discord review MINOR 17, in part) → an `Unknown …`
    JSON code (10003, 10004, 10007, 10008, 10013, 10026) is read as
    "absent" only with its documented 404; with another status it is an
    `Other` refusal, so a 400 carrying 10007 is never "the member left".
