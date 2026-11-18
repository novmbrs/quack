import type {
  Action,
  ActionKind,
  AppealState,
  CaseSource,
  DmState,
  Level,
  LevelAction,
  ModLogResult,
} from "~/api/types";
import type { QuackIconName } from "~/ui/QuackIcon";
import type { Tone } from "~/ui/Badge";

const rtf = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });
const dateFmt = new Intl.DateTimeFormat(undefined, { dateStyle: "medium" });
const dateTimeFmt = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" });
const timeFmt = new Intl.DateTimeFormat(undefined, { timeStyle: "short" });

/** ago says how long ago a timestamp was, the way Discord does: "5 minutes ago". */
export function ago(iso: string | null | undefined, now = Date.now()): string {
  if (!iso) return "";
  const seconds = Math.round((new Date(iso).getTime() - now) / 1000);
  const abs = Math.abs(seconds);
  if (abs < 45) return "just now";
  if (abs < 45 * 60) return rtf.format(Math.round(seconds / 60), "minute");
  if (abs < 22 * 3600) return rtf.format(Math.round(seconds / 3600), "hour");
  if (abs < 6 * 86400) return rtf.format(Math.round(seconds / 86400), "day");
  return dateFmt.format(new Date(iso));
}

/** stamp is Discord's message timestamp: "Today at 4:20 PM" or a full date. */
export function stamp(iso: string | null | undefined, now = new Date()): string {
  if (!iso) return "";
  const d = new Date(iso);
  const day = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const diff = (day(now) - day(d)) / 86400000;
  if (diff === 0) return `Today at ${timeFmt.format(d)}`;
  if (diff === 1) return `Yesterday at ${timeFmt.format(d)}`;
  return dateTimeFmt.format(d);
}

export function fullDate(iso: string | null | undefined): string {
  return iso ? dateTimeFmt.format(new Date(iso)) : "";
}

const units: [number, string][] = [
  [86400, "day"],
  [3600, "hour"],
  [60, "minute"],
  [1, "second"],
];

const unit = (n: number, name: string) => `${n.toLocaleString()} ${name}${n === 1 ? "" : "s"}`;

/**
 * duration spells out seconds the way Quack's messages do (design §12.0):
 * the largest whole unit and, when not zero, the next smaller one: "45
 * seconds", "1 hour 30 minutes", "7 days".
 */
export function duration(seconds: number | null | undefined): string {
  const total = Math.max(0, Math.floor(seconds ?? 0));
  if (total === 0) return "0 seconds";
  const i = units.findIndex(([size]) => total >= size);
  const [size, name] = units[i]!;
  const whole = Math.floor(total / size);
  const next = units[i + 1];
  const rest = next ? Math.floor((total % size) / next[0]) : 0;
  return rest && next ? `${unit(whole, name)} ${unit(rest, next[1])}` : unit(whole, name);
}

/** last is a duration after "the last": "the last day", "the last 7 days". */
export function last(seconds: number): string {
  const d = duration(seconds);
  return d.startsWith("1 ") && !d.includes(" ", 2) ? d.slice(2) : d;
}

export function plural(n: number, one: string, many = `${one}s`): string {
  return `${n.toLocaleString()} ${n === 1 ? one : many}`;
}

export function ordinal(n: number): string {
  const rules = new Intl.PluralRules("en", { type: "ordinal" });
  const suffix = { one: "st", two: "nd", few: "rd", other: "th", zero: "th", many: "th" }[
    rules.select(n)
  ];
  return `${n}${suffix}`;
}

type ActionMeta = { label: string; icon: QuackIconName; tone: Tone };

/** actionMeta names each action kind (design §12.2) with its icon. */
export const actionMeta: Record<ActionKind, ActionMeta> = {
  timeout: { label: "Timeout", icon: "timeout", tone: "purple" },
  kick: { label: "Kick", icon: "kick", tone: "orange" },
  ban: { label: "Ban", icon: "ban", tone: "danger" },
  remove_timeout: { label: "Remove timeout", icon: "untimeout", tone: "success" },
  unban: { label: "Unban", icon: "unban", tone: "success" },
};

