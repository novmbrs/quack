import { useMutation, useQueryClient } from "@tanstack/react-query";
import { ChevronRight, EyeOff, Eye, Trash2 } from "lucide-react";
import { useState } from "react";

import { ApiError } from "~/api/client";
import { keys } from "~/api/queries";
import type { Case, Evidence } from "~/api/types";
import { fullDate, plural, stamp } from "~/lib/format";
import { Badge } from "~/ui/Badge";
import { Button } from "~/ui/Button";
import { ConfirmDialog } from "~/ui/ConfirmDialog";
import { Switch, TextArea } from "~/ui/Field";
import { QuackIcon } from "~/ui/QuackIcon";
import { InlineError, errorMessage } from "~/ui/States";
import { toast } from "~/ui/Toast";

import { UserChip } from "../people/User";
import { EvidenceMessage, UploadedFiles } from "./Evidence";
import { NOTE_MAX, noteLength } from "./uploads";
import { removeEvidence, setEvidenceNote, setEvidenceVisibility } from "./writes";

import s from "./EvidenceItem.module.css";

type Props = {
  guildId: string;
  caseNumber: number;
  evidence: Evidence;
  /** The case's file route, `/api/guilds/{id}/cases/{number}/evidence`. */
  files: string;
  /** The reader may change evidence (moderate). */
  moderate: boolean;
  /** Shows the case a write answered with. */
  onCase: (c: Case) => void;
};

/**
 * EvidenceItem is one item of a case's evidence as staff see it (design
 * §7.4): a saved message or uploaded files, who added it, whether the
 * member sees it, its staff note, and the messages around a saved message.
 * A removed item is listed collapsed, without controls.
 */
export function EvidenceItem(props: Props) {
  if (props.evidence.removed) return <RemovedItem {...props} />;
  return <LiveItem {...props} />;
}

function kindLabel(e: Evidence) {
  return e.kind === "upload" ? `Uploaded ${plural(e.files.length, "file")}` : "Saved message";
}

function AddedBy({ guildId, evidence: e }: { guildId: string; evidence: Evidence }) {
  return (
    <span className={s.added}>
      {e.added_by ? (
        <>
          added by <UserChip guildId={guildId} userId={e.added_by} size={18} />
        </>
      ) : (
        "added by Quack"
      )}{" "}
      <time dateTime={e.created_at} title={fullDate(e.created_at)}>
        {stamp(e.created_at)}
      </time>
    </span>
  );
}

function Body({ guildId, evidence: e, files }: Props) {
  return e.kind === "upload" ? (
    <UploadedFiles evidence={e} files={files} />
  ) : (
    <EvidenceMessage evidence={e} guildId={guildId} files={files} bare />
  );
}

function LiveItem(props: Props) {
  const { guildId, caseNumber, evidence: e, moderate, onCase } = props;
  const [removing, setRemoving] = useState(false);
  const [removeError, setRemoveError] = useState<string | null>(null);

  const visibility = useMutation({
    mutationFn: (visible: boolean) =>
      setEvidenceVisibility(guildId, String(caseNumber), e.id, visible),
    onSuccess: (done) => {
      toast.success(done.message);
      onCase(done.case);
    },
  });
  const remove = useMutation({
    mutationFn: () => removeEvidence(guildId, String(caseNumber), e.id),
    onSuccess: (done) => {
      setRemoving(false);
      toast.success(done.message);
      onCase(done.case);
    },
    onError: (err) => setRemoveError(errorMessage(err)),
  });

  const upload = e.kind === "upload";
  const visible = visibility.isPending
    ? (visibility.variables ?? e.visible_to_member)
    : e.visible_to_member;
  return (
    <article className={s.item} data-kind={e.kind}>
      <header className={s.head}>
        <QuackIcon name={upload ? "pin" : "message"} size={18} />
        <span className={s.kind}>{kindLabel(e)}</span>
        <AddedBy guildId={guildId} evidence={e} />
        <span className={s.controls}>
          <Visibility upload={upload} visible={visible} />
          {moderate && upload ? (
            <Switch
              label="Visible to member"
              checked={visible}
              disabled={visibility.isPending}
              onChange={(v) => visibility.mutate(v)}
            />
          ) : null}
          {moderate ? (
            <Button
              size="sm"
              variant="ghost"
              icon={<Trash2 size={14} />}
              onClick={() => setRemoving(true)}
              aria-label={`Remove this ${upload ? "upload" : "saved message"}`}
            >
              Remove
            </Button>
          ) : null}
        </span>
      </header>
      <Body {...props} />
      {e.surrounding?.length ? <Surrounding guildId={guildId} evidence={e} /> : null}
      <Note {...props} />
      <ConfirmDialog
        open={removing}
        onClose={() => {
          setRemoving(false);
          setRemoveError(null);
        }}
        title={`Remove this evidence from case #${caseNumber}?`}
        description={
          upload
            ? "Use this for files added by mistake. Discord and the member won't see them, and they're listed here as removed. Their copies stay in the evidence channel. To use them again, upload them again."
            : "Use this for a message added by mistake. Discord and the member won't see it, and it's listed here as removed. Adding the same message link again brings it back."
        }
        confirmLabel="Remove"
        tone="danger"
        pending={remove.isPending}
        error={removeError}
        onConfirm={() => remove.mutate()}
      />
    </article>
  );
}

/** Visibility says plainly who sees an item. */
function Visibility({ upload, visible }: { upload: boolean; visible: boolean }) {
  if (visible) {
    return (
      <Badge
        tone="info"
        icon={<Eye size={12} />}
        title={
          upload
            ? "The member sees these files on their case page."
            : "The member sees this message's text and files on their case page, never who sent it or where."
        }
      >
        Visible to member
      </Badge>
    );
  }
  return (
    <Badge
      tone="neutral"
      icon={<EyeOff size={12} />}
      title="Only staff see these files. Show them to the member when they should see them."
    >
      Staff only
    </Badge>
  );
}

