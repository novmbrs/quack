import { useMutation } from "@tanstack/react-query";
import { FileText, Upload, X } from "lucide-react";
import { type DragEvent, useEffect, useRef, useState } from "react";

import { ApiError, newKey } from "~/api/client";
import type { Case } from "~/api/types";
import { bytes, plural } from "~/lib/format";
import { Button } from "~/ui/Button";
import { Field, TextArea } from "~/ui/Field";
import { Heading } from "~/ui/Page";
import { InlineError, errorMessage } from "~/ui/States";
import { toast } from "~/ui/Toast";

import { AddEvidenceDialog } from "./CaseEdits";
import { EvidenceItem } from "./EvidenceItem";
import {
  ACCEPT,
  MAX_FILES,
  NOTE_MAX,
  noteLength,
  SUPPORTED,
  fileProgress,
  hasFiles,
  pastedFiles,
  refusal,
  refusalText,
  uploadProblem,
} from "./uploads";
import { uploadEvidence } from "./writes";

import s from "./EvidenceSection.module.css";

/**
 * EvidenceSection is a case's evidence on the staff case page: every item
 * (removed ones collapsed at the end), message links through Add message
 * links, and file uploads by dropping files on the section, pasting them,
 * or picking them. Staged files are checked here the way the server checks
 * them, and the server's refusal is shown as it says it.
 */
export function EvidenceSection({
  guildId,
  value: c,
  moderate,
  onCase,
}: {
  guildId: string;
  value: Case;
  moderate: boolean;
  onCase: (c: Case) => void;
}) {
  const [adding, setAdding] = useState(false);
  const [staged, setStaged] = useState<File[]>([]);
  const [dragging, setDragging] = useState(false);
  const picker = useRef<HTMLInputElement>(null);

  const stage = (files: File[]) => {
    if (files.length === 0) return;
    setStaged((now) => [...now, ...files]);
  };

  // Pasting files anywhere on the case page stages them, unless a dialog
  // is open (its fields take the paste).
  useEffect(() => {
    if (!moderate) return;
    const onPaste = (e: ClipboardEvent) => {
      if (document.querySelector("dialog[open]")) return;
      const files = pastedFiles(e.clipboardData);
      if (files.length === 0) return;
      e.preventDefault();
      stage(files);
    };
    document.addEventListener("paste", onPaste);
    return () => document.removeEventListener("paste", onPaste);
  }, [moderate]);

  const dragProps = moderate
    ? {
        onDragEnter: (e: DragEvent) => {
          if (!hasFiles(e.dataTransfer)) return;
          e.preventDefault();
          setDragging(true);
        },
        onDragOver: (e: DragEvent) => {
          if (!hasFiles(e.dataTransfer)) return;
          e.preventDefault();
          e.dataTransfer.dropEffect = "copy";
        },
        onDragLeave: (e: DragEvent) => {
          if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setDragging(false);
        },
        onDrop: (e: DragEvent) => {
          if (!hasFiles(e.dataTransfer)) return;
          e.preventDefault();
          setDragging(false);
          stage(Array.from(e.dataTransfer.files));
        },
      }
    : {};

  const files = `/api/guilds/${guildId}/cases/${c.number}/evidence`;
  const live = c.evidence.filter((e) => !e.removed);
  const removed = c.evidence.filter((e) => e.removed);

  return (
    <section
      className={s.section}
      data-dragging={dragging || undefined}
      aria-label="Evidence"
      {...dragProps}
    >
      <div className={s.head}>
        <Heading>Evidence</Heading>
        {moderate ? (
          <span className={s.actions}>
            <Button size="sm" variant="ghost" onClick={() => setAdding(true)}>
              Add message links
            </Button>
            <Button
              size="sm"
              variant="ghost"
              icon={<Upload size={14} />}
              onClick={() => picker.current?.click()}
            >
              Upload files
            </Button>
            <input
              ref={picker}
              type="file"
              multiple
              accept={ACCEPT}
              className={s.picker}
              tabIndex={-1}
              aria-hidden
              onChange={(e) => {
                stage(Array.from(e.currentTarget.files ?? []));
                e.currentTarget.value = "";
              }}
            />
          </span>
        ) : null}
      </div>
      {moderate && staged.length === 0 ? (
        <p className={s.hint}>
          Drop, paste, or pick files to upload them as evidence. Only staff see uploaded files until
          you show them to the member.
        </p>
      ) : null}
      {moderate && staged.length > 0 ? (
        <UploadTray
          guildId={guildId}
          caseNumber={c.number}
          files={staged}
          onRemove={(f) => setStaged((now) => now.filter((n) => n !== f))}
          onClear={() => setStaged([])}
          onDone={(updated, sent) => {
            // Files staged while the upload was sent stay for the next one.
            setStaged((now) => now.filter((f) => !sent.includes(f)));
            onCase(updated);
          }}
        />
      ) : null}
      {live.length === 0 && removed.length === 0 ? (
        <p className={s.muted}>Case #{c.number} has no evidence yet.</p>
      ) : null}
      {live.map((e) => (
        <EvidenceItem
          key={e.id}
          guildId={guildId}
          caseNumber={c.number}
          evidence={e}
          files={files}
          moderate={moderate}
          onCase={onCase}
        />
      ))}
      {removed.map((e) => (
        <EvidenceItem
          key={e.id}
          guildId={guildId}
          caseNumber={c.number}
          evidence={e}
          files={files}
          moderate={moderate}
          onCase={onCase}
        />
      ))}
      {dragging ? (
        <div className={s.drop} aria-hidden>
          <Upload size={22} />
          Drop files to upload them as evidence for case #{c.number}
        </div>
      ) : null}
      <AddEvidenceDialog
        guildId={guildId}
        caseNumber={c.number}
        open={adding}
        onClose={() => setAdding(false)}
        onCase={onCase}
      />
    </section>
  );
}

