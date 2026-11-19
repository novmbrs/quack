import { useQuery } from "@tanstack/react-query";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { Check, Copy, Plus, ScrollText, UserRound } from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import { useUser } from "~/api/directory";
import { useCursorPager } from "~/api/paging";
import { casesQuery, memberSummaryQuery } from "~/api/queries";
import type { CaseSummary, MemberSummary } from "~/api/types";
import { caseColumns } from "~/features/cases/columns";
import { CreateCaseDialog } from "~/features/cases/CreateCaseDialog";
import { plural } from "~/lib/format";
import { useCan } from "~/lib/permissions";
import { Avatar } from "~/ui/Avatar";
import { Badge } from "~/ui/Badge";
import { Button, ButtonLink } from "~/ui/Button";
import { DataTable, features, Pager, useTable } from "~/ui/DataTable";
import { Heading, Page, Panel } from "~/ui/Page";
import { Empty, ErrorState, Skeleton, SkeletonRows } from "~/ui/States";
import { toast } from "~/ui/Toast";
import { Tooltip } from "~/ui/Tooltip";

import s from "./member.module.css";

const PAGE = 50;

export const Route = createFileRoute("/_authed/guilds/$guildId/members/$userId")({
  loader: ({ context, params }) =>
    Promise.all([
      context.queryClient.ensureQueryData(memberSummaryQuery(params.guildId, params.userId)),
      context.queryClient.ensureQueryData(
        casesQuery(params.guildId, { member_id: params.userId, limit: PAGE }),
      ),
    ]),
  component: MemberPage,
  errorComponent: ({ error, reset }) => (
    <Page icon={<UserRound size={20} />} title="Member">
      <ErrorState error={error} retry={reset} />
    </Page>
  ),
});

function MemberPage() {
  const { guildId, userId } = Route.useParams();
  const navigate = useNavigate({ from: Route.fullPath });
  const can = useCan(guildId);
  const [creating, setCreating] = useState(false);

  const user = useUser(guildId, userId);
  const pager = useCursorPager(userId);
  const summary = useQuery(memberSummaryQuery(guildId, userId));
  const cases = useQuery(casesQuery(guildId, { member_id: userId, limit: PAGE }, pager.cursor));
  const columns = useMemo(() => caseColumns(guildId, { member: false }), [guildId]);
  const table = useTable({ features, columns, data: cases.data?.cases ?? EMPTY });

  const name = user.data?.display_name ?? (user.isPending ? "" : "Unknown member");

  return (
    <Page
      icon={<UserRound size={20} />}
      title={name || "Member"}
      topic="Case history in this server"
      actions={
        <>
          <ButtonLink
            to="/guilds/$guildId/mod-log"
            params={{ guildId }}
            search={{ member: userId }}
            variant="ghost"
            size="sm"
            icon={<ScrollText size={16} />}
          >
            Open in moderation log
          </ButtonLink>
          {can("moderate") ? (
            <Button size="sm" icon={<Plus size={16} />} onClick={() => setCreating(true)}>
              Add case
            </Button>
          ) : null}
        </>
      }
    >
      <Panel padded={false} className={s.card}>
        <div className={s.profile}>
          <div className={s.avatar}>
            {user.isPending ? (
              <Skeleton width={88} height={88} round />
            ) : (
              <Avatar src={user.data?.avatar_url} name={name} size={72} />
            )}
          </div>
          <div className={s.identity}>
            {user.isPending ? (
              <>
                <Skeleton width={180} height={22} />
                <Skeleton width={120} height={14} />
              </>
            ) : (
              <>
                <h2 className={s.name}>{name}</h2>
                {user.data ? <p className={s.username}>@{user.data.username}</p> : null}
              </>
            )}
            <div className={s.tags}>
              {user.data ? (
                user.data.in_server ? (
                  <Badge tone="success" dot>
                    In this server
                  </Badge>
                ) : (
                  <Badge tone="neutral" dot>
                    Not in this server
                  </Badge>
                )
              ) : null}
              {user.data?.bot ? <Badge tone="brand">Bot</Badge> : null}
              <CopyId id={userId} />
            </div>
          </div>
        </div>
      </Panel>

      {cases.isPending ? (
        <SkeletonRows rows={8} />
      ) : cases.isError ? (
        <ErrorState error={cases.error} retry={() => void cases.refetch()} />
      ) : (
        <>
          {summary.data ? <Summary summary={summary.data} /> : null}
          <section className={s.block}>
            <Heading>Cases</Heading>
            <DataTable
              table={table}
              dim={cases.isPlaceholderData}
              onRowClick={(row, newTab) => {
                if (newTab) {
                  window.open(`/guilds/${guildId}/cases/${row.number}`, "_blank");
                  return;
                }
                void navigate({
                  to: "/guilds/$guildId/cases/$caseRef",
                  params: { guildId, caseRef: String(row.number) },
                });
              }}
              empty={
                <Empty
                  icon="case"
                  title="No cases"
                  action={
                    can("moderate") ? (
                      <Button variant="secondary" onClick={() => setCreating(true)}>
                        Add case
                      </Button>
                    ) : null
                  }
                >
                  This member has no cases in this server.
                </Empty>
              }
            />
            <Pager
              page={pager.page}
              hasPrevious={pager.hasPrevious}
              hasNext={Boolean(cases.data.next_cursor)}
              onPrevious={pager.previous}
              onNext={() => pager.next(cases.data.next_cursor)}
            />
          </section>
        </>
      )}

      <CreateCaseDialog
        guildId={guildId}
        open={creating}
        onClose={() => setCreating(false)}
        initialMember={userId}
      />
    </Page>
  );
}

