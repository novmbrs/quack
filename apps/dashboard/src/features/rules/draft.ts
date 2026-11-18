import type { ApiError } from "~/api/client";
import type { FieldKind, LevelAction, RuleDefinition, RuleFile } from "~/api/types";
import { capitalize, levelOutcome } from "~/lib/format";

// Limits from the rule validation in design §3.3. The editor checks them
// first so problems show next to the field instead of after a round trip;
// the API checks them again and its field errors map back with apiIssues.
export const limits = {
  name: 100,
  description: 500,
  reason: 1000,
  fieldLabel: 100,
  fields: 10,
  levels: 10,
  windowDays: 36500,
  caseCount: 1_000_000,
  timeoutSeconds: 28 * 86400,
  deleteSeconds: 7 * 86400,
} as const;

/** keyPattern is the shape of case context field keys. */
export const keyPattern = /^[a-z0-9][a-z0-9_-]{1,63}$/;

/** ActionChoice is a level's action, or none for a warning. */
export type ActionChoice = "none" | LevelAction["kind"];

/** FieldDraft is a case context field being edited. id is local to the editor. */
export type FieldDraft = {
  id: string;
  label: string;
  key: string;
  /** Whether the rules manager typed the key, which stops deriving it from the label. */
  keyEdited: boolean;
  kind: FieldKind;
  required: boolean;
};

/** LevelDraft is a level being edited. id is local to the editor. */
export type LevelDraft = {
  id: string;
  /** The case count the level starts at. The first level always starts at 1. */
  from: number;
  sendDm: boolean;
  action: ActionChoice;
  timeoutSeconds: number;
  deleteSeconds: number;
};

/** Draft is the editor's working copy of a rule definition. */
export type Draft = {
  name: string;
  description: string;
  reason: string;
  appealable: boolean;
  /** Whether only recent cases count toward the case count. */
  windowed: boolean;
  /** How many days of cases count while windowed. */
  windowDays: number;
  fields: FieldDraft[];
  levels: LevelDraft[];
};

let nextId = 0;
/** uid makes a key for a row being edited. */
export function uid(): string {
  nextId += 1;
  return `d${nextId}`;
}

/** slugify turns a label into a field key: "Message link" → "message_link". */
export function slugify(text: string): string {
  return text
    .normalize("NFKD")
    .replace(/[̀-ͯ]/g, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "_")
    .replace(/^_+|_+$/g, "")
    .slice(0, 64)
    .replace(/_+$/, "");
}

/** newLevel is a fresh level that starts at the given case count. */
export function newLevel(from: number): LevelDraft {
  return {
    id: uid(),
    from,
    sendDm: true,
    action: "none",
    timeoutSeconds: 86400,
    deleteSeconds: 0,
  };
}

/** newField is a blank short-text case context field. */
export function newField(): FieldDraft {
  return { id: uid(), label: "", key: "", keyEdited: false, kind: "short_text", required: false };
}

/** emptyDraft is the starting point for a new rule: one level, a warning from the first case. */
export function emptyDraft(): Draft {
  return {
    name: "",
    description: "",
    reason: "",
    appealable: true,
    windowed: false,
    windowDays: 90,
    fields: [],
    levels: [newLevel(1)],
  };
}

function levelDraft(level: RuleDefinition["levels"][number]): LevelDraft {
  const action = level.action ?? null;
  return {
    id: uid(),
    from: level.from_case_count,
    sendDm: level.send_dm,
    action: action ? action.kind : "none",
    timeoutSeconds: action?.kind === "timeout" ? action.duration_seconds : 86400,
    deleteSeconds: action?.kind === "ban" ? action.delete_message_seconds : 0,
  };
}

/** fromDefinition loads a saved rule definition into the editor. */
export function fromDefinition(def: RuleDefinition): Draft {
  return {
    name: def.name,
    description: def.description,
    reason: def.reason,
    appealable: def.appealable,
    windowed: def.counting_window_days != null,
    windowDays: def.counting_window_days ?? 90,
    fields: def.fields.map((f) => ({
      id: uid(),
      label: f.label,
      key: f.key,
      keyEdited: true,
      kind: f.kind,
      required: f.required,
    })),
    levels: sortLevels(def.levels.map(levelDraft)),
  };
}

/** sortLevels orders levels by the case count they start at. */
export function sortLevels(levels: LevelDraft[]): LevelDraft[] {
  return [...levels].sort((a, b) => a.from - b.from);
}

