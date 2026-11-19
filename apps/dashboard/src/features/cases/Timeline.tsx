import type { ModLogKind, ModLogResult } from "~/api/types";
import { fullDate, stamp } from "~/lib/format";
import { QuackIcon, type QuackIconName } from "~/ui/QuackIcon";

import { UserChip } from "../people/User";

import s from "./Timeline.module.css";

/**
 * TimelineItem is one line of a case's history. The text comes from the
 * server as is (design §12.9); staff views also pass who acted.
 */
export type TimelineItem = {
  at: string;
  text: string;
  /** Who acted; null means Quack. Leave it out on member views. */
  actorId?: string | null;
  result?: ModLogResult;
  kind?: ModLogKind;
};

const kindIcons: Partial<Record<ModLogKind, QuackIconName>> = {
  case_created: "case_add",
  case_voided: "case_void",
  case_context_edited: "edit",
  evidence_added: "evidence",
  action_done: "success",
  action_failed: "error",
  action_retry_requested: "retry",
  action_dismissed: "decline",
  action_undo_requested: "untimeout",
  action_undo_skipped: "info",
  dm_sent: "message",
  dm_failed: "error",
  appeal_submitted: "appeal",
  appeal_information_requested: "reply",
  appeal_information_provided: "reply",
  appeal_accepted: "accept",
  appeal_rejected: "decline",
  appeal_closed: "lock",
  appeal_reopened: "unlock",
};

function iconFor(item: TimelineItem): QuackIconName {
  if (item.result === "failed") return "error";
  if (item.result === "refused") return "warn";
  return (item.kind && kindIcons[item.kind]) || "history";
}

/**
 * Timeline lists a case's history oldest first, like a channel's history.
 * With guildId and actorId, each line shows who acted.
 */
export function Timeline({ items, guildId }: { items: TimelineItem[]; guildId?: string }) {
  const sorted = [...items].sort((a, b) => a.at.localeCompare(b.at));
  return (
    <ol className={s.list}>
      {sorted.map((e, i) => (
        <li key={`${e.at}-${i}`} className={s.item}>
          <span className={s.rail}>
            <QuackIcon name={iconFor(e)} size={20} />
          </span>
          <div className={s.body}>
            <p className={s.text}>{e.text}</p>
            <p className={s.meta}>
              {guildId && e.actorId !== undefined ? (
                <>
                  <UserChip guildId={guildId} userId={e.actorId} size={16} />
                  <span>·</span>
                </>
              ) : null}
              <time dateTime={e.at} title={fullDate(e.at)}>
                {stamp(e.at)}
              </time>
            </p>
          </div>
        </li>
      ))}
    </ol>
  );
}
