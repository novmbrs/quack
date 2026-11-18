import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import { useUser } from "~/api/directory";
import { guildMeQuery, keys } from "~/api/queries";
import type { Action } from "~/api/types";
import { actionMeta, actionOutcome, plural } from "~/lib/format";
import { Button } from "~/ui/Button";
import { ConfirmDialog } from "~/ui/ConfirmDialog";
import { QuackIcon } from "~/ui/QuackIcon";
import { errorMessage } from "~/ui/States";
import { toast } from "~/ui/Toast";

import {
  dismissDone,
  dismissLabel,
  mayHaveApplied,
  retryDone,
  retryLabel,
  undoConfirm,
  undoKind,
  undoLabel,
  undoQueued,
} from "./controls";
import { OutcomeBadge } from "./Outcome";
import { dismissAction, retryAction, undoAction } from "./writes";

import s from "./Actions.module.css";

/**
 * useActionControls runs retry, dismiss, and undo for one server and
 * refreshes everything they change: cases, the failed actions list, and the
 * failed-action count in the sidebar.
 */
export function useActionControls(guildId: string) {
  const queryClient = useQueryClient();
  const refresh = () => {
    void queryClient.invalidateQueries({ queryKey: keys.cases(guildId) });
    void queryClient.invalidateQueries({ queryKey: keys.actions(guildId) });
    void queryClient.invalidateQueries({ queryKey: guildMeQuery(guildId).queryKey });
  };
  // Retry and undo report errors where they were asked for (a dialog or a
  // toast), so they opt out of the global error toast.
  const retry = useMutation({
    mutationFn: (a: Action) => retryAction(guildId, a.id),
    onSuccess: refresh,
    onError: () => undefined,
  });
  const dismiss = useMutation({
    mutationFn: (a: Action) => dismissAction(guildId, a.id),
    onSuccess: (_, a) => {
      toast.success(dismissDone(a.kind, a.case_number));
      refresh();
    },
  });
  const undo = useMutation({
    mutationFn: (a: Action) => undoAction(guildId, a.id),
    onSuccess: refresh,
    onError: () => undefined,
  });
  return { retry, dismiss, undo };
}

/**
 * CaseActions lists each action on a case (the original and any undo) with
 * the server's outcome sentence and the controls the server allows.
 */
export function CaseActions({
  guildId,
  memberId,
  actions,
}: {
  guildId: string;
  memberId: string;
  actions: Action[];
}) {
  const { data: member } = useUser(guildId, memberId);
  const memberName = member?.display_name ?? "the member";
  const { retry, dismiss, undo } = useActionControls(guildId);
  const [confirmRetry, setConfirmRetry] = useState<Action | null>(null);
  const [undoing, setUndoing] = useState<Action | null>(null);
  const [error, setError] = useState<string | null>(null);

  if (actions.length === 0) {
    return <p className={s.none}>This case is a warning.</p>;
  }

  const runRetry = (a: Action) =>
    retry.mutate(a, {
      onSuccess: () => {
        toast.success(retryDone(a.kind, a.case_number, true));
        setConfirmRetry(null);
      },
      onError: (e) => (confirmRetry ? setError(errorMessage(e)) : toast.error(errorMessage(e))),
    });
  const undoingKind = undoing ? undoKind(undoing) : null;

  return (
    <>
      <ul className={s.list}>
        {actions.map((a) => {
          const meta = actionMeta[a.kind];
          const outcome = actionOutcome(a);
          const offered = undoKind(a);
          return (
            <li key={a.id} className={s.item}>
              <div className={s.row}>
                <QuackIcon name={meta.icon} size={22} />
                <span className={s.name}>{meta.label}</span>
                <OutcomeBadge
                  outcome={outcome}
                  moving={a.state === "pending" || a.state === "running"}
                />
              </div>
              <p className={a.state === "failed" ? s.error : s.detail}>{a.outcome_text}</p>
              {a.attempts > 1 ? <p className={s.detail}>{plural(a.attempts, "attempt")}</p> : null}
              {a.can_retry || a.can_dismiss || (a.can_undo && offered) ? (
                <div className={s.buttons}>
                  {a.can_retry ? (
                    <Button
                      size="sm"
                      pending={retry.isPending && retry.variables?.id === a.id}
                      onClick={() => (mayHaveApplied(a) ? setConfirmRetry(a) : runRetry(a))}
                    >
                      {retryLabel(a.kind)}
                    </Button>
                  ) : null}
                  {a.can_dismiss ? (
                    <Button
                      size="sm"
                      variant="secondary"
                      pending={dismiss.isPending && dismiss.variables?.id === a.id}
                      onClick={() => dismiss.mutate(a)}
                    >
                      {dismissLabel(a.kind)}
                    </Button>
                  ) : null}
                  {a.can_undo && offered ? (
                    <Button size="sm" variant="secondary" onClick={() => setUndoing(a)}>
                      {undoLabel(offered)}
                    </Button>
                  ) : null}
                </div>
              ) : null}
            </li>
          );
        })}
      </ul>
      <ConfirmDialog
        open={confirmRetry !== null}
        onClose={() => {
          setConfirmRetry(null);
          setError(null);
        }}
        title={confirmRetry ? `${retryLabel(confirmRetry.kind)}?` : "Retry?"}
        description="Quack couldn't confirm whether Discord applied this action. Check the member in Discord first, so it isn't applied twice."
        confirmLabel={confirmRetry ? retryLabel(confirmRetry.kind) : "Retry"}
        pending={retry.isPending}
        error={error}
        onConfirm={() => confirmRetry && runRetry(confirmRetry)}
      />
      <ConfirmDialog
        open={undoing !== null}
        onClose={() => {
          setUndoing(null);
          setError(null);
        }}
        title={
          undoing && undoingKind ? undoConfirm(undoingKind, memberName, undoing.case_number) : ""
        }
        description="Quack checks Discord first and leaves the timeout or ban in place if another case still applies. The case keeps counting; void it if it shouldn't."
        confirmLabel={undoingKind === "unban" ? "Yes, unban" : "Yes, remove timeout"}
        pending={undo.isPending}
        error={error}
        onConfirm={() =>
          undoing &&
          undoingKind &&
          undo.mutate(undoing, {
            onSuccess: () => {
              toast.success(undoQueued(undoingKind, memberName, undoing.case_number));
              setUndoing(null);
            },
            onError: (e) => setError(errorMessage(e)),
          })
        }
      />
    </>
  );
}