function levelAction(level: LevelDraft): LevelAction | null {
  switch (level.action) {
    case "timeout":
      return { kind: "timeout", duration_seconds: level.timeoutSeconds };
    case "kick":
      return { kind: "kick" };
    case "ban":
      return { kind: "ban", delete_message_seconds: level.deleteSeconds };
    default:
      return null;
  }
}

/** toDefinition is the rule definition the API saves (design §3.2). */
export function toDefinition(draft: Draft): RuleDefinition {
  return {
    name: draft.name.trim(),
    description: draft.description.trim(),
    reason: draft.reason.trim(),
    appealable: draft.appealable,
    counting_window_days: draft.windowed ? draft.windowDays : null,
    fields: draft.fields.map((f) => ({
      key: f.key.trim(),
      label: f.label.trim(),
      kind: f.kind,
      required: f.required,
    })),
    levels: sortLevels(draft.levels).map((l) => ({
      from_case_count: l.from,
      action: levelAction(l),
      send_dm: l.sendDm,
    })),
  };
}

/**
 * fingerprint is a stable string for the parts of a draft that get saved,
 * so the editor can tell whether anything changed.
 */
export function fingerprint(draft: Draft): string {
  return JSON.stringify(toDefinition(draft));
}

/** Issues maps an editor field ("name", "levels.<id>.from") to its problem. */
export type Issues = Record<string, string>;

/** validate mirrors the API's rule checks (design §3.3) so problems show before saving. */
export function validate(draft: Draft): Issues {
  const issues: Issues = {};
  const name = draft.name.trim();
  if (!name) issues.name = "Give the rule a name.";
  else if (chars(name) > limits.name) issues.name = `Keep it to ${limits.name} characters.`;

  if (chars(draft.description.trim()) > limits.description) {
    issues.description = `Keep it to ${limits.description} characters.`;
  }

  const reason = draft.reason.trim();
  if (!reason) issues.reason = "Members need to know why.";
  else if (chars(reason) > limits.reason)
    issues.reason = `Keep it to ${limits.reason.toLocaleString()} characters.`;

  if (
    draft.windowed &&
    (!Number.isInteger(draft.windowDays) ||
      draft.windowDays < 1 ||
      draft.windowDays > limits.windowDays)
  ) {
    issues.window = `Pick between 1 and ${limits.windowDays.toLocaleString()} days.`;
  }

  if (draft.fields.length > limits.fields) {
    issues.fields = `A rule can ask for at most ${limits.fields} fields.`;
  }
  const seenKeys = new Set<string>();
  for (const f of draft.fields) {
    const label = f.label.trim();
    if (!label) issues[`fields.${f.id}.label`] = "Write the question moderators answer.";
    else if (chars(label) > limits.fieldLabel)
      issues[`fields.${f.id}.label`] = `Keep it to ${limits.fieldLabel} characters.`;
    const key = f.key.trim();
    if (!keyPattern.test(key)) {
      issues[`fields.${f.id}.key`] =
        key.length < 2 ? "Use at least 2 characters." : "Use a–z, 0–9, - or _.";
    } else if (seenKeys.has(key)) {
      issues[`fields.${f.id}.key`] = "Another field already uses this key.";
    }
    seenKeys.add(key);
  }

  if (draft.levels.length === 0) issues.levels = "A rule needs at least one level.";
  else if (draft.levels.length > limits.levels)
    issues.levels = `A rule can have at most ${limits.levels} levels.`;

  const sorted = sortLevels(draft.levels);
  const seen = new Set<number>();
  sorted.forEach((l, i) => {
    const at = (k: string) => `levels.${l.id}.${k}`;
    if (i === 0) {
      if (l.from !== 1) issues[at("from")] = "The first level starts at 1 case.";
    } else if (!Number.isInteger(l.from) || l.from < 2 || l.from > limits.caseCount) {
      issues[at("from")] = `Use a whole number from 2 to ${limits.caseCount.toLocaleString()}.`;
    } else if (seen.has(l.from)) {
      issues[at("from")] = "Another level already starts at this case count.";
    }
    seen.add(l.from);
    if (
      l.action === "timeout" &&
      (!Number.isInteger(l.timeoutSeconds) ||
        l.timeoutSeconds < 1 ||
        l.timeoutSeconds > limits.timeoutSeconds)
    ) {
      issues[at("timeout")] = "Discord allows timeouts of up to 28 days.";
    }
    if (l.action === "ban" && (l.deleteSeconds < 0 || l.deleteSeconds > limits.deleteSeconds)) {
      issues[at("delete")] = "Discord deletes at most the last 7 days of messages.";
    }
  });
  return issues;
}

