import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { FileUp } from "lucide-react";
import { useMemo, useRef, useState } from "react";

import { ApiError } from "~/api/client";
import { keys } from "~/api/queries";
import { importRule } from "~/api/rules";
import type { RuleFile } from "~/api/types";
import { plural } from "~/lib/format";
import { Badge } from "~/ui/Badge";
import { Button } from "~/ui/Button";
import { Dialog } from "~/ui/Dialog";
import { Field, TextArea, TextInput } from "~/ui/Field";
import { InlineError } from "~/ui/States";
import { toast } from "~/ui/Toast";

import {
  fromDefinition,
  ladder,
  limits,
  parseRuleFile,
  uniqueName,
  validate,
  windowLabel,
} from "./draft";
import s from "./ImportDialog.module.css";
import { Ladder } from "./Ladder";

/**
 * ImportDialog creates a rule from a rule file exported from another server.
 * The rules manager pastes or picks the file, checks a preview of what the
 * rule does, and confirms; moderators can use the new rule right away.
 */
export function ImportDialog({
  guildId,
  open,
  onClose,
  takenNames,
}: {
  guildId: string;
  open: boolean;
  onClose: () => void;
  /** Names of the server's active rules; a new rule can't reuse one. */
  takenNames: string[];
}) {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const fileRef = useRef<HTMLInputElement>(null);
  const [text, setText] = useState("");
  const [file, setFile] = useState<RuleFile | null>(null);
  const [name, setName] = useState("");
  const [nameError, setNameError] = useState<string | null>(null);
  const [error, setError] = useState<unknown>(null);

  const close = () => {
    setText("");
    setFile(null);
    setError(null);
    setNameError(null);
    onClose();
  };

  const read = (raw: string) => {
    setError(null);
    try {
      const parsed = parseRuleFile(raw);
      setFile(parsed);
      setName(uniqueName(parsed.rule.name, takenNames));
      setNameError(null);
    } catch (e) {
      setError(new Error(e instanceof Error ? e.message : "Quack couldn't read that file."));
    }
  };

  const save = useMutation({
    mutationFn: (f: RuleFile) =>
      importRule(guildId, f, name.trim() !== f.rule.name ? name.trim() : undefined),
    onSuccess: (rule) => {
      void queryClient.invalidateQueries({ queryKey: keys.rules(guildId) });
      toast.success(`Imported ${rule.definition.name}. Moderators can use it now.`);
      close();
      void navigate({
        to: "/guilds/$guildId/rules/$ruleId",
        params: { guildId, ruleId: rule.id },
      });
    },
    onError: (e) => {
      if (e instanceof ApiError && e.code === "rule_name_taken") setNameError(e.message);
      else setError(e);
    },
  });

  const draft = useMemo(
    () => (file ? fromDefinition({ ...file.rule, name: name.trim() }) : null),
    [file, name],
  );
  const problems = useMemo(
    () =>
      draft
        ? Object.entries(validate(draft))
            .filter(([k]) => k !== "name")
            .map(([k, v]) => `${area(k)}: ${v}`)
        : [],
    [draft],
  );
  const nameTaken = takenNames.some((n) => n.trim().toLowerCase() === name.trim().toLowerCase());
  const nameProblem = !name.trim()
    ? "Give the rule a name."
    : Array.from(name.trim()).length > limits.name
      ? `Keep it to ${limits.name} characters.`
      : nameTaken
        ? "Another active rule in this server already uses this name."
        : nameError;

  return (
    <Dialog
      open={open}
      onClose={close}
      size="md"
      title={file ? `Import ${file.rule.name || "rule"}?` : "Import a rule"}
      description={
        file
          ? "It becomes a new rule in this server. Moderators can use it as soon as you import it."
          : "Paste a rule file exported from Quack, or choose the .quack-rule.json file."
      }
      onSubmit={() => {
        if (!file) read(text);
        else if (!nameProblem) save.mutate(file);
      }}
      footer={
        file ? (
          <>
            <Button
              variant="ghost"
              onClick={() => {
                setFile(null);
                setError(null);
              }}
            >
              Back
            </Button>
            <Button type="submit" disabled={Boolean(nameProblem)} pending={save.isPending}>
              Import rule
            </Button>
          </>
        ) : (
          <>
            <Button variant="ghost" onClick={close}>
              Cancel
            </Button>
            <Button type="submit" disabled={!text.trim()}>
              Preview
            </Button>
          </>
        )
      }
    >
      {file && draft ? (
        <>
          <div className={s.summary}>
            <p className={s.name}>{draft.name}</p>
            {draft.description ? <p className={s.description}>{draft.description}</p> : null}
            <div className={s.badges}>
              <Badge tone={draft.appealable ? "success" : "neutral"} dot>
                {draft.appealable ? "Appealable" : "No appeals"}
              </Badge>
              <Badge tone="neutral">{windowLabel(draft.windowed ? draft.windowDays : null)}</Badge>
              <Badge tone="neutral">
                {draft.fields.length
                  ? plural(draft.fields.length, "case context field")
                  : "No case context fields"}
              </Badge>
            </div>
            {draft.reason ? (
              <p className={s.reason}>
                <span className={s.reasonLabel}>Members see: </span>
                {draft.reason}
              </p>
            ) : null}
          </div>
          <div className={s.ladder}>
            <Ladder steps={ladder(draft.levels)} />
          </div>
          <Field
            label="Name"
            error={nameProblem ?? undefined}
            hint={
              name.trim() !== file.rule.name
                ? `This server already has an active rule called ${file.rule.name}, so the new rule needs another name.`
                : undefined
            }
          >
            {(id) => (
              <TextInput
                id={id}
                value={name}
                maxLength={limits.name}
                invalid={Boolean(nameProblem)}
                onChange={(e) => {
                  setName(e.currentTarget.value);
                  setNameError(null);
                }}
              />
            )}
          </Field>
          {problems.length > 0 ? (
            <div className={s.problems}>
              <p className={s.problemsTitle}>Quack may refuse this rule:</p>
              <ul className={s.problemList}>
                {[...new Set(problems)].map((p) => (
                  <li key={p}>{p}</li>
                ))}
              </ul>
            </div>
          ) : null}
        </>
      ) : (
        <>
          <Field label="Rule file">
            {(id) => (
              <TextArea
                id={id}
                rows={8}
                autoFocus
                spellCheck={false}
                value={text}
                placeholder='{ "format": "quack-rule", "format_version": 1, "rule": { "name": "Spam", … } }'
                className={s.mono}
                onChange={(e) => {
                  setText(e.currentTarget.value);
                  setError(null);
                }}
              />
            )}
          </Field>
          <div>
            <input
              ref={fileRef}
              type="file"
              accept=".json,application/json"
              hidden
              onChange={async (e) => {
                const file = e.currentTarget.files?.[0];
                e.currentTarget.value = "";
                if (!file) return;
                const content = await file.text();
                setText(content);
                read(content);
              }}
            />
            <Button
              variant="secondary"
              size="sm"
              icon={<FileUp size={16} />}
              onClick={() => fileRef.current?.click()}
            >
              Choose file
            </Button>
          </div>
        </>
      )}
      {error instanceof ApiError ? (
        <InlineError error={error} />
      ) : error instanceof Error ? (
        <InlineError>{error.message}</InlineError>
      ) : null}
    </Dialog>
  );
}

/** area names the part of the rule an issue path points at. */
function area(path: string): string {
  if (path.startsWith("fields")) return "Case context";
  if (path.startsWith("levels")) return "Levels";
  if (path === "window") return "Cases that count";
  if (path === "reason") return "Reason";
  if (path === "description") return "Description";
  return "Rule";
}
