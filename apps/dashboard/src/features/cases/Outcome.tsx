import type { ActionKind, CaseSummary } from "~/api/types";
import { actionMeta, type Outcome as OutcomeWord } from "~/lib/format";
import { Badge } from "~/ui/Badge";
import { QuackIcon } from "~/ui/QuackIcon";
import { Spinner } from "~/ui/Spinner";

import s from "./Outcome.module.css";

/** ActionBrief is a case's action as lists carry it: kind, state, and short outcome word. */
type ActionBrief = NonNullable<CaseSummary["action"]>;

/**
 * Outcome summarizes what a case did: a warning, or its action and the
 * outcome word the server gives ("Done", "Failed", "Couldn't confirm"…).
 * Compact rows leave out the word when the action is done.
 */
export function Outcome({
  action,
  compact,
}: {
  action: ActionBrief | null | undefined;
  compact?: boolean;
}) {
  if (!action) {
    return (
      <span className={s.row}>
        <QuackIcon name="warn" size={18} />
        <span className={s.label}>Warning</span>
      </span>
    );
  }
  const meta = actionMeta[action.kind];
  return (
    <span className={s.row}>
      <QuackIcon name={meta.icon} size={18} />
      <span className={s.label}>{meta.label}</span>
      {!compact || action.state !== "done" ? (
        <OutcomeBadge
          outcome={{ label: action.outcome_text, tone: briefTone(action) }}
          moving={action.state === "pending" || action.state === "running"}
        />
      ) : null}
    </span>
  );
}

function briefTone(a: ActionBrief): OutcomeWord["tone"] {
  switch (a.state) {
    case "done":
      return "success";
    case "failed":
      return "danger";
    case "pending":
    case "running":
      return "info";
    default:
      return "neutral";
  }
}

/** OutcomeBadge shows an outcome word, with a spinner while it's still in progress. */
export function OutcomeBadge({ outcome, moving }: { outcome: OutcomeWord; moving?: boolean }) {
  return (
    <Badge tone={outcome.tone} dot={!moving} icon={moving ? <Spinner size={10} /> : undefined}>
      {outcome.label}
    </Badge>
  );
}

/** actionLabel is an action's name in a sentence: "timeout", "remove timeout". */
export function actionLabel(kind: ActionKind): string {
  return actionMeta[kind].label.toLowerCase();
}