/** Staged files' list keys: two files can share a name, size, and date. */
const stagedIds = new WeakMap<File, string>();
function stagedId(f: File): string {
  let id = stagedIds.get(f);
  if (!id) {
    id = newKey();
    stagedIds.set(f, id);
  }
  return id;
}

/** Whether a failed upload may have been saved anyway (no answer, or a server fault). */
const ambiguous = (e: unknown) => !(e instanceof ApiError) || e.status === 0 || e.status >= 500;

/**
 * UploadTray lists the staged files with what's wrong with any of them, an
 * optional note, and each file's progress while the upload is sent.
 */
function UploadTray({
  guildId,
  caseNumber,
  files,
  onRemove,
  onClear,
  onDone,
}: {
  guildId: string;
  caseNumber: number;
  files: File[];
  onRemove: (f: File) => void;
  onClear: () => void;
  onDone: (c: Case, sent: File[]) => void;
}) {
  const [note, setNote] = useState("");
  // Each sent file's share sent so far, by file, while an upload is out.
  const [progress, setProgress] = useState<Map<File, number> | null>(null);
  const [error, setError] = useState<unknown>(null);
  // The key of an upload whose outcome isn't known, with what it sent: the
  // same files and note go out again under the same key, so a retry by hand
  // can't save them twice.
  const unsettled = useRef<{ key: string; batch: File[]; note: string } | null>(null);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const send = useMutation({
    mutationFn: (batch: File[]) => {
      const last = unsettled.current;
      const same =
        last !== null &&
        last.note === note.trim() &&
        last.batch.length === batch.length &&
        last.batch.every((f, i) => f === batch[i]);
      const key = same ? last.key : newKey();
      unsettled.current = { key, batch, note: note.trim() };
      return uploadEvidence(guildId, String(caseNumber), batch, note, key, (p) => {
        const done = fileProgress(
          batch.map((f) => f.size),
          p.loaded,
          p.total,
        );
        setProgress(new Map(batch.map((f, i) => [f, done[i] ?? 0])));
      });
    },
    onSuccess: (added, batch) => {
      unsettled.current = null;
      toast.success(added.message);
      setNote("");
      setProgress(null);
      onDone(added.case, batch);
    },
    onError: (e) => {
      if (!ambiguous(e)) unsettled.current = null;
      setProgress(null);
      setError(e);
      // The reader left the page: the tray can't show it, so say it here.
      if (!mounted.current) {
        toast.error(`Quack couldn't upload the files for case #${caseNumber}. ${errorMessage(e)}`);
      }
    },
  });

  const problem = uploadProblem(files, note);
  const apiError = error instanceof ApiError ? error : null;
  const fileErrors = apiError?.fields.filter((f) => f.field === "files") ?? [];
  const noteError = apiError?.field("note");
  const sending = send.isPending;
  const sent = Boolean(progress && [...progress.values()].every((p) => p >= 1));

  return (
    <form
      className={s.tray}
      onSubmit={(e) => {
        e.preventDefault();
        if (!problem && !sending) {
          setError(null);
          setProgress(new Map(files.map((f) => [f, 0])));
          send.mutate([...files]);
        }
      }}
    >
      <ul className={s.staged} aria-label="Files to upload">
        {files.map((f) => {
          const why = refusal(f);
          const p = progress?.get(f) ?? 0;
          const out = sending && progress?.has(f);
          return (
            <li key={stagedId(f)} className={s.file} data-refused={why ? true : undefined}>
              <FileText size={20} className={s.fileIcon} />
              <span className={s.fileText}>
                <span className={s.fileName}>{f.name}</span>
                <span className={s.fileMeta}>
                  {bytes(f.size)}
                  {why ? ` · ${refusalText(why)}` : null}
                  {out
                    ? ` · ${p >= 1 ? (sent ? "Saving…" : "Sent") : `${Math.round(p * 100)}%`}`
                    : sending
                      ? " · Waiting for the next upload"
                      : null}
                </span>
                {out ? (
                  <span
                    className={s.bar}
                    role="progressbar"
                    aria-label={`${f.name} upload`}
                    aria-valuemin={0}
                    aria-valuemax={100}
                    aria-valuenow={Math.round(p * 100)}
                  >
                    <span className={s.fill} style={{ width: `${p * 100}%` }} />
                  </span>
                ) : null}
              </span>
              {!sending ? (
                <button
                  type="button"
                  className={s.unstage}
                  aria-label={`Don't upload ${f.name}`}
                  onClick={() => onRemove(f)}
                >
                  <X size={14} />
                </button>
              ) : null}
            </li>
          );
        })}
      </ul>
      <Field
        label="Staff note"
        hint={`Optional. Only staff see it. ${SUPPORTED}`}
        error={noteError}
      >
        {(id) => (
          <TextArea
            id={id}
            rows={2}
            value={note}
            disabled={sending}
            invalid={Boolean(noteError) || noteLength(note) > NOTE_MAX}
            placeholder="Where these came from, or what to look for."
            onChange={(e) => setNote(e.currentTarget.value)}
          />
        )}
      </Field>
      {problem && !sending ? <p className={s.problem}>{problem}</p> : null}
      {fileErrors.length > 0 ? (
        <ul role="alert" className={s.serverErrors}>
          {fileErrors.map((f, i) => (
            <li key={i}>{f.message}</li>
          ))}
        </ul>
      ) : error && !noteError ? (
        <InlineError error={error} />
      ) : null}
      <p className={s.status} role="status" aria-live="polite">
        {sending ? (sent ? "Files sent. Quack is saving them…" : "Uploading files…") : ""}
      </p>
      <div className={s.trayBar}>
        <span className={s.count}>
          {plural(files.length, "file")} of {MAX_FILES} · one evidence item
        </span>
        <Button variant="ghost" size="sm" onClick={onClear} disabled={sending}>
          Clear
        </Button>
        <Button
          type="submit"
          size="sm"
          icon={<Upload size={14} />}
          disabled={Boolean(problem)}
          pending={sending}
        >
          Upload {plural(files.length, "file")}
        </Button>
      </div>
    </form>
  );
}