/** Outcome is what actually happened to an action or DM (glossary). */
export type Outcome = { label: string; tone: Tone };

/**
 * actionOutcome is the short outcome word for an action (design §12.2): in
 * progress, done, failed, couldn't confirm, cancelled, or left in place.
 */
export function actionOutcome(
  a: Pick<Action, "state"> & Partial<Pick<Action, "failure_reason">>,
): Outcome {
  switch (a.state) {
    case "pending":
    case "running":
      return { label: "In progress", tone: "info" };
    case "done":
      return { label: "Done", tone: "success" };
    case "failed":
      return a.failure_reason === "unconfirmed"
        ? { label: "Couldn't confirm", tone: "warning" }
        : { label: "Failed", tone: "danger" };
    case "cancelled":
      return { label: "Cancelled", tone: "neutral" };
    case "skipped":
      return { label: "Left in place", tone: "neutral" };
  }
}

/** dmOutcome is the short outcome word for a case DM: sent, couldn't send, couldn't confirm, not needed. */
export const dmOutcome: Record<DmState, Outcome> = {
  pending: { label: "In progress", tone: "info" },
  sending: { label: "In progress", tone: "info" },
  sent: { label: "Sent", tone: "success" },
  failed: { label: "Couldn't send", tone: "danger" },
  unknown: { label: "Couldn't confirm", tone: "warning" },
  not_needed: { label: "Not needed", tone: "neutral" },
};

export const appealMeta: Record<AppealState, { label: string; tone: Tone; icon: QuackIconName }> = {
  pending: { label: "Waiting for a decision", tone: "warning", icon: "pending" },
  needs_information: { label: "Waiting for the member", tone: "info", icon: "reply" },
  accepted: { label: "Accepted", tone: "success", icon: "accept" },
  rejected: { label: "Rejected", tone: "danger", icon: "decline" },
  closed: { label: "Closed", tone: "neutral", icon: "lock" },
};

export const sourceLabel: Record<CaseSource, string> = {
  dashboard: "Dashboard",
  discord: "Discord",
  honeypot: "Honeypot",
  imported: "Imported",
};

export const modLogResultTone: Record<ModLogResult, Tone> = {
  ok: "success",
  failed: "danger",
  refused: "warning",
};

/**
 * levelName names a level by its position and where it starts, since levels
 * have no names: "Level 2 (starts at 3 cases)". index is 1-based.
 */
export function levelName(index: number, fromCaseCount: number): string {
  return `Level ${index} (starts at ${plural(fromCaseCount, "case")})`;
}

/**
 * levelOutcome is what a level does (design §12.2 level.outcome.*):
 * "warning", "timeout for 1 hour", "kick", "ban", or "ban and delete the
 * last day of messages".
 */
export function levelOutcome(action: LevelAction | null | undefined): string {
  if (!action) return "warning";
  switch (action.kind) {
    case "timeout":
      return `timeout for ${duration(action.duration_seconds)}`;
    case "kick":
      return "kick";
    case "ban":
      return action.delete_message_seconds > 0
        ? `ban and delete the last ${last(action.delete_message_seconds)} of messages`
        : "ban";
  }
}

/** levelSummary is one level in a sentence: "at 3 cases: timeout for 1 hour". */
export function levelSummary(level: Pick<Level, "from_case_count" | "action">): string {
  return `at ${plural(level.from_case_count, "case")}: ${levelOutcome(level.action)}`;
}

/** capitalize upper-cases the first letter, for a word at the start of a label. */
export function capitalize(text: string): string {
  return text.charAt(0).toUpperCase() + text.slice(1);
}

/** humanize turns "message_delete" into "Message delete". */
export function humanize(key: string): string {
  const words = key.replace(/[._-]+/g, " ").trim();
  return words.charAt(0).toUpperCase() + words.slice(1);
}

export function bytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

/** isSnowflake reports whether s looks like a Discord ID. */
export function isSnowflake(s: string): boolean {
  return /^\d{15,21}$/.test(s.trim());
}
