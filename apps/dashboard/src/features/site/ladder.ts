import type { LevelAction } from "~/api/types";
import { capitalize, levelOutcome } from "~/lib/format";

/** Step is one level of the homepage's example rule. */
export type Step = {
  from_case_count: number;
  action: LevelAction | null;
  detail: string;
  /** when names the case counts the level covers, in words. */
  when: string;
};

/**
 * starterSteps mirrors the General rule violation rule Quack creates when it
 * joins a server (design §3.7), so the homepage demo shows what a new
 * server really gets.
 */
export const starterSteps: Step[] = [
  {
    from_case_count: 1,
    action: null,
    detail: "Quack adds the case and sends the member a DM.",
    when: "Cases 1–2",
  },
  {
    from_case_count: 3,
    action: { kind: "timeout", duration_seconds: 86400 },
    detail: "The member can't chat for a day.",
    when: "Cases 3–4",
  },
  {
    from_case_count: 5,
    action: { kind: "ban", delete_message_seconds: 86400 },
    detail: "Quack bans the member and deletes their messages from the last day.",
    when: "Case 5 and on",
  },
];

/** stepLabel is what a level does, as a label: "Warning", "Timeout for 1 day". */
export function stepLabel(step: Pick<Step, "action">): string {
  return capitalize(levelOutcome(step.action));
}

/** ordinal writes 1 as "1st", 2 as "2nd", and so on. */
export function ordinal(n: number): string {
  const tens = n % 100;
  if (tens >= 11 && tens <= 13) return `${n}th`;
  return `${n}${["th", "st", "nd", "rd"][n % 10] ?? "th"}`;
}
