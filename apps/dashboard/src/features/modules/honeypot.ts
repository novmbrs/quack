import type { LevelAction, RuleDefinition } from "~/api/types";
import { duration } from "~/lib/format";

/** outcome is what a level can do to the poster, as the warning words it (design §12.13). */
function outcome(action: LevelAction | null | undefined): string {
  if (!action) return "give you a warning";
  switch (action.kind) {
    case "timeout":
      return `time you out for ${duration(action.duration_seconds)}`;
    case "kick":
      return "remove you from the server";
    case "ban":
      return "ban you";
  }
}

/** orList joins phrases as "a", "a or b", or "a, b, or c". */
function orList(items: string[]): string {
  if (items.length <= 2) return items.join(" or ");
  return `${items.slice(0, -1).join(", ")}, or ${items.at(-1)}`;
}

/**
 * honeypotWarning is the warning Quack posts in the honeypot channel
 * (honeypot.warning.*). A custom text replaces the heading and sentences;
 * otherwise Quack words one from the rule's levels, each outcome once in
 * level order. It is null when there's no custom text and no rule to word
 * it from.
 */
export function honeypotWarning(
  text: string,
  rule: Pick<RuleDefinition, "name" | "levels"> | undefined,
): string | null {
  if (text.trim()) return text;
  if (!rule) return null;
  const said: string[] = [];
  const levels = [...rule.levels].sort((a, b) => a.from_case_count - b.from_case_count);
  for (const level of levels) {
    const o = outcome(level.action);
    if (!said.includes(o)) said.push(o);
  }
  const lead = `If you post here, Quack can add a case under ${rule.name}.`;
  const can =
    said.length > 1
      ? `This rule can ${orList(said)}. What happens depends on how many of your cases count under it.`
      : `This rule can ${said[0] ?? "give you a warning"}.`;
  return `### Don't post here\n${lead} ${can}\nThis channel catches spam accounts. Quack ignores posts from staff and bots.`;
}

/** honeypotWarningOff is what the warning post says while the honeypot is off. */
export const honeypotWarningOff = "### The honeypot is off\nPosts here don't add cases right now.";

/** honeypotCount is the count line under the warning post. */
export function honeypotCount(count: number): string {
  if (count === 0) return "No honeypot cases yet.";
  if (count === 1) return "1 honeypot case so far.";
  return `${count.toLocaleString("en-US")} honeypot cases so far.`;
}
