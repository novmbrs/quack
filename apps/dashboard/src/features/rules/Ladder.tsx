import { ChevronRight } from "lucide-react";
import { Fragment } from "react";

import { QuackIcon, type QuackIconName } from "~/ui/QuackIcon";

import { type Enforcement, outcome, range, shortOutcome, type Step, summarize } from "./draft";
import s from "./Ladder.module.css";

/** enforcementIcon is the Signals icon for a level's action. */
export const enforcementIcon: Record<Enforcement, QuackIconName> = {
  none: "warn",
  timeout: "timeout",
  kick: "kick",
  ban: "ban",
};

/**
 * Ladder shows a rule's levels top to bottom, the way a rules manager reasons
 * about them: "Case count 3–4: Timeout for 1 day. Sends a DM".
 */
export function Ladder({ steps }: { steps: Step[] }) {
  return (
    <ol className={s.list} aria-label="Levels">
      {steps.map((step) => (
        <li
          key={step.id}
          className={s.step}
          data-action={step.action}
          data-unreachable={step.unreachable || undefined}
        >
          <div className={s.rail}>
            <span className={s.node}>
              <QuackIcon name={enforcementIcon[step.action]} size={18} />
            </span>
            <span className={s.line} />
          </div>
          <div className={s.body}>
            <p className={s.range}>{range(step)}</p>
            <p className={s.outcome}>{outcome(step)}</p>
            <p className={s.meta}>
              Level {step.index}
              {" · "}
              {step.sendDm ? "Sends a DM" : "No DM"}
            </p>
          </div>
        </li>
      ))}
    </ol>
  );
}

/**
 * LadderInline is the one-line version for lists:
 * 1–2 Warning → 3–4 Timeout for 1 day → 5+ Ban.
 */
export function LadderInline({ steps }: { steps: Step[] }) {
  const shown = steps.filter((step) => !step.unreachable);
  return (
    <p className={s.inline} aria-label={summarize(steps)}>
      {shown.map((step, i) => (
        <Fragment key={step.id}>
          {i > 0 ? <ChevronRight size={14} aria-hidden className={s.arrow} /> : null}
          <span className={s.chip} data-action={step.action} aria-hidden>
            <QuackIcon name={enforcementIcon[step.action]} size={16} />
            <span className={s.chipRange}>{range(step, true)}</span>
            <span>{shortOutcome(step)}</span>
          </span>
        </Fragment>
      ))}
    </p>
  );
}
