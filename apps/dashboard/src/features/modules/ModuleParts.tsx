import { useMutation, useQueryClient } from "@tanstack/react-query";
import { type ReactNode, useState } from "react";

import { api, write } from "~/api/client";
import { healthQuery, keys } from "~/api/queries";
import { type ModuleName, type ModuleState, moduleSegment } from "~/api/types";
import { Badge } from "~/ui/Badge";
import { Button } from "~/ui/Button";
import { QuackIcon, type QuackIconName } from "~/ui/QuackIcon";
import { toast } from "~/ui/Toast";

import { disabledReasonText, type EnableProblem, enableProblem, moduleWords } from "./enable";
import s from "./ModuleParts.module.css";

/**
 * useModuleSwitch turns a module on or off with its own enable and disable
 * routes (design §13.1.7). Turning on re-checks the module's channels and
 * Quack's permissions; a refusal is kept as problem to show under the
 * header, one line per thing to fix.
 */
export function useModuleSwitch(guildId: string, module: ModuleName) {
  const queryClient = useQueryClient();
  const [problem, setProblem] = useState<EnableProblem | null>(null);
  const words = moduleWords[module];
  const mutation = useMutation({
    mutationFn: (on: boolean) =>
      write((init) =>
        api.POST(
          on
            ? "/api/guilds/{guild_id}/modules/{module}/enable"
            : "/api/guilds/{guild_id}/modules/{module}/disable",
          { params: { path: { guild_id: guildId, module: moduleSegment[module] } }, ...init },
        ),
      ),
    onMutate: () => setProblem(null),
    onSuccess: (state: ModuleState, on) => {
      const verb = module === "tickets" ? "are" : "is";
      toast.success(
        on && state.enabled ? `${words.Noun} ${verb} on.` : `${words.Noun} ${verb} off.`,
      );
      void queryClient.invalidateQueries({ queryKey: keys.modules(guildId) });
      void queryClient.invalidateQueries({ queryKey: keys.settings(guildId) });
      void queryClient.invalidateQueries({ queryKey: healthQuery(guildId).queryKey });
    },
    onError: (e) => setProblem(enableProblem(e)),
  });
  return { toggle: mutation.mutate, pending: mutation.isPending, problem };
}

/**
 * ModuleHeader introduces a module: what it does in a sentence or two,
 * whether it's on, and the button that turns it on or off. Below it are why
 * Quack turned it off by itself, what it currently can't do, and what
 * stopped the last switch.
 */
export function ModuleHeader({
  module,
  icon,
  children,
  state,
  onToggle,
  pending,
  canManage,
  locked,
  problem,
}: {
  module: ModuleName;
  icon: QuackIconName;
  /** One or two plain sentences on what the module does. */
  children: ReactNode;
  /** The module's state, undefined while it loads. */
  state: Pick<ModuleState, "enabled" | "disabled_reason" | "problems"> | undefined;
  onToggle: (on: boolean) => void;
  pending?: boolean;
  canManage: boolean;
  /** Why the button is unavailable right now, such as unsaved changes. */
  locked?: string | null;
  problem?: EnableProblem | null;
}) {
  const words = moduleWords[module];
  const enabled = state?.enabled;
  const reason = state ? disabledReasonText(module, state) : null;
  return (
    <div className={s.header}>
      <div className={s.top}>
        <span className={s.iconTile} data-on={enabled || undefined}>
          <QuackIcon name={icon} size={32} />
        </span>
        <div className={s.text}>
          <div className={s.titleRow}>
            <h2 className={s.title}>{words.title}</h2>
            {enabled === undefined ? null : enabled ? (
              <Badge tone="success" dot>
                On
              </Badge>
            ) : (
              <Badge tone="neutral" dot>
                Off
              </Badge>
            )}
          </div>
          <p className={s.description}>{children}</p>
          {locked ? <p className={s.locked}>{locked}</p> : null}
        </div>
        {canManage && enabled !== undefined ? (
          <Button
            size="sm"
            variant={enabled ? "secondary" : "primary"}
            pending={pending}
            disabled={Boolean(locked)}
            onClick={() => onToggle(!enabled)}
          >
            {enabled ? "Turn off" : "Turn on"}
          </Button>
        ) : null}
      </div>
      {reason ? (
        <Callout tone="danger" icon="error" title={`Quack turned ${words.noun} off`}>
          {reason}
        </Callout>
      ) : null}
      {state?.problems.length ? (
        <Callout
          tone="warning"
          icon="warn"
          title={enabled ? "Fix these to get everything working" : "Fix these before turning it on"}
        >
          <ProblemList items={state.problems.map((p) => p.message)} />
        </Callout>
      ) : null}
      {problem ? (
        <Callout tone="danger" icon="error" title={problem.message}>
          {problem.problems.length ? <ProblemList items={problem.problems} /> : null}
          {problem.problems.length ? (
            <>
              {" "}
              Fix these here, or open <Command>/setup</Command> in Discord and choose{" "}
              {words.setupArea} to let Quack set it up.
            </>
          ) : null}
        </Callout>
      ) : null}
    </div>
  );
}

/** ProblemList shows one sentence per problem, inside a callout's text. */
function ProblemList({ items }: { items: string[] }) {
  return (
    <>
      {items.map((item) => (
        <span key={item} className={s.problem}>
          {item}
        </span>
      ))}
    </>
  );
}

/** Callout is a banner that needs the reader's attention. */
export function Callout({
  tone,
  icon,
  title,
  children,
  action,
}: {
  tone: "danger" | "warning" | "info" | "success";
  icon: QuackIconName;
  title: ReactNode;
  children?: ReactNode;
  action?: ReactNode;
}) {
  return (
    <div role={tone === "danger" ? "alert" : undefined} data-tone={tone} className={s.callout}>
      <QuackIcon name={icon} size={24} />
      <div className={s.calloutText}>
        <p className={s.calloutTitle}>{title}</p>
        {children ? <p className={s.calloutBody}>{children}</p> : null}
      </div>
      {action ? <div className={s.calloutAction}>{action}</div> : null}
    </div>
  );
}

/** Command shows a Discord slash command inline, like a code span. */
export function Command({ children }: { children: ReactNode }) {
  return <code className={s.command}>{children}</code>;
}

/** StatTiles is a row of small counters. */
export function StatTiles({
  items,
}: {
  items: { label: string; value: number | string; tone?: "danger" | "warning"; hint?: string }[];
}) {
  return (
    <dl className={s.tiles}>
      {items.map((item) => (
        <div key={item.label} title={item.hint} className={s.tile}>
          <dt className={s.tileLabel}>{item.label}</dt>
          <dd className={s.tileValue} data-tone={item.tone}>
            {typeof item.value === "number" ? item.value.toLocaleString() : item.value}
          </dd>
        </div>
      ))}
    </dl>
  );
}