/** Surrounding lists the messages saved around a message, collapsed. */
function Surrounding({ guildId, evidence: e }: { guildId: string; evidence: Evidence }) {
  const around = e.surrounding ?? [];
  const before = around.filter((m) => m.before);
  const after = around.filter((m) => !m.before);
  const line = (m: (typeof around)[number]) => (
    <li key={m.message_id} className={s.line}>
      <UserChip guildId={guildId} userId={m.author_id} size={18} />
      <time dateTime={m.created_at} title={fullDate(m.created_at)} className={s.lineTime}>
        {stamp(m.created_at)}
      </time>
      <span className={s.lineText}>
        {m.content || (m.files ? "" : "No text")}
        {m.files ? (
          <span className={s.lineFiles}> {plural(m.files, "file")} (not saved)</span>
        ) : null}
      </span>
    </li>
  );
  return (
    <details className={s.fold}>
      <summary className={s.summary}>
        <ChevronRight size={14} className={s.chevron} />
        Messages around it ({around.length})<span className={s.staffOnly}>Staff only</span>
      </summary>
      <ol className={s.lines}>
        {before.map(line)}
        <li className={s.marker}>Saved message</li>
        {after.map(line)}
      </ol>
    </details>
  );
}

/** Note shows an item's staff note and, for moderators, edits it. */
function Note({ guildId, caseNumber, evidence: e, moderate, onCase }: Props) {
  const queryClient = useQueryClient();
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const [revision, setRevision] = useState(0);
  const [error, setError] = useState<unknown>(null);

  const save = useMutation({
    mutationFn: () =>
      setEvidenceNote(guildId, String(caseNumber), e.id, draft.trim() || null, revision),
    onSuccess: (done) => {
      toast.success(done.message);
      setEditing(false);
      onCase(done.case);
    },
    onError: (err) => {
      setError(err);
      if (err instanceof ApiError && err.code === "note_changed") {
        // Someone else saved first: load their note for the next try.
        void queryClient.invalidateQueries({ queryKey: keys.cases(guildId) });
      }
    },
  });

  const start = () => {
    setDraft(e.note ?? "");
    setRevision(e.note_revision);
    setError(null);
    setEditing(true);
  };
  const length = noteLength(draft);
  const tooLong = length > NOTE_MAX;
  const conflict = error instanceof ApiError && error.code === "note_changed";
  // After a conflict, the saved note is loadable once the refetch brought a
  // newer revision than the one this draft was based on.
  const fresh = e.note_revision !== revision;

  if (editing) {
    return (
      <form
        className={s.noteForm}
        onSubmit={(ev) => {
          ev.preventDefault();
          if (!tooLong && !save.isPending) {
            setError(null);
            save.mutate();
          }
        }}
      >
        <label className={s.noteLabel} htmlFor={`note-${e.id}`}>
          Staff note
        </label>
        <TextArea
          id={`note-${e.id}`}
          rows={3}
          autoFocus
          value={draft}
          invalid={tooLong}
          placeholder="Where this came from, or what to look for. Only staff see it."
          onChange={(ev) => setDraft(ev.currentTarget.value)}
        />
        <div className={s.noteBar}>
          <span className={s.count} data-over={tooLong || undefined}>
            {length}/{NOTE_MAX}
          </span>
          <Button size="sm" variant="ghost" onClick={() => setEditing(false)}>
            Cancel
          </Button>
          {conflict ? (
            <Button size="sm" variant="secondary" onClick={start} disabled={!fresh}>
              {fresh ? "Load the saved note" : "Loading the saved note…"}
            </Button>
          ) : null}
          <Button size="sm" type="submit" disabled={tooLong} pending={save.isPending}>
            Save note
          </Button>
        </div>
        {error ? <InlineError error={error} /> : null}
      </form>
    );
  }
  if (!e.note) {
    return moderate ? (
      <Button
        size="sm"
        variant="ghost"
        className={s.addNote}
        icon={<QuackIcon name="note" size={14} />}
        onClick={start}
      >
        Add a staff note
      </Button>
    ) : null;
  }
  return (
    <div className={s.note}>
      <QuackIcon name="note" size={16} />
      <div className={s.noteBody}>
        <p className={s.noteText}>{e.note}</p>
        <p className={s.noteMeta}>
          Staff note
          {e.note_updated_by ? (
            <>
              {" · "}
              <UserChip guildId={guildId} userId={e.note_updated_by} size={16} />
            </>
          ) : null}
          {e.note_updated_at ? (
            <time dateTime={e.note_updated_at} title={fullDate(e.note_updated_at)}>
              {" "}
              {stamp(e.note_updated_at)}
            </time>
          ) : null}
        </p>
      </div>
      {moderate ? (
        <Button size="sm" variant="ghost" onClick={start}>
          Edit note
        </Button>
      ) : null}
    </div>
  );
}

/** RemovedItem is a removed item, collapsed under who removed it. */
function RemovedItem(props: Props) {
  const { evidence: e } = props;
  return (
    <details className={s.removed}>
      <summary className={s.summary}>
        <ChevronRight size={14} className={s.chevron} />
        <QuackIcon name="delete" size={16} />
        <span className={s.kind}>{kindLabel(e)}</span>
        <span className={s.removedText}>{e.removed?.text}</span>
      </summary>
      <div className={s.removedBody}>
        <Body {...props} />
        {e.surrounding?.length ? <Surrounding guildId={props.guildId} evidence={e} /> : null}
        {e.note ? <p className={s.noteText}>Staff note: {e.note}</p> : null}
      </div>
    </details>
  );
}
