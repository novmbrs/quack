import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import { ApiError, api, write } from "~/api/client";
import { useUser } from "~/api/directory";
import { appealQuery, keys } from "~/api/queries";
import type { Appeal } from "~/api/types";
import { stamp } from "~/lib/format";
import { useCan } from "~/lib/permissions";
import { Button } from "~/ui/Button";
import { ConfirmDialog } from "~/ui/ConfirmDialog";
import { toast } from "~/ui/Toast";

import {
  type Decision,
  decisionReason,
  decisionsFrom,
  defaultReason,
  reasonRequired,
} from "./decisions";
import s from "./Review.module.css";

type Copy = {
  button: string;
  variant: "success" | "danger" | "primary" | "secondary";
  title: string;
  description: (caseNumber: number) => string;
  confirm: string;
  tone: "success" | "danger" | "primary";
  label: string;
  placeholder: string;
  /** The confirmation once it's saved (design §12.9 appeal.decided.*). */
  done: (caseNumber: number, member: string) => string;
};

const reasonLabel = "Reason the member will see";

const copy: Record<Decision, Copy> = {
  accept: {
    button: "Accept",
    variant: "success",
    title: "Accept this appeal?",
    description: (n) =>
      `Quack voids case #${n}, so it no longer counts, and tries to remove any timeout or ban the case applied. A kick can't be undone. The member gets a DM with your reason.`,
    confirm: "Accept appeal",
    tone: "success",
    label: reasonLabel,
    placeholder: "Why are you accepting it?",
    done: (n, member) =>
      `Appeal accepted. Case #${n} is voided and no longer counts. Quack will try to send ${member} a DM. Open the case to check the outcome of any timeout or ban.`,
  },
  reject: {
    button: "Reject",
    variant: "danger",
    title: "Reject this appeal?",
    description: (n) => `Case #${n} still counts. The member gets a DM with your reason.`,
    confirm: "Reject appeal",
    tone: "danger",
    label: reasonLabel,
    placeholder: "Why does the case still count?",
    done: (n, member) =>
      `Appeal rejected. Case #${n} still counts. Quack will try to send ${member} a DM.`,
  },
  request_information: {
    button: "Request more information",
    variant: "secondary",
    title: "Request more information",
    description: () =>
      "Quack tries to send the member a DM with your question. They can reply on the dashboard, and the appeal comes back to the queue when they do.",
    confirm: "Send question",
    tone: "primary",
    label: "Question the member will see",
    placeholder: "What do you need to know?",
    done: (_, member) =>
      `Question saved. Quack will try to send ${member} a DM. They can reply on the dashboard.`,
  },
  close: {
    button: "Close",
    variant: "secondary",
    title: "Close this appeal?",
    description: (n) =>
      `Case #${n} still counts and the appeal leaves the queue. You can reopen it later. The member gets a DM with your reason.`,
    confirm: "Close appeal",
    tone: "primary",
    label: reasonLabel,
    placeholder: "Why are you closing it?",
    done: (_, member) => `Appeal closed. Quack will try to send ${member} a DM.`,
  },
  reopen: {
    button: "Reopen",
    variant: "primary",
    title: "Reopen this appeal?",
    description: () =>
      "Reopening asks the member for more information. The appeal comes back to the queue when they reply.",
    confirm: "Reopen and ask",
    tone: "primary",
    label: "Question the member will see",
    placeholder: "What should the member tell you?",
    done: () =>
      "Appeal reopened. Quack will try to send the member a DM. They can reply on the dashboard.",
  },
};

/**
 * StatusLine says where the appeal stands (design §12.9 appeal.post.state.*).
 */