/**
 * Summary counts the member's whole case history, not just the page on
 * screen: totals, then how many cases fall under each rule.
 */
function Summary({ summary }: { summary: MemberSummary }) {
  const rules = summary.by_rule
    .map((r) => ({ ...r, count: r.valid + r.voided }))
    .filter((r) => r.count > 0)
    .sort((a, b) => b.count - a.count);

  if (summary.total === 0) return null;
  return (
    <div className={s.summary}>
      <div className={s.tiles}>
        <Tile label="Total cases" value={summary.total} />
        <Tile label="Valid" value={summary.valid} hint="Count toward the case count" />
        <Tile label="Voided" value={summary.voided} hint="No longer count" />
      </div>
      {rules.length > 0 ? (
        <Panel>
          <Heading>By rule</Heading>
          <ul className={s.rules}>
            {rules.map((r) => (
              <li key={r.rule_id ?? r.rule_name} className={s.ruleRow}>
                <span className={s.ruleName}>{r.rule_name}</span>
                <span className={s.bar}>
                  <span
                    className={s.fill}
                    style={{
                      width: `${Math.max(4, Math.round((r.count / summary.total) * 100))}%`,
                    }}
                  />
                </span>
                <span className={s.ruleCount}>
                  {plural(r.count, "case")}
                  {r.voided > 0 ? ` (${r.voided.toLocaleString()} voided)` : ""}
                </span>
              </li>
            ))}
          </ul>
        </Panel>
      ) : null}
    </div>
  );
}

function Tile({ label, value, hint }: { label: string; value: number; hint?: string }) {
  return (
    <div className={s.tile}>
      <span className={s.tileValue}>{value.toLocaleString()}</span>
      <span className={s.tileLabel}>{label}</span>
      {hint ? <span className={s.tileHint}>{hint}</span> : null}
    </div>
  );
}

function CopyId({ id }: { id: string }) {
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const timer = window.setTimeout(() => setCopied(false), 1500);
    return () => window.clearTimeout(timer);
  }, [copied]);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(id);
      setCopied(true);
      toast.success("Member ID copied.");
    } catch {
      toast.error("Couldn't copy. Select the ID and copy it yourself.");
    }
  };
  return (
    <Tooltip label="Copy member ID">
      <button type="button" onClick={copy} className={s.copy} data-copied={copied || undefined}>
        <span className={s.id}>{id}</span>
        {copied ? <Check size={14} /> : <Copy size={14} />}
      </button>
    </Tooltip>
  );
}

const EMPTY: CaseSummary[] = [];
