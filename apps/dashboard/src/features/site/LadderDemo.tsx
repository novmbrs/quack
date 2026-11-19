import { useState } from "react";

import { selectLevel } from "~/lib/escalation";
import { QuackIcon, type QuackIconName } from "~/ui/QuackIcon";

import { starterSteps, stepLabel } from "./ladder";
import s from "./LadderDemo.module.css";

const icons: QuackIconName[] = ["warn", "timeout", "ban"];
const counts = [1, 2, 3, 4, 5, 6];

/**
 * LadderDemo lets a visitor step through a member's case count under the
 * starter rule and watch Quack pick the level, which explains escalation
 * faster than a paragraph does.
 */
export function LadderDemo() {
  const [count, setCount] = useState(3);
  const active = selectLevel(starterSteps, count);

  return (
    <div className={s.demo}>
      <p className={s.rule}>General rule violation</p>

      <div className={s.picker} role="radiogroup" aria-label="Case count">
        {counts.map((n) => (
          <button
            key={n}
            type="button"
            role="radio"
            aria-checked={n === count}
            className={s.count}
            onClick={() => setCount(n)}
          >
            {n === counts.at(-1) ? `${n}+` : n}
          </button>
        ))}
      </div>

      <ol className={s.steps}>
        {starterSteps.map((step, i) => (
          <li
            key={step.from_case_count}
            className={s.step}
            data-active={step === active || undefined}
          >
            <QuackIcon name={icons[i]!} size={26} />
            <div className={s.stepText}>
              <p className={s.stepLabel}>{stepLabel(step)}</p>
              <p className={s.stepDetail}>{step.detail}</p>
            </div>
            <span className={s.when}>{step.when}</span>
          </li>
        ))}
      </ol>

      <p className={s.result} aria-live="polite">
        With a case count of <b>{count}</b>, Quack uses level{" "}
        {active ? starterSteps.indexOf(active) + 1 : 1}:{" "}
        <b>{active ? stepLabel(active).toLowerCase() : "warning"}</b>.
      </p>
    </div>
  );
}