const apiTopLevel: Record<string, string> = {
  name: "name",
  description: "description",
  reason: "reason",
  counting_window_days: "window",
  fields: "fields",
  levels: "levels",
};

const apiLevelField: Record<string, string> = {
  from_case_count: "from",
  "action.duration_seconds": "timeout",
  "action.delete_message_seconds": "delete",
};

/**
 * apiIssues maps the API's field errors ("levels[1].from_case_count",
 * "fields[0].key") onto the editor's fields. Levels are matched in the order
 * they were sent (by case count), fields in their list order. Errors that
 * match no field come back under "form".
 */
export function apiIssues(error: Pick<ApiError, "fields">, draft: Draft): Issues {
  const issues: Issues = {};
  const levels = sortLevels(draft.levels);
  for (const f of error.fields) {
    const indexed = /^(fields|levels)\[(\d+)\](?:\.(.+))?$/.exec(f.field);
    let key: string | undefined;
    if (indexed) {
      const [, list, index, rest = ""] = indexed;
      if (list === "fields") {
        const field = draft.fields[Number(index)];
        if (field) key = `fields.${field.id}.${rest === "label" ? "label" : "key"}`;
      } else {
        const level = levels[Number(index)];
        const part = apiLevelField[rest];
        if (level) key = part ? `levels.${level.id}.${part}` : "levels";
      }
    } else {
      key = apiTopLevel[f.field];
    }
    const at = key ?? "form";
    issues[at] = issues[at] ? `${issues[at]} ${f.message}` : f.message;
  }
  return issues;
}

/** Step is one level of a rule as it reads in the preview. */
export type Step = {
  id: string;
  /** The level's position, from 1. */
  index: number;
  /** The case count the level starts at. */
  from: number;
  /** The last case count it applies to, or undefined when it never ends. */
  to: number | undefined;
  /** Whether another level starts at the same case count, so this one never applies. */
  unreachable: boolean;
  sendDm: boolean;
  action: ActionChoice;
  timeoutSeconds: number;
  deleteSeconds: number;
};

/**
 * ladder works out which case counts each level covers, the way Quack picks
 * a level (design §3.4): the level with the highest starting case count at
 * or below the case count.
 */
export function ladder(levels: LevelDraft[]): Step[] {
  const ordered = sortLevels(levels.filter((l) => Number.isInteger(l.from) && l.from >= 1));
  return ordered.map((l, i) => {
    const next = ordered[i + 1];
    const to = next ? next.from - 1 : undefined;
    return {
      id: l.id,
      index: i + 1,
      from: l.from,
      to,
      unreachable: to !== undefined && to < l.from,
      sendDm: l.sendDm,
      action: l.action,
      timeoutSeconds: l.timeoutSeconds,
      deleteSeconds: l.deleteSeconds,
    };
  });
}

/** range says which case counts a step covers: "Case count 1–2", "Case count 5+". */
export function range(step: Pick<Step, "from" | "to" | "unreachable">, short = false): string {
  if (step.unreachable) return "Never reached";
  const { from, to } = step;
  const span = to === undefined ? `${from}+` : to === from ? `${from}` : `${from}–${to}`;
  return short ? span : `Case count ${span}`;
}

function stepAction(step: Pick<Step, "action" | "timeoutSeconds" | "deleteSeconds">) {
  return levelAction({ ...newLevel(1), ...step });
}

/** outcome says what a step does: "Warning", "Timeout for 1 day", "Ban". */
export function outcome(step: Pick<Step, "action" | "timeoutSeconds" | "deleteSeconds">): string {
  return capitalize(levelOutcome(stepAction(step)));
}

/** shortOutcome is outcome for tight spaces, without the message deletion. */
export function shortOutcome(step: Pick<Step, "action" | "timeoutSeconds">): string {
  return outcome({ ...step, deleteSeconds: 0 });
}

/**
 * summarize reads the levels as one line, for lists and screen readers:
 * "1–2 · Warning → 3–4 · Timeout for 1 day → 5+ · Ban".
 */
export function summarize(steps: Step[]): string {
  return steps
    .filter((s) => !s.unreachable)
    .map((s) => `${range(s, true)} · ${shortOutcome(s)}`)
    .join(" → ");
}

/** windowLabel says which cases count: "Counts the last 90 days" or "Counts every case". */
export function windowLabel(days: number | null | undefined): string {
  if (!days) return "Counts every case";
  return days === 1 ? "Counts the last day" : `Counts the last ${days.toLocaleString()} days`;
}

