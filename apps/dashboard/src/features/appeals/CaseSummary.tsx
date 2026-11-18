import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { ChevronRight } from "lucide-react";

import { caseQuery } from "~/api/queries";
import type { Appeal } from "~/api/types";
import { UserChip } from "~/features/people/User";
import { actionMeta, actionOutcome, ago, fullDate } from "~/lib/format";
import { Badge } from "~/ui/Badge";
import { QuackIcon } from "~/ui/QuackIcon";
import { Skeleton } from "~/ui/States";
import s from "./CaseSummary.module.css";

/**
 * CaseSummary is the case under appeal in one card: what it did and why,
 * linking to the full case. The reason comes from the case page's cache
 * entry; everything else is on the appeal.
 */
export function CaseSummary({ guildId, appeal }: { guildId: string; appeal: Appeal }) {
  const c = appeal.case;
  const caseRef = String(c.number);
  const full = useQuery(caseQuery(guildId, caseRef));
  const voided = Boolean(c.voided_at);
  const action = c.action;

  return (
    <Link to="/guilds/$guildId/cases/$caseRef" params={{ guildId, caseRef }} className={s.card}>
      <QuackIcon name={voided ? "case_void" : "case"} size={28} />
      <div className={s.body}>
        <div className={s.top}>
          <span className={s.title}>
            Case #{c.number}
            <span className={s.rule}> · {c.rule_name}</span>
          </span>
          {voided ? (
            <Badge tone="neutral" dot>
              Voided
            </Badge>
          ) : action ? (
            <Badge tone={actionOutcome(action).tone}>
              {actionMeta[action.kind].label} · {action.outcome_text}
            </Badge>
          ) : (
            <Badge tone="neutral">Warning</Badge>
          )}
        </div>
        {full.isPending ? (
          <Skeleton width="70%" height={12} />
        ) : full.data ? (
          <p className={s.reason}>{full.data.reason}</p>
        ) : null}
        <p className={s.meta}>
          {c.level_index ? <span>Level {c.level_index}</span> : null}
          {c.level_index ? <span>·</span> : null}
          {c.moderator_id ? (
            <>
              <span>by</span>
              <UserChip guildId={guildId} userId={c.moderator_id} size={16} link={false} />
              <span>·</span>
            </>
          ) : null}
          <time dateTime={c.created_at} title={fullDate(c.created_at)}>
            {ago(c.created_at)}
          </time>
        </p>
      </div>
      <ChevronRight size={20} className={s.chevron} />
    </Link>
  );
}