function StatusLine({ guildId, appeal: a }: { guildId: string; appeal: Appeal }) {
  const { data: member } = useUser(guildId, a.member_id);
  const { data: actor } = useUser(guildId, a.decided_by);
  const closedByVoid = a.state === "closed" && a.events.at(-1)?.closed_by_void === true;
  const by = actor?.display_name;
  const when = a.decided_at ? ` ${stamp(a.decided_at)}` : "";
  let line: string;
  switch (a.state) {
    case "pending":
      line = "Waiting for a decision.";
      break;
    case "needs_information":
      line = `Waiting for ${member?.display_name ?? "the member"} to reply.`;
      break;
    case "accepted":
      line = by ? `Accepted by ${by}${when}.` : "Accepted.";
      break;
    case "rejected":
      line = by ? `Rejected by ${by}${when}.` : "Rejected.";
      break;
    case "closed":
      line = closedByVoid
        ? "Closed because the case was voided."
        : by
          ? `Closed by ${by}${when}.`
          : "Closed.";
      break;
  }
  const reason = !closedByVoid && a.state !== "pending" ? a.decision_reason : null;
  return (
    <p className={s.status}>
      {line}
      {reason ? <> Reason shared with the member: “{reason}”</> : null}
    </p>
  );
}

/**
 * DecisionBar sits under the conversation with the moves valid for the
 * appeal's status. Each one confirms first and collects the text the
 * member receives.
 */
export function DecisionBar({ guildId, appeal }: { guildId: string; appeal: Appeal }) {
  const can = useCan(guildId);
  const queryClient = useQueryClient();
  const [open, setOpen] = useState<Decision | null>(null);
  const [error, setError] = useState<string | null>(null);

  const { data: member } = useUser(guildId, appeal.member_id);

  const decide = useMutation({
    mutationFn: ({ decision, reason }: { decision: Decision; reason: string }) =>
      write((init) =>
        api.POST("/api/guilds/{guild_id}/appeals/{appeal_id}/decision", {
          params: { path: { guild_id: guildId, appeal_id: appeal.id } },
          body: { decision, reason, expected_version: appeal.version },
          ...init,
        }),
      ),
    onSuccess: (updated, { decision }) => {
      queryClient.setQueryData(appealQuery(guildId, appeal.id).queryKey, updated);
      void queryClient.invalidateQueries({ queryKey: keys.appeals(guildId) });
      void queryClient.invalidateQueries({ queryKey: keys.cases(guildId) });
      void queryClient.invalidateQueries({ queryKey: keys.guild(guildId) });
      toast.success(copy[decision].done(appeal.case_number, member?.display_name ?? "the member"));
      setOpen(null);
    },
    onError: (e) => {
      setError(errorText(e));
      // Someone else decided first: show their decision behind the dialog.
      if (e instanceof ApiError && e.code === "appeal_changed") {
        void queryClient.invalidateQueries({ queryKey: appealQuery(guildId, appeal.id).queryKey });
      }
    },
  });

  const decisions = can("moderate") ? decisionsFrom[appeal.state] : [];
  const current = open ? copy[open] : null;
  const required = open ? reasonRequired(open) : true;
  const fallback = open ? defaultReason(open) : undefined;

  return (
    <div className={s.bar}>
      <StatusLine guildId={guildId} appeal={appeal} />
      {decisions.length > 0 ? (
        <div className={s.buttons}>
          {decisions.map((d) => (
            <Button
              key={d}
              size="sm"
              variant={copy[d].variant}
              onClick={() => {
                setError(null);
                setOpen(d);
              }}
            >
              {copy[d].button}
            </Button>
          ))}
        </div>
      ) : null}
      <ConfirmDialog
        // Remount per decision so the reason box starts empty.
        key={open ?? "none"}
        open={open !== null}
        onClose={() => {
          setOpen(null);
          setError(null);
        }}
        title={current?.title ?? ""}
        description={current?.description(appeal.case_number)}
        confirmLabel={current?.confirm ?? ""}
        tone={current?.tone}
        reason={
          current && open
            ? {
                label: current.label,
                required,
                placeholder: current.placeholder,
                hint: fallback
                  ? `Leave out staff names and private details. Leave it blank to send “${fallback}”`
                  : "Leave out staff names and private details.",
              }
            : undefined
        }
        pending={decide.isPending}
        error={error}
        onConfirm={(text) =>
          open && decide.mutate({ decision: open, reason: decisionReason(open, text) })
        }
      />
    </div>
  );
}

/** errorText is the API's message, with the trace of an unexpected failure. */
function errorText(e: unknown): string {
  if (!(e instanceof ApiError)) return "Quack couldn't save the decision. Try again.";
  return e.unexpected && e.traceId ? `${e.message} (Trace ID ${e.traceId})` : e.message;
}
