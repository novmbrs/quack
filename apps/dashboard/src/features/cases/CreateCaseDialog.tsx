import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useEffect, useState } from "react";

import { ApiError } from "~/api/client";
import { casePreviewQuery, guildMeQuery, keys, ruleQuery, rulesQuery } from "~/api/queries";
import type { CasePreview, LevelAction } from "~/api/types";
import { Button } from "~/ui/Button";
import { Dialog } from "~/ui/Dialog";
import { Field, Select, TextArea } from "~/ui/Field";
import { QuackIcon, type QuackIconName } from "~/ui/QuackIcon";
import { errorMessage, Skeleton, TraceId } from "~/ui/States";
import { toast } from "~/ui/Toast";

import { MemberPicker } from "../people/MemberPicker";
import {
  type ContextValues,
  contextFields,
  contextPayload,
  evidenceLinks,
  missingRequired,
} from "./context";
import { ContextForm } from "./ContextForm";
import { describePreview } from "./preview";
import { createSubmissionGate } from "./submission";
import { addCase } from "./writes";

import s from "./CreateCaseDialog.module.css";

/**
 * CreateCaseDialog applies a rule to a member. It shows the level Quack
 * would choose from the server's preview, so the moderator knows what will
 * happen before they add the case.
 */
export function CreateCaseDialog({
  guildId,
  open,
  onClose,
  initialMember,
  replacesCaseNumber,
}: {
  guildId: string;
  open: boolean;
  onClose: () => void;
  initialMember?: string;
  /** Adds the case as the replacement of this voided case. */
  replacesCaseNumber?: number;
}) {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const rules = useQuery({ ...rulesQuery(guildId), enabled: open });
  const active = rules.data ?? [];

  const [member, setMember] = useState<string | null>(initialMember ?? null);
  const [ruleId, setRuleId] = useState("");
  const [values, setValues] = useState<ContextValues>({});
  const [links, setLinks] = useState("");
  const [error, setError] = useState<unknown>(null);
  const [submission] = useState(createSubmissionGate);
  useEffect(() => {
    if (open) submission.reset();
  }, [open, submission]);

  const chosen =
    active.find((r) => r.id === ruleId) ?? (active.length === 1 ? active[0] : undefined);
  const rule = useQuery({
    ...ruleQuery(guildId, chosen?.id ?? ""),
    enabled: open && Boolean(chosen),
  });
  const preview = useQuery({
    ...casePreviewQuery(guildId, member ?? "", chosen?.id ?? ""),
    enabled: open && Boolean(member && chosen),
  });
  const fields = rule.data ? contextFields(rule.data.definition.fields) : [];

  const create = useMutation({
    mutationFn: () =>
      addCase(guildId, {
        member_id: member!,
        rule_id: chosen!.id,
        rule_version: preview.data?.rule_version,
        context: contextPayload(fields, values),
        evidence_links: evidenceLinks(links),
        replaces_case_number: replacesCaseNumber,
      }),
    onSuccess: (created) => {
      void queryClient.invalidateQueries({ queryKey: keys.cases(guildId) });
      void queryClient.invalidateQueries({ queryKey: guildMeQuery(guildId).queryKey });
      toast.success(`Case #${created.number} is saved.`);
      onClose();
      void navigate({
        to: "/guilds/$guildId/cases/$caseRef",
        params: { guildId, caseRef: String(created.number) },
      });
    },
    onError: (e) => {
      setError(e);
      // The rule changed after the preview: show the new level before trying again.
      if (e instanceof ApiError && e.code === "rule_changed") {
        void rule.refetch();
        void preview.refetch();
      }
    },
  });

  const apiError = error instanceof ApiError ? error : null;
  const fieldError = (path: string) => apiError?.field(path);
  // Messages that belong to a field on the form show there; anything else
  // shows below the form.
  const shownOnFields =
    apiError !== null &&
    apiError.fields.length > 0 &&
    apiError.fields.every((f) => /^(member_id|rule_id|context\.|evidence_links)/.test(f.field));
  const general = error && !shownOnFields ? error : null;

  const missing = missingRequired(fields, values);
  const ready = Boolean(member && chosen && rule.data && preview.data && missing.length === 0);

  return (
    <Dialog
      open={open}
      onClose={onClose}
      size="md"
      title={replacesCaseNumber ? `Add a case to replace case #${replacesCaseNumber}` : "Add case"}
      description="Choose the member and the rule. Quack chooses the level from the member's case count."
      onSubmit={() => {
        if (!open || !ready || create.isPending) return;
        void submission
          .submit(() => {
            setError(null);
            return create.mutateAsync();
          })
          .catch(() => {
            // The mutation callback shows the error inline.
          });
      }}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button type="submit" disabled={!ready} pending={create.isPending}>
            Add case
          </Button>
        </>
      }
    >
      <Field label="Member" required error={fieldError("member_id")}>
        {(id) => (
          <MemberPicker
            id={id}
            guildId={guildId}
            value={member}
            onChange={setMember}
            autoFocus={!initialMember}
          />
        )}
      </Field>

      <Field label="Rule" required error={fieldError("rule_id")}>
        {(id) =>
          rules.isPending ? (
            <Skeleton height={40} />
          ) : active.length === 0 ? (
            <p className={s.previewSub}>
              This server has no active rules yet. Ask a rules manager to create one.
            </p>
          ) : (
            <Select
              id={id}
              value={chosen?.id ?? ""}
              onChange={(e) => {
                setRuleId(e.currentTarget.value);
                setValues({});
              }}
            >
              <option value="" disabled>
                Choose a rule
              </option>
              {active.map((r) => (
                <option key={r.id} value={r.id}>
                  {r.name}
                </option>
              ))}
            </Select>
          )
        }
      </Field>

      {chosen && member ? (
        <Preview preview={preview.data} pending={preview.isPending} error={preview.error} />
      ) : null}

      <ContextForm
        fields={fields}
        values={values}
        onChange={setValues}
        errors={(key) => fieldError(`context.${key}`)}
      />

      {chosen ? (
        <Field
          label="Evidence"
          hint="Paste Discord message links, one per line. Quack saves a copy of each message and its files."
          error={fieldError("evidence_links")}
        >
          {(id) => (
            <TextArea
              id={id}
              rows={2}
              value={links}
              placeholder="https://discord.com/channels/…"
              onChange={(e) => setLinks(e.currentTarget.value)}
            />
          )}
        </Field>
      ) : null}

      {general ? (
        <p role="alert" className={s.error}>
          {errorMessage(general)} <TraceId error={general} />
        </p>
      ) : null}
    </Dialog>
  );
}

function actionIcon(action: LevelAction | null | undefined): QuackIconName {
  if (!action) return "warn";
  return action.kind;
}

function Preview({
  preview,
  pending,
  error,
}: {
  preview: CasePreview | undefined;
  pending: boolean;
  error: unknown;
}) {
  if (pending) {
    return (
      <div className={s.preview}>
        <Skeleton height={36} />
      </div>
    );
  }
  if (!preview) {
    return error ? (
      <p role="alert" className={s.error}>
        {errorMessage(error)}
      </p>
    ) : null;
  }
  const d = describePreview(preview);
  return (
    <div className={s.preview} aria-live="polite">
      <QuackIcon name={actionIcon(preview.level.action)} size={28} />
      <div className={s.previewText}>
        <p className={s.previewTitle}>
          {d.title}
          <span className={s.previewLevel}> · {d.level}</span>
        </p>
        <p className={s.previewSub}>{d.detail}</p>
      </div>
    </div>
  );
}
