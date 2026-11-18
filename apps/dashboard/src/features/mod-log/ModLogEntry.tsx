import { Link, useRouter } from "@tanstack/react-router";
import { ChevronRight } from "lucide-react";
import { type MouseEvent, type ReactNode, useState } from "react";

import { useUser } from "~/api/directory";
import type { ModLogEntry as Entry } from "~/api/types";
import { cx } from "~/lib/cx";
import { ago, fullDate, modLogResultTone } from "~/lib/format";
import { Avatar } from "~/ui/Avatar";
import { Badge } from "~/ui/Badge";
import { Logo } from "~/ui/Logo";
import { QuackIcon } from "~/ui/QuackIcon";
import { Skeleton } from "~/ui/States";

import { iconFor, resultLabel, sourceLabel } from "./kinds";
import s from "./ModLogEntry.module.css";

/**
 * ModLogEntry is one row of the moderation log, laid out like Discord's own
 * server history: who acted, the server's sentence for what happened,
 * when, and links to what it's about.
 */
export function ModLogEntry({ guildId, entry }: { guildId: string; entry: Entry }) {
  const [open, setOpen] = useState(false);
  const links = entryLinks(entry);

  return (
    <li className={s.entry} data-open={open || undefined}>
      <div
        className={s.head}
        onClick={(e) => {
          if ((e.target as HTMLElement).closest("a, button")) return;
          setOpen((o) => !o);
        }}
      >
        <span className={s.avatar}>
          <ActorAvatar guildId={guildId} userId={entry.actor_id} />
          <span className={s.icon}>
            <QuackIcon name={iconFor(entry.kind, entry.result)} size={16} />
          </span>
        </span>
        <div className={s.text}>
          <p className={s.sentence}>{entry.text}</p>
          <p className={s.meta}>
            <ActorName guildId={guildId} userId={entry.actor_id} />
            <span>·</span>
            <time dateTime={entry.created_at} title={fullDate(entry.created_at)}>
              {ago(entry.created_at)}
            </time>
            <span>·</span>
            <span>{sourceLabel[entry.source]}</span>
            {links.map((l) => (
              <span key={l.href} className={s.linkItem}>
                <span>·</span>
                <PathLink href={l.href} className={s.ref}>
                  {l.label}
                </PathLink>
              </span>
            ))}
          </p>
        </div>
        {entry.result !== "ok" ? (
          <Badge tone={modLogResultTone[entry.result]} dot>
            {resultLabel[entry.result]}
          </Badge>
        ) : null}
        <button
          type="button"
          aria-expanded={open}
          aria-label={open ? "Hide details" : "Show details"}
          onClick={() => setOpen((o) => !o)}
          className={s.toggle}
        >
          <ChevronRight size={18} className={s.chevron} />
        </button>
      </div>
      {open ? (
        <div className={s.details}>
          <dl className={s.grid}>
            <Row label="Recorded" value={fullDate(entry.created_at)} plain />
            <Row label="Source" value={sourceLabel[entry.source]} plain />
            {entry.actor_id ? <Row label="Done by (ID)" value={entry.actor_id} /> : null}
            {entry.member_id ? <Row label="Member ID" value={entry.member_id} /> : null}
            <Row label="Entry ID" value={entry.id} />
          </dl>
        </div>
      ) : null}
    </li>
  );
}

/** entryLinks labels the dashboard paths the API sends with an entry. */
export function entryLinks(entry: Pick<Entry, "links" | "case_number">) {
  const l = entry.links;
  const out: { href: string; label: string }[] = [];
  if (l.case)
    out.push({ href: l.case, label: entry.case_number ? `Case #${entry.case_number}` : "Case" });
  if (l.appeal) out.push({ href: l.appeal, label: "Appeal" });
  if (l.member) out.push({ href: l.member, label: "Member" });
  if (l.rule) out.push({ href: l.rule, label: "Rule" });
  if (l.ticket) out.push({ href: l.ticket, label: "Tickets" });
  return out;
}

/**
 * PathLink is a link to a dashboard path the API built. A plain click
 * navigates inside the app; a modified click opens a tab as usual.
 */
function PathLink({
  href,
  className,
  children,
}: {
  href: string;
  className?: string;
  children: ReactNode;
}) {
  const router = useRouter();
  const onClick = (e: MouseEvent<HTMLAnchorElement>) => {
    if (e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
    e.preventDefault();
    void router.navigate({ href });
  };
  return (
    <a href={href} onClick={onClick} className={className}>
      {children}
    </a>
  );
}

function Row({ label, value, plain }: { label: string; value: string; plain?: boolean }) {
  return (
    <div className={s.row}>
      <dt className={s.label}>{label}</dt>
      <dd className={cx(s.value, !plain && s.mono)}>{value}</dd>
    </div>
  );
}

function ActorAvatar({ guildId, userId }: { guildId: string; userId?: string | null }) {
  const { data: user, isPending } = useUser(guildId, userId);
  if (!userId) return <Logo size={40} />;
  if (isPending) return <Skeleton width={40} height={40} round />;
  return <Avatar src={user?.avatar_url} name={user?.display_name ?? "?"} size={40} />;
}

function ActorName({ guildId, userId }: { guildId: string; userId?: string | null }) {
  const { data: user } = useUser(guildId, userId);
  if (!userId) return <span className={s.actor}>Quack</span>;
  return (
    <Link
      to="/guilds/$guildId/members/$userId"
      params={{ guildId, userId }}
      title={user ? `@${user.username} · ${userId}` : userId}
      className={cx(s.actor, s.actorLink)}
    >
      {user?.display_name ?? `A member (ID ${userId})`}
    </Link>
  );
}
