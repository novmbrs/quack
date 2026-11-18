import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import { ApiError } from "~/api/client";
import { keys } from "~/api/queries";
import type { Case } from "~/api/types";
import { Button } from "~/ui/Button";
import { Dialog } from "~/ui/Dialog";
import { Field, TextArea } from "~/ui/Field";
import { errorMessage, TraceId } from "~/ui/States";
import { toast } from "~/ui/Toast";

import {
  type ContextValues,
  contextFields,
  contextPayload,
  evidenceLinks,
  missingRequired,
} from "./context";
import { ContextForm } from "./ContextForm";
import { addEvidence, editContext } from "./writes";

import s from "./CreateCaseDialog.module.css";

/**
 * EditContextDialog replaces a case's context. It sends the revision it was
 * opened at, so a save that would overwrite someone else's edit is refused
 * (409 context_changed) and the page reloads the case.
 */
export function EditContextDialog({
  guildId,
  value: c,
  open,
  onClose,
}: {
  guildId: string;
  value: Case;
  open: boolean;
  onClose: () => void;
}) {
  const queryClient = useQueryClient();
  const fields = contextFields(c.fields);
  const [values, setValues] = useState<ContextValues>(() => ({ ...c.context }));
  const [revision, setRevision] = useState(c.context_revision);
  const [error, setError] = useState<unknown>(null);

  const save = useMutation({
    mutationFn: () =>
      editContext(guildId, String(c.number), contextPayload(fields, values), revision),
    onSuccess: (saved) => {
      void queryClient.invalidateQueries({ queryKey: keys.cases(guildId) });
      const added = saved.evidence.length - c.evidence.length;
      toast.success(
        added > 0
          ? `Case context saved for case #${c.number}. Quack is saving ${added === 1 ? "1 new message" : `${added} new messages`} as evidence.`
          : `Case context saved for case #${c.number}.`,
      );
      onClose();
    },
    onError: (e) => {
      setError(e);
      if (e instanceof ApiError && e.code === "context_changed") {
        // Someone else saved first: load their version so the next save
        // starts from it.
        void queryClient.invalidateQueries({ queryKey: keys.cases(guildId) });
      }
    },
  });

  const apiError = error instanceof ApiError ? error : null;
  const fieldError = (key: string) => apiError?.field(`context.${key}`);
  const conflict = apiError?.code === "context_changed";

  return (
    <Dialog
      open={open}
      onClose={onClose}
      size="md"
      title={`Edit context · case #${c.number}`}
      description="The member can see the case context, except message links."
      onSubmit={() => {
        if (missingRequired(fields, values).length === 0 && !save.isPending) {
          setError(null);
          save.mutate();
        }
      }}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          {conflict ? (
            <Button
              variant="secondary"
              onClick={() => {
                setValues({ ...c.context });
                setRevision(c.context_revision);
                setError(null);
              }}
            >
              Load the saved context
            </Button>
          ) : null}
          <Button
            type="submit"
            disabled={missingRequired(fields, values).length > 0}
            pending={save.isPending}
          >
            Save context
          </Button>
        </>
      }
    >
      <ContextForm fields={fields} values={values} onChange={setValues} errors={fieldError} />
      {error && !(apiError && apiError.fields.length > 0) ? (
        <p role="alert" className={s.error}>
          {errorMessage(error)} <TraceId error={error} />
        </p>
      ) : null}
    </Dialog>
  );
}

/** AddEvidenceDialog saves up to 10 more Discord messages as evidence on a case. */
export function AddEvidenceDialog({
  guildId,
  caseNumber,
  open,
  onClose,
}: {
  guildId: string;
  caseNumber: number;
  open: boolean;
  onClose: () => void;
}) {
  const queryClient = useQueryClient();
  const [text, setText] = useState("");
  const [error, setError] = useState<unknown>(null);
  const links = evidenceLinks(text);

  const add = useMutation({
    mutationFn: () => addEvidence(guildId, String(caseNumber), links),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: keys.cases(guildId) });
      toast.success(
        links.length === 1
          ? `Quack is saving 1 message as evidence for case #${caseNumber}.`
          : `Quack is saving ${links.length} messages as evidence for case #${caseNumber}.`,
      );
      setText("");
      onClose();
    },
    onError: setError,
  });

  const apiError = error instanceof ApiError ? error : null;
  return (
    <Dialog
      open={open}
      onClose={onClose}
      size="md"
      title={`Add evidence · case #${caseNumber}`}
      onSubmit={() => {
        if (links.length > 0 && links.length <= 10 && !add.isPending) {
          setError(null);
          add.mutate();
        }
      }}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button
            type="submit"
            disabled={links.length === 0 || links.length > 10}
            pending={add.isPending}
          >
            Add evidence
          </Button>
        </>
      }
    >
      <Field
        label="Message links"
        hint="One Discord message link per line, up to 10."
        error={
          apiError?.field("links") ??
          (links.length > 10 ? "Add at most 10 links at a time." : undefined)
        }
      >
        {(id) => (
          <TextArea
            id={id}
            rows={4}
            autoFocus
            value={text}
            placeholder="https://discord.com/channels/…"
            onChange={(e) => setText(e.currentTarget.value)}
          />
        )}
      </Field>
      {error && !apiError?.field("links") ? (
        <p role="alert" className={s.error}>
          {errorMessage(error)} <TraceId error={error} />
        </p>
      ) : null}
    </Dialog>
  );
}
