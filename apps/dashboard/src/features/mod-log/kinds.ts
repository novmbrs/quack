import type { ModLogKind, ModLogResult, ModLogSource } from "~/api/types";
import type { QuackIconName } from "~/ui/QuackIcon";

/** KindGroup is a heading in the moderation log's kind filter. */
export type KindGroup = { label: string; kinds: { value: ModLogKind; label: string }[] };

/**
 * kindGroups names every moderation log kind (design §9.2) for the filter.
 * The entries themselves use the server's sentence (`text`), never these.
 */
export const kindGroups: KindGroup[] = [
  {
    label: "Cases",
    kinds: [
      { value: "case_created", label: "Case added" },
      { value: "case_create_refused", label: "Case refused" },
      { value: "case_voided", label: "Case voided" },
      { value: "case_context_edited", label: "Case context edited" },
      { value: "evidence_added", label: "Evidence added" },
    ],
  },
  {
    label: "Actions",
    kinds: [
      { value: "action_done", label: "Action done" },
      { value: "action_failed", label: "Action failed" },
      { value: "action_retry_requested", label: "Action retried" },
      { value: "action_dismissed", label: "Failed action dismissed" },
      { value: "action_undo_requested", label: "Action undone" },
      { value: "action_undo_skipped", label: "Undo left in place" },
    ],
  },
  {
    label: "DMs",
    kinds: [
      { value: "dm_sent", label: "DM sent" },
      { value: "dm_failed", label: "DM not sent" },
    ],
  },
  {
    label: "Appeals",
    kinds: [
      { value: "appeal_submitted", label: "Appeal submitted" },
      { value: "appeal_information_requested", label: "More information requested" },
      { value: "appeal_information_provided", label: "Member replied" },
      { value: "appeal_accepted", label: "Appeal accepted" },
      { value: "appeal_rejected", label: "Appeal rejected" },
      { value: "appeal_closed", label: "Appeal closed" },
      { value: "appeal_reopened", label: "Appeal reopened" },
    ],
  },
  {
    label: "Rules",
    kinds: [
      { value: "rule_created", label: "Rule created" },
      { value: "rule_edited", label: "Rule edited" },
      { value: "rule_archived", label: "Rule archived" },
      { value: "rule_restored", label: "Rule restored" },
      { value: "rule_imported", label: "Rule imported" },
    ],
  },
  {
    label: "Server and settings",
    kinds: [
      { value: "settings_changed", label: "Settings changed" },
      { value: "module_enabled", label: "Module turned on" },
      { value: "module_disabled", label: "Module turned off" },
      { value: "module_settings_changed", label: "Module settings changed" },
      { value: "channel_setting_cleared", label: "Channel setting cleared" },
      { value: "log_channel_failed", label: "Log channel posting stopped" },
      { value: "quack_joined", label: "Quack joined" },
      { value: "quack_left", label: "Quack left" },
    ],
  },
  {
    label: "Tickets and honeypot",
    kinds: [
      { value: "ticket_opened", label: "Ticket opened" },
      { value: "ticket_closed", label: "Ticket closed" },
      { value: "honeypot_case_failed", label: "Honeypot case not added" },
    ],
  },
  { label: "Earlier versions", kinds: [{ value: "legacy", label: "Imported entry" }] },
];

const allKinds = new Set<string>(kindGroups.flatMap((g) => g.kinds.map((k) => k.value)));

/** parseKind reads a kind from search params, dropping anything unknown. */
export function parseKind(value: unknown): ModLogKind | undefined {
  return typeof value === "string" && allKinds.has(value) ? (value as ModLogKind) : undefined;
}

export const sourceLabel: Record<ModLogSource, string> = {
  discord: "Discord",
  dashboard: "Dashboard",
  quack: "Quack",
  imported: "Imported",
};

/** resultLabel names a result; entries that went fine show no badge. */
export const resultLabel: Record<ModLogResult, string> = {
  ok: "Done",
  failed: "Failed",
  refused: "Refused",
};

const results = new Set<string>(["ok", "failed", "refused"]);

/** parseResult reads a result filter from search params. */
export function parseResult(value: unknown): ModLogResult | undefined {
  return typeof value === "string" && results.has(value) ? (value as ModLogResult) : undefined;
}

/** iconFor picks an entry's icon from its kind, or its result when it didn't go through. */
export function iconFor(kind: ModLogKind, result: ModLogResult): QuackIconName {
  if (result === "failed") return "error";
  if (result === "refused") return "warn";
  if (kind === "case_voided") return "case_void";
  if (kind === "case_created") return "case_add";
  if (kind === "evidence_added") return "evidence";
  if (kind.startsWith("case_")) return "case";
  if (kind === "action_retry_requested") return "retry";
  if (kind.startsWith("action_")) return "success";
  if (kind.startsWith("dm_")) return "message";
  if (kind.startsWith("appeal_")) return "appeal";
  if (kind.startsWith("rule_")) return "note";
  if (kind.startsWith("ticket_")) return "ticket";
  if (kind.startsWith("module_") || kind.endsWith("_changed")) return "settings";
  if (kind === "quack_joined") return "join";
  if (kind === "quack_left") return "leave";
  return "history";
}

/** ModLogSearch is the page's filter in the URL. */
export type ModLogSearch = {
  kind?: ModLogKind;
  result?: ModLogResult;
  actor?: string;
  member?: string;
  case?: number;
  rule?: string;
  from?: string;
  to?: string;
};

/**
 * filtersOf turns the URL filter into API query filters. Dates are whole
 * UTC days and "to" includes its day, so created_before is the next midnight.
 */
export function filtersOf(q: ModLogSearch) {
  return {
    kind: q.kind ? [q.kind] : undefined,
    result: q.result,
    actor_id: q.actor,
    member_id: q.member,
    case_number: q.case,
    rule_id: q.rule,
    created_after: q.from ? `${q.from}T00:00:00.000Z` : undefined,
    created_before: q.to ? nextDay(q.to) : undefined,
  };
}

function nextDay(day: string): string {
  const d = new Date(`${day}T00:00:00.000Z`);
  d.setUTCDate(d.getUTCDate() + 1);
  return d.toISOString();
}