/**
 * parseRuleFile reads a rule file (design §3.6): {format: "quack-rule",
 * format_version: 1, rule}. Files exported by the earlier dashboard
 * (schema_version 1) are converted. It throws an Error with a readable
 * message when the text isn't a Quack rule file.
 */
export function parseRuleFile(text: string, now = new Date()): RuleFile {
  let raw: unknown;
  try {
    raw = JSON.parse(text);
  } catch {
    throw new Error("That isn't a rule file. Paste the whole file you exported from Quack.");
  }
  if (isRecord(raw) && raw.format === "quack-rule") {
    if (raw.format_version !== 1) {
      throw new Error("This rule file comes from a newer version of Quack. Export it again here.");
    }
    if (
      !isRecord(raw.rule) ||
      typeof raw.rule.name !== "string" ||
      !Array.isArray(raw.rule.levels)
    ) {
      throw new Error("This rule file is incomplete. Export it again and paste the whole file.");
    }
    return raw as unknown as RuleFile;
  }
  if (isRecord(raw) && isRecord(raw.policy)) raw = raw.policy;
  if (isRecord(raw) && raw.schema_version === 1 && typeof raw.name === "string") {
    return {
      format: "quack-rule",
      format_version: 1,
      exported_at: now.toISOString(),
      rule: legacyRule(raw),
    };
  }
  throw new Error("That isn't a rule file. Paste the whole file you exported from Quack.");
}

/** legacyRule converts a rule exported by the earlier dashboard. */
function legacyRule(raw: Record<string, unknown>): RuleDefinition {
  const legacyFields = Array.isArray(raw.context_fields) ? raw.context_fields.filter(isRecord) : [];
  const legacyLevels = Array.isArray(raw.levels) ? raw.levels.filter(isRecord) : [];
  const kinds: Record<string, FieldKind> = {
    short_text: "short_text",
    long_text: "long_text",
    boolean: "boolean",
    number: "number",
    discord_message_link: "message_link",
    message_link: "message_link",
  };
  const levels = legacyLevels
    .map((l) => {
      const first = Array.isArray(l.actions) ? l.actions.filter(isRecord)[0] : undefined;
      let action: LevelAction | null = null;
      if (first?.action_type === "timeout_user") {
        action = { kind: "timeout", duration_seconds: num(first.timeout_duration_seconds, 86400) };
      } else if (first?.action_type === "kick_user") {
        action = { kind: "kick" };
      } else if (first?.action_type === "ban_user") {
        action = { kind: "ban", delete_message_seconds: num(first.delete_message_seconds, 0) };
      }
      return {
        from_case_count: l.is_default === true ? 1 : Math.max(1, num(l.trigger_case_count, 1)),
        action,
        send_dm: l.notify_user !== false,
      };
    })
    .sort((a, b) => a.from_case_count - b.from_case_count);
  if (levels[0]) levels[0].from_case_count = 1;
  const days = num(raw.case_decay_days, 0);
  return {
    name: str(raw.name),
    description: str(raw.description),
    reason: str(raw.official_reason),
    appealable: raw.appealable === true,
    counting_window_days: days > 0 ? days : null,
    fields: [...legacyFields]
      .sort((a, b) => num(a.position, 0) - num(b.position, 0))
      .map((f) => ({
        key: str(f.key),
        label: str(f.label),
        kind: kinds[str(f.type)] ?? "short_text",
        required: f.required === true,
      })),
    levels,
  };
}

/** uniqueName returns name, or "name (2)", "name (3)"… when an active rule already uses it. */
export function uniqueName(name: string, taken: Iterable<string>): string {
  const used = new Set([...taken].map((n) => n.trim().toLowerCase()));
  if (!used.has(name.trim().toLowerCase())) return name;
  for (let n = 2; ; n++) {
    const suffix = ` (${n})`;
    const candidate = `${Array.from(name)
      .slice(0, limits.name - suffix.length)
      .join("")}${suffix}`;
    if (!used.has(candidate.toLowerCase())) return candidate;
  }
}

/** chars counts Unicode scalar values, the way the API measures length. */
function chars(s: string): number {
  return Array.from(s).length;
}

function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

function str(v: unknown): string {
  return typeof v === "string" ? v : "";
}

function num(v: unknown, fallback: number): number {
  return typeof v === "number" && Number.isFinite(v) ? v : fallback;
}
