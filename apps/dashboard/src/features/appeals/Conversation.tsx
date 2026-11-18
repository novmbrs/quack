import type { ReactNode } from "react";

import { cx } from "~/lib/cx";
import { useUser } from "~/api/directory";
import type { Appeal, AppealEvent, AppealEventKind } from "~/api/types";
import { fullDate, stamp } from "~/lib/format";
import { Avatar } from "~/ui/Avatar";
import { Badge } from "~/ui/Badge";
import { QuackIcon, type QuackIconName } from "~/ui/QuackIcon";
import { Skeleton } from "~/ui/States";
import s from "./Conversation.module.css";

/**
 * How decisions read as system lines in the conversation (design §12.9,
 * appeal.event, staff view). An event without a known actor reads "The
 * moderators …".
 */
const decisionLines: Partial<Record<AppealEventKind, { verb: string; icon: QuackIconName }>> = {
  accepted: {
    verb: "accepted the appeal. The case was voided and no longer counts.",
    icon: "accept",
  },
  rejected: { verb: "rejected the appeal. The case still counts.", icon: "decline" },
  closed: { verb: "closed the appeal.", icon: "lock" },
};

/**
 * Conversation shows an appeal the way it happened, as a Discord channel:
 * the member's statement first, then each request, reply, and decision.
 */
export function Conversation({ guildId, appeal }: { guildId: string; appeal: Appeal }) {
  const events = [...appeal.events]
    .filter((e) => e.kind !== "submitted")
    .sort((a, b) => a.created_at.localeCompare(b.created_at));

  return (
    <ol className={s.list}>
      <Message
        guildId={guildId}
        userId={appeal.member_id}
        at={appeal.created_at}
        tag={<Badge tone="brand">Appeal statement</Badge>}
      >
        {appeal.statement}
      </Message>
      {events.map((e) => (
        <Event key={e.id} guildId={guildId} event={e} memberId={appeal.member_id} />
      ))}
    </ol>
  );
}

function Event({
  guildId,
  event: e,
  memberId,
}: {
  guildId: string;
  event: AppealEvent;
  memberId: string;
}) {
  if (e.kind === "closed" && e.closed_by_void) {
    return (
      <li className={s.system}>
        <span className={s.systemIcon}>
          <QuackIcon name="case_void" size={20} />
        </span>
        <div className={s.systemBody}>
          <p className={s.systemLine}>
            The appeal was closed because the case was voided.
            <Time at={e.created_at} />
          </p>
        </div>
      </li>
    );
  }
  const decision = decisionLines[e.kind];
  if (decision) {
    return (
      <li className={s.system}>
        <span className={s.systemIcon}>
          <QuackIcon name={decision.icon} size={20} />
        </span>
        <div className={s.systemBody}>
          <p className={s.systemLine}>
            <Name guildId={guildId} userId={e.actor_id} fallback="The moderators" /> {decision.verb}
            <Time at={e.created_at} />
          </p>
          {e.text ? (
            <blockquote className={s.quote}>
              <span className={s.quoteLabel}>Reason shared with the member</span>
              {e.text}
            </blockquote>
          ) : null}
        </div>
      </li>
    );
  }
  // The member's own events are theirs; everything else is staff.
  const fromMember = e.kind === "information_provided";
  const tag =
    e.kind === "information_requested" ? (
      <Badge tone="info">Asked for more information</Badge>
    ) : e.kind === "reopened" ? (
      <Badge tone="info">Reopened and asked for more information</Badge>
    ) : fromMember ? (
      <Badge tone="neutral">Reply from the member</Badge>
    ) : null;
  return (
    <Message
      guildId={guildId}
      userId={fromMember ? memberId : (e.actor_id ?? undefined)}
      at={e.created_at}
      tag={tag}
      staff={!fromMember}
    >
      {e.text}
    </Message>
  );
}

function Message({
  guildId,
  userId,
  at,
  tag,
  staff,
  children,
}: {
  guildId: string;
  userId?: string;
  at: string;
  tag?: ReactNode;
  staff?: boolean;
  children: ReactNode;
}) {
  const { data: user, isPending } = useUser(guildId, userId);
  const name =
    user?.display_name ?? (staff ? (userId ? "A moderator" : "The moderators") : "Member");
  return (
    <li className={cx(s.message, staff && s.staff)}>
      {userId && isPending ? (
        <Skeleton width={40} height={40} round />
      ) : (
        <Avatar src={user?.avatar_url} name={name} size={40} />
      )}
      <div className={s.body}>
        <header className={s.meta}>
          <span className={s.author}>
            {userId && isPending ? <Skeleton width={90} height={12} /> : name}
          </span>
          {tag}
          <Time at={at} />
        </header>
        <p className={s.content}>{children}</p>
      </div>
    </li>
  );
}

function Name({
  guildId,
  userId,
  fallback,
}: {
  guildId: string;
  userId?: string | null;
  fallback: string;
}) {
  const { data: user } = useUser(guildId, userId);
  return <strong className={s.strong}>{user?.display_name ?? fallback}</strong>;
}

function Time({ at }: { at: string }) {
  return (
    <time dateTime={at} title={fullDate(at)} className={s.time}>
      {stamp(at)}
    </time>
  );
}
