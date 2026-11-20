import { ExternalLink, FileText } from "lucide-react";

import { useUser } from "~/api/directory";
import type { Evidence, EvidenceFile } from "~/api/types";
import { cx } from "~/lib/cx";
import { bytes, stamp } from "~/lib/format";
import { Avatar } from "~/ui/Avatar";
import { QuackIcon } from "~/ui/QuackIcon";

import s from "./Evidence.module.css";

/**
 * EvidenceMessage renders a saved Discord message the way Discord shows it,
 * from Quack's snapshot, so it still reads right after the original is
 * edited or deleted. Saving and failure states use the server's text.
 */
export function EvidenceMessage({
  evidence,
  files,
  guildId,
  bare,
}: {
  evidence: Evidence;
  /** Drops the frame, for a message shown inside an evidence item. */
  bare?: boolean;
  /**
   * The case's evidence file route, such as `/api/guilds/{id}/cases/{ref}/evidence`.
   * It redirects to a freshly signed copy, since stored Discord links expire.
   */
  files: string;
  guildId: string;
}) {
  const { data: author, isPending } = useUser(guildId, evidence.author_id);
  const name =
    author?.display_name ??
    (evidence.author_id
      ? isPending
        ? evidence.author_id
        : `Unknown member (ID ${evidence.author_id})`
      : "Message");
  const time = evidence.message_created_at ?? evidence.created_at;
  const previews = evidence.embeds
    .map((e) => e.title || e.type)
    .filter(Boolean)
    .join(", ");

  return (
    <article className={cx(s.message, bare && s.bare)}>
      <Avatar src={author?.avatar_url} name={author?.display_name ?? "?"} size={40} />
      <div className={s.body}>
        <header className={s.meta}>
          <span className={s.author}>{name}</span>
          <time dateTime={time} className={s.time}>
            {stamp(time)}
          </time>
          {evidence.message_edited_at ? <span className={s.time}>(edited)</span> : null}
          {evidence.url ? (
            <a href={evidence.url} target="_blank" rel="noreferrer" className={s.jump}>
              Open message <ExternalLink size={12} />
            </a>
          ) : null}
        </header>
        {evidence.state === "saved" ? (
          evidence.content ? (
            <p className={s.content}>{evidence.content}</p>
          ) : null
        ) : evidence.state === "saving" ? (
          <p className={s.muted}>{evidence.state_text}</p>
        ) : (
          <p className={s.warning}>
            <QuackIcon name="warn" size={14} /> {evidence.state_text}
          </p>
        )}
        {previews ? <p className={s.muted}>Link previews: {previews}</p> : null}
        {evidence.files.length ? (
          <div className={s.attachments}>
            {evidence.files.map((f) => (
              <FileView key={f.id} file={f} url={`${files}/${encodeURIComponent(f.id)}`} />
            ))}
          </div>
        ) : null}
        {evidence.files_not_saved > 0 ? (
          <p className={s.warning}>
            <QuackIcon name="warn" size={14} /> {filesNotSaved(evidence.files_not_saved)}
          </p>
        ) : null}
      </div>
    </article>
  );
}

/** filesNotSaved is design §12.6 evidence.files_not_saved. */
export function filesNotSaved(count: number): string {
  const lead =
    count === 1 ? "1 more file wasn't saved" : `${count.toLocaleString()} more files weren't saved`;
  return `${lead}, because a case keeps at most 100 files and 10 per message.`;
}

/**
 * UploadedFiles is the body of an uploaded-files item: its files, each
 * served through the case's file route once copied, with the copy's state.
 */
export function UploadedFiles({ evidence, files }: { evidence: Evidence; files: string }) {
  return (
    <div className={s.uploads}>
      {evidence.state === "failed" ? (
        <p className={s.warning}>
          <QuackIcon name="warn" size={14} /> {evidence.state_text}
        </p>
      ) : null}
      <div className={s.grid}>
        {evidence.files.map((f) => (
          <FileView
            key={f.id}
            file={f}
            url={`${files}/${encodeURIComponent(f.id)}`}
            servable={f.copy_state === "copied"}
          />
        ))}
      </div>
    </div>
  );
}

/**
 * FileView is one evidence file: an image preview or a file tile. A file
 * that can't be served yet (an upload before its copy) shows as a plain
 * tile with its state.
 */
export function FileView({
  file: f,
  url,
  servable = true,
}: {
  file: EvidenceFile;
  url: string;
  servable?: boolean;
}) {
  const image = (f.content_type ?? "").startsWith("image/");
  const tile = (
    <>
      <FileText size={28} />
      <span className={s.fileText}>
        <span className={cx(s.fileName, !servable && s.fileNamePlain)}>{f.filename}</span>
        <span className={s.time}>{bytes(f.size_bytes)}</span>
      </span>
    </>
  );
  return (
    <div className={s.attachment}>
      {!servable ? (
        <div className={s.file}>{tile}</div>
      ) : image ? (
        <a href={url} target="_blank" rel="noreferrer">
          <img src={url} alt={f.filename} loading="lazy" className={s.image} />
        </a>
      ) : (
        <a href={url} target="_blank" rel="noreferrer" className={s.file}>
          {tile}
        </a>
      )}
      {f.copy_state !== "copied" ? (
        <p className={cx(s.warning, f.copy_state === "pending" && s.pending)}>
          {f.copy_state === "pending" ? null : <QuackIcon name="warn" size={14} />}
          {f.state_text}
        </p>
      ) : null}
    </div>
  );
}
