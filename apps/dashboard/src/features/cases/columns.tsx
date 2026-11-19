import { Link } from "@tanstack/react-router";

import type { CaseSummary } from "~/api/types";
import { ago, fullDate } from "~/lib/format";
import { Badge } from "~/ui/Badge";
import { columnsFor } from "~/ui/DataTable";

import { UserChip } from "../people/User";
import { Outcome } from "./Outcome";
import { cx } from "~/lib/cx";

import s from "./columns.module.css";

const col = columnsFor<CaseSummary>();

/**
 * caseColumns is the staff case table: number, member, rule, outcome,
 * moderator, and age. A member's profile leaves out the member column, since
 * every row is theirs. Call it inside useMemo keyed on its arguments.
 */
export function caseColumns(guildId: string, { member = true }: { member?: boolean } = {}) {
  return col.columns([
    col.accessor("number", {
      header: "Case",
      meta: { width: 84 },
      cell: (c) => (
        <Link
          to="/guilds/$guildId/cases/$caseRef"
          params={{ guildId, caseRef: String(c.getValue()) }}
          className={s.number}
        >
          #{c.getValue()}
        </Link>
      ),
    }),
    ...(member
      ? [
          col.accessor("member_id", {
            header: "Member",
            meta: { width: "20%" },
            cell: (c) => <UserChip guildId={guildId} userId={c.getValue()} />,
          }),
        ]
      : []),
    col.display({
      id: "rule",
      header: "Rule",
      cell: (c) => {
        const row = c.row.original;
        return (
          <div className={s.stack}>
            <span className={cx(s.rule, row.voided_at && s.voided)}>{row.rule_name}</span>
            <span className={s.sub}>
              {row.source === "imported"
                ? "Imported"
                : row.level_index
                  ? `Level ${row.level_index}`
                  : ""}
            </span>
          </div>
        );
      },
    }),
    col.display({
      id: "outcome",
      header: "Outcome",
      meta: { width: 160 },
      cell: (c) =>
        c.row.original.voided_at ? (
          <Badge tone="neutral" dot>
            Voided
          </Badge>
        ) : (
          <Outcome action={c.row.original.action} compact />
        ),
    }),
    col.accessor("moderator_id", {
      header: "Moderator",
      meta: { width: "18%", hideWhenNarrow: true },
      cell: (c) => <UserChip guildId={guildId} userId={c.getValue()} size={20} />,
    }),
    col.accessor("created_at", {
      header: "When",
      meta: { width: 120, align: "end", hideOnMobile: true },
      cell: (c) => (
        <time dateTime={c.getValue()} title={fullDate(c.getValue())} className={s.sub}>
          {ago(c.getValue())}
        </time>
      ),
    }),
  ]);
}
