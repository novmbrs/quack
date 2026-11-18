import { Plus, Trash2 } from "lucide-react";
import { useState } from "react";

import { last } from "~/lib/format";
import { Button } from "~/ui/Button";
import { Field, Select, Switch } from "~/ui/Field";
import { QuackIcon } from "~/ui/QuackIcon";
import { Segmented } from "~/ui/Segmented";
import { InlineError } from "~/ui/States";

import { DurationPicker, IconButton, NumberInput } from "./controls";
import {
  type Enforcement,
  type Issues,
  type LevelDraft,
  limits,
  newLevel,
  sortLevels,
} from "./draft";
import { enforcementIcon } from "./Ladder";
import s from "./LevelsEditor.module.css";

const actions: { value: Enforcement; label: string }[] = [
  { value: "none", label: "Warning" },
  { value: "timeout", label: "Timeout" },
  { value: "kick", label: "Kick" },
  { value: "ban", label: "Ban" },
];

const deleteChoices = [
  { seconds: 0, label: "Don't delete any" },
  { seconds: 3600, label: "The last hour" },
  { seconds: 86400, label: "The last day" },
  { seconds: 7 * 86400, label: "The last 7 days" },
];

/**
 * LevelsEditor edits a rule's levels: the first starts at a case count of 1
 * and each later one takes over at a higher case count. Levels re-sort by
 * their case count once the rules manager leaves that box, so the list
 * always reads in order.
 */
export function LevelsEditor({
  levels,
  onChange,
  issues,
  readOnly,
}: {
  levels: LevelDraft[];
  onChange: (levels: LevelDraft[]) => void;
  issues: Issues;
  readOnly?: boolean;
}) {
  const [added, setAdded] = useState<string | null>(null);
  const patch = (id: string, change: Partial<LevelDraft>) =>
    onChange(levels.map((l) => (l.id === id ? { ...l, ...change } : l)));

  const add = () => {
    const highest = Math.max(
      1,
      ...levels.filter((l) => Number.isFinite(l.from)).map((l) => l.from),
    );
    const level = newLevel(highest + 2);
    setAdded(level.id);
    onChange(sortLevels([...levels, level]));
  };

  return (
    <div className={s.list}>
      {levels.map((level, i) => (
        <LevelCard
          key={level.id}
          index={i + 1}
          first={i === 0}
          level={level}
          issues={issues}
          readOnly={readOnly}
          autoFocus={level.id === added}
          onChange={(change) => patch(level.id, change)}
          onSettle={() => onChange(sortLevels(levels))}
          onRemove={() => onChange(levels.filter((l) => l.id !== level.id))}
        />
      ))}
      {issues.levels ? <InlineError>{issues.levels}</InlineError> : null}
      {readOnly || levels.length >= limits.levels ? null : (
        <div>
          <Button variant="secondary" size="sm" icon={<Plus size={16} />} onClick={add}>
            Add level
          </Button>
        </div>
      )}
    </div>
  );
}

function LevelCard({
  index,
  first,
  level,
  issues,
  readOnly,
  autoFocus,
  onChange,
  onSettle,
  onRemove,
}: {
  index: number;
  /** The first level always starts at a case count of 1. */
  first: boolean;
  level: LevelDraft;
  issues: Issues;
  readOnly?: boolean;
  autoFocus: boolean;
  onChange: (change: Partial<LevelDraft>) => void;
  onSettle: () => void;
  onRemove: () => void;
}) {
  const issue = (k: string) => issues[`levels.${level.id}.${k}`];
  const deleteOptions = deleteChoices.some((c) => c.seconds === level.deleteSeconds)
    ? deleteChoices
    : [
        ...deleteChoices,
        { seconds: level.deleteSeconds, label: `The last ${last(level.deleteSeconds)}` },
      ];

  return (
    <div className={s.card} data-default={first || undefined} data-action={level.action}>
      <div className={s.head}>
        <span className={s.badge}>
          <QuackIcon name={enforcementIcon[level.action]} size={20} />
        </span>
        <div className={s.kicker}>Level {index}</div>
        {readOnly || first ? null : (
          <IconButton label={`Remove level ${index}`} tone="danger" onClick={onRemove}>
            <Trash2 size={16} />
          </IconButton>
        )}
      </div>

      {first && level.from === 1 ? (
        <p className={s.hint}>
          Starts at a case count of 1: a member's first counted case under this rule.
        </p>
      ) : (
        <div className={s.topRow}>
          <Field
            label="Starts at case count"
            error={issue("from")}
            hint="The number of counted cases, including the new one."
          >
            {(id) => (
              <NumberInput
                id={id}
                min={first ? 1 : 2}
                max={limits.caseCount}
                value={level.from}
                autoFocus={autoFocus}
                invalid={Boolean(issue("from"))}
                onChange={(from) => onChange({ from })}
                onBlur={onSettle}
              />
            )}
          </Field>
        </div>
      )}

      <div className={s.switchRow}>
        <div>
          <p className={s.switchTitle}>DM the member</p>
          <p className={s.hint}>
            One DM with the case number, the reason, the action, and how to appeal.
          </p>
        </div>
        <Switch
          label="DM the member"
          checked={level.sendDm}
          disabled={readOnly}
          onChange={(sendDm) => onChange({ sendDm })}
        />
      </div>

      <div className={s.block}>
        <p className={s.label}>Action</p>
        <div>
          <Segmented
            label="Action"
            value={level.action}
            options={actions}
            onChange={(action) => onChange({ action })}
          />
        </div>
        {level.action === "none" ? (
          <p className={s.hint}>
            The case itself is the warning. It still counts toward the case count.
          </p>
        ) : null}
      </div>

      {level.action === "timeout" ? (
        <Field
          label="Timeout length"
          error={issue("timeout")}
          hint="Discord allows timeouts of up to 28 days."
          className={s.appear}
        >
          {(id) => (
            <DurationPicker
              id={id}
              value={level.timeoutSeconds}
              invalid={Boolean(issue("timeout"))}
              onChange={(timeoutSeconds) => onChange({ timeoutSeconds })}
            />
          )}
        </Field>
      ) : null}

      {level.action === "ban" ? (
        <Field label="Delete message history" error={issue("delete")} className={s.appear}>
          {(id) => (
            <Select
              id={id}
              value={level.deleteSeconds}
              className={s.select}
              onChange={(e) => onChange({ deleteSeconds: Number(e.currentTarget.value) })}
            >
              {deleteOptions.map((c) => (
                <option key={c.seconds} value={c.seconds}>
                  {c.label}
                </option>
              ))}
            </Select>
          )}
        </Field>
      ) : null}
    </div>
  );
}
