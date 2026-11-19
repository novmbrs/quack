import type { Action, ActionKind } from "~/api/types";

// Words for the staff action controls (design §12.2 and §12.6), shared by
// the case page and the failed actions page.

const noun: Record<ActionKind, string> = {
  timeout: "timeout",
  kick: "kick",
  ban: "ban",
  remove_timeout: "remove timeout",
  unban: "unban",
};

/** retryLabel is the Retry button: "Retry ban", "Retry removing timeout". */
export function retryLabel(kind: ActionKind): string {
  return kind === "remove_timeout" ? "Retry removing timeout" : `Retry ${noun[kind]}`;
}

/** dismissLabel is the Dismiss button: "Dismiss failed ban", "Dismiss failed timeout removal". */
export function dismissLabel(kind: ActionKind): string {
  return kind === "remove_timeout"
    ? "Dismiss failed timeout removal"
    : `Dismiss failed ${noun[kind]}`;
}

/** undoKind is the undo an original action offers: remove timeout for a timeout, unban for a ban. */
export function undoKind(a: Pick<Action, "kind">): "remove_timeout" | "unban" | null {
  return a.kind === "timeout" ? "remove_timeout" : a.kind === "ban" ? "unban" : null;
}

/** undoLabel is the undo button: "Remove timeout" or "Unban". */
export function undoLabel(kind: "remove_timeout" | "unban"): string {
  return kind === "remove_timeout" ? "Remove timeout" : "Unban";
}

const does: Record<ActionKind, string> = {
  timeout: "time out the member",
  kick: "remove the member from the server",
  ban: "ban the member",
  remove_timeout: "remove the timeout",
  unban: "unban the member",
};

/** retryDone is action.retry.done, without the pointer to the case when already on it. */
export function retryDone(kind: ActionKind, caseNumber: number, onCase = false): string {
  const lead = `Quack will try again to ${does[kind]} for case #${caseNumber}.`;
  return onCase ? lead : `${lead} Open the case to see the outcome.`;
}

/** dismissDone is action.dismiss.done. */
export function dismissDone(kind: ActionKind, caseNumber: number): string {
  const lead =
    kind === "remove_timeout"
      ? `The failed attempt to remove the timeout for case #${caseNumber} was dismissed.`
      : kind === "unban"
        ? `The failed attempt to unban the member for case #${caseNumber} was dismissed.`
        : `The ${noun[kind]} failure for case #${caseNumber} was dismissed.`;
  return `${lead} The case keeps its history.`;
}

/** undoConfirm is action.undo.confirm.*. */
export function undoConfirm(kind: "remove_timeout" | "unban", member: string, caseNumber: number) {
  return kind === "remove_timeout"
    ? `Remove ${member}'s timeout for case #${caseNumber}?`
    : `Unban ${member} for case #${caseNumber}?`;
}

/** undoQueued is action.undo.queued.*. */
export function undoQueued(kind: "remove_timeout" | "unban", member: string, caseNumber: number) {
  const lead =
    kind === "remove_timeout"
      ? `Quack will check whether it can remove ${member}'s timeout.`
      : `Quack will check whether it can unban ${member}.`;
  return `${lead} Open case #${caseNumber} to see the outcome.`;
}

/**
 * mayHaveApplied reports whether a failed action's outcome is unknown, so a
 * retry must be confirmed: Discord may already have applied it.
 */
export function mayHaveApplied(a: Pick<Action, "state" | "failure_reason">): boolean {
  return a.state === "failed" && a.failure_reason === "unconfirmed";
}
