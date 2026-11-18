import type { AppealState, Decision } from "~/api/types";

export type { Decision };

/**
 * decisionsFrom mirrors the backend's appeal transitions (design §8.2):
 * what staff can do from each state. Decided appeals can only be reopened,
 * which asks the member for more information.
 */
export const decisionsFrom: Record<AppealState, Decision[]> = {
  pending: ["accept", "reject", "request_information", "close"],
  needs_information: ["accept", "reject", "close"],
  accepted: ["reopen"],
  rejected: ["reopen"],
  closed: ["reopen"],
};

/**
 * The reasons Discord's Accept and Reject buttons send when the moderator
 * leaves the box as it is (design §12.9 appeal.reason.default.*), so the
 * member reads the same thing whichever surface staff used. Every other
 * decision is a message only staff can write.
 */
const defaultReasons: Partial<Record<Decision, string>> = {
  accept: "We reviewed your appeal. The case no longer counts.",
  reject: "We reviewed your appeal. The case still counts.",
};

/** defaultReason is the reason sent when staff leave the box empty, if the decision has one. */
export function defaultReason(decision: Decision): string | undefined {
  return defaultReasons[decision];
}

/** reasonRequired says whether staff must write the reason themselves. */
export function reasonRequired(decision: Decision): boolean {
  return !(decision in defaultReasons);
}

/** decisionReason is what to send: the staff text, or the standard reason. */
export function decisionReason(decision: Decision, text: string): string {
  return text.trim() || defaultReasons[decision] || "";
}

/** The state filter in the appeal queue's URL. "all" lists every state. */
export type StateFilter = AppealState | "all";

/** FilterTab is a top-level tab; decided appeals share one, split below it. */
export type FilterTab = "pending" | "needs_information" | "decided" | "all";

const decided: AppealState[] = ["accepted", "rejected", "closed"];

/** tabOf finds the tab a state filter belongs to. */
export function tabOf(filter: StateFilter): FilterTab {
  return decided.includes(filter as AppealState) ? "decided" : (filter as FilterTab);
}

/** filterOf picks the state filter a tab opens with. */
export function filterOf(tab: FilterTab): StateFilter {
  return tab === "decided" ? "accepted" : tab;
}

/** parseStateFilter reads the filter from search params, defaulting to pending. */
export function parseStateFilter(value: unknown): StateFilter {
  const all: StateFilter[] = ["pending", "needs_information", ...decided, "all"];
  return all.includes(value as StateFilter) ? (value as StateFilter) : "pending";
}
