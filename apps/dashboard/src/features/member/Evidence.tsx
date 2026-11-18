import { FileText } from "lucide-react";

import type { MemberCase } from "~/api/types";
import { bytes } from "~/lib/format";
import { QuackIcon } from "~/ui/QuackIcon";

import s from "~/features/cases/Evidence.module.css";

type Evidence = MemberCase["evidence"][number];

/**
 * MemberEvidence is one saved message on the member's own case: its text,
 * link previews, and files, never its author or link (design §7.4). Files
 * open through the member route, which redirects to a freshly signed copy.
 */
export function MemberEvidence({ evidence: e, caseId }: { evidence: Evidence; caseId: string }) {
  const previews = e.embeds
    .map((x) => x.title || x.type)
    .filter(Boolean)
    .join(", ");
  const fileUrl = (id: string) =>
    `/api/me/cases/${encodeURIComponent(caseId)}/evidence/${encodeURIComponent(id)}`;
  return (
    <article className={s.message}>
      <QuackIcon name="evidence" size={32} />
      <div className={s.body}>
        {e.content ? <p className={s.content}>{e.content}</p> : null}
        {previews ? <p className={s.muted}>Link previews: {previews}</p> : null}
        {e.files.length ? (
          <div className={s.attachments}>
            {e.files.map((f) =>
              (f.content_type ?? "").startsWith("image/") ? (
                <div key={f.id} className={s.attachment}>
                  <a href={fileUrl(f.id)} target="_blank" rel="noreferrer">
                    <img src={fileUrl(f.id)} alt={f.filename} loading="lazy" className={s.image} />
                  </a>
                </div>
              ) : (
                <div key={f.id} className={s.attachment}>
                  <a href={fileUrl(f.id)} target="_blank" rel="noreferrer" className={s.file}>
                    <FileText size={28} />
                    <span className={s.fileText}>
                      <span className={s.fileName}>{f.filename}</span>
                      <span className={s.time}>{bytes(f.size_bytes)}</span>
                    </span>
                  </a>
                </div>
              ),
            )}
          </div>
        ) : null}
        {!e.content && !previews && e.files.length === 0 ? (
          <p className={s.muted}>This message had no text or files Quack could save.</p>
        ) : null}
      </div>
    </article>
  );
}
