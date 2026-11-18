import { useQuery } from "@tanstack/react-query";
import { createFileRoute, Link } from "@tanstack/react-router";
import { TriangleAlert } from "lucide-react";
import { useMemo, useState } from "react";

import { useCursorPager } from "~/api/paging";
import { failedActionsQuery } from "~/api/queries";
import type { ActionListItem } from "~/api/types";
import { useActionControls } from "~/features/cases/Actions";
import { dismissLabel, mayHaveApplied, retryDone, retryLabel } from "~/features/cases/controls";
import { UserChip } from "~/features/people/User";
import { actionMeta, ago, fullDate, plural } from "~/lib/format";
import { Button, ButtonLink } from "~/ui/Button";
import { ConfirmDialog } from "~/ui/ConfirmDialog";
import { columnsFor, DataTable, features, Pager, useTable } from "~/ui/DataTable";
import { Page } from "~/ui/Page";
import { QuackIcon } from "~/ui/QuackIcon";
import { Segmented } from "~/ui/Segmented";
import { Empty, ErrorState, errorMessage, SkeletonRows } from "~/ui/States";
import { toast } from "~/ui/Toast";
import s from "./failures.module.css";

type Search = { dismissed?: boolean };

export const Route = createFileRoute("/_authed/guilds/$guildId/failures")({
  validateSearch: (raw: Record<string, unknown>): Search => ({
    dismissed: raw.dismissed === true || raw.dismissed === "true" ? true : undefined,
  }),
  loaderDeps: ({ search }) => search,
  loader: ({ context, params, deps }) =>
    context.queryClient.ensureQueryData(
      failedActionsQuery(params.guildId, deps.dismissed ?? false),
    ),
  component: Failures,
});

const col = columnsFor<ActionListItem>();

function Failures() {
  const { guildId } = Route.useParams();
  const { dismissed = false } = Route.useSearch();
  const navigate = Route.useNavigate();
  const pager = useCursorPager(String(dismissed));
  const failures = useQuery(failedActionsQuery(guildId, dismissed, pager.cursor));
  const { retry, dismiss } = useActionControls(guildId);
  const [confirming, setConfirming] = useState<ActionListItem | null>(null);
  const [retryError, setRetryError] = useState<string | null>(null);

  const columns = useMemo(() => {
    const runRetry = (a: ActionListItem, inDialog: boolean) =>
      retry.mutate(a, {
        onSuccess: () => {
          toast.success(retryDone(a.kind, a.case_number));
          setConfirming(null);
        },
        onError: (e) => (inDialog ? setRetryError(errorMessage(e)) : toast.error(errorMessage(e))),
      });
    return col.columns([
      col.accessor("case_number", {
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
      col.accessor("member_id", {
        header: "Member",
        meta: { width: "18%", hideOnMobile: true },
        cell: (c) => <UserChip guildId={guildId} userId={c.getValue()} size={20} />,
      }),
      col.accessor("kind", {
        header: "Action",
        meta: { width: 150 },
        cell: (c) => {
          const meta = actionMeta[c.getValue()];
          return (
            <span className={s.action}>
              <QuackIcon name={meta.icon} size={20} />
              <span className={s.strong}>{meta.label}</span>
            </span>
          );
        },
      }),
      col.accessor("outcome_text", {
        header: "What happened",
        cell: (c) => {
          const row = c.row.original;
          return (
            <div className={s.stack}>
              <span className={s.error} title={c.getValue()}>
                {c.getValue()}
              </span>
              <span className={s.sub}>
                {row.rule_name}
                {mayHaveApplied(row) ? " · May already have happened" : ""}
              </span>
            </div>
          );
        },
      }),
      col.accessor("attempts", {
        header: "Attempts",
        meta: { width: 84, align: "end", hideOnMobile: true },
        cell: (c) => (
          <span className={s.sub} title={plural(c.getValue(), "attempt")}>
            {c.getValue()}
          </span>
        ),
      }),
      col.accessor("created_at", {
        header: "When",
        meta: { width: 110, align: "end", hideOnMobile: true },
        cell: (c) => (
          <time dateTime={c.getValue()} title={fullDate(c.getValue())} className={s.sub}>
            {ago(c.getValue())}
          </time>
        ),
      }),
      col.display({
        id: "controls",
        header: "",
        meta: { width: 320, align: "end" },
        cell: (c) => {
          const row = c.row.original;
          return (
            <div className={s.buttons}>
              {row.can_retry ? (
                <Button
                  size="sm"
                  pending={retry.isPending && retry.variables?.id === row.id}
                  onClick={() => (mayHaveApplied(row) ? setConfirming(row) : runRetry(row, false))}
                >
                  {retryLabel(row.kind)}
                </Button>
              ) : null}
              {row.can_dismiss ? (
                <Button
                  size="sm"
                  variant="secondary"
                  pending={dismiss.isPending && dismiss.variables?.id === row.id}
                  onClick={() => dismiss.mutate(row)}
                >
                  {dismissLabel(row.kind)}
                </Button>
              ) : null}
              <ButtonLink
                size="sm"
                variant="ghost"
                to="/guilds/$guildId/cases/$caseRef"
                params={{ guildId, caseRef: String(row.case_number) }}
              >
                Open case
              </ButtonLink>
            </div>
          );
        },
      }),
    ]);
  }, [guildId, retry, dismiss]);

  const table = useTable({ features, columns, data: failures.data?.actions ?? EMPTY });

  return (
    <Page
      icon={<TriangleAlert size={22} />}
      title="Failed actions"
      topic="Timeouts, kicks, and bans Quack couldn't carry out. Retry them, dismiss them, or open the case."
    >
      <div className={s.filters}>
        <Segmented
          label="Show"
          value={dismissed ? "dismissed" : "open"}
          onChange={(v) => void navigate({ search: { dismissed: v === "dismissed" || undefined } })}
          options={[
            { value: "open", label: "Needs a decision" },
            { value: "dismissed", label: "Dismissed" },
          ]}
        />
      </div>
      {failures.isPending ? (
        <SkeletonRows rows={6} />
      ) : failures.isError ? (
        <ErrorState error={failures.error} retry={() => void failures.refetch()} />
      ) : (
        <>
          {!dismissed && failures.data.actions.length > 0 ? (
            <p className={s.intro}>
              Quack tries again on its own while it can. These need a person: usually Quack's role
              is below the member's, or it's missing a permission. Fix that in Discord, then retry.
              Dismissing takes an action off this list; the case keeps its history.
            </p>
          ) : null}
          <DataTable
            table={table}
            dim={failures.isPlaceholderData}
            empty={
              dismissed ? (
                <Empty icon="success" title="No dismissed actions">
                  Failed actions you dismiss show up here.
                </Empty>
              ) : (
                <Empty icon="success" title="No failed actions">
                  Quack lists any timeout, kick, or ban it couldn't carry out here.
                </Empty>
              )
            }
          />
          <Pager
            page={pager.page}
            hasPrevious={pager.hasPrevious}
            hasNext={Boolean(failures.data.next_cursor)}
            onPrevious={pager.previous}
            onNext={() => pager.next(failures.data.next_cursor)}
          />
        </>
      )}
      <ConfirmDialog
        open={confirming !== null}
        onClose={() => {
          setConfirming(null);
          setRetryError(null);
        }}
        title={confirming ? `${retryLabel(confirming.kind)}?` : "Retry?"}
        description="Quack couldn't confirm whether Discord applied this action. Check the member in Discord first, so it isn't applied twice."
        confirmLabel={confirming ? retryLabel(confirming.kind) : "Retry"}
        pending={retry.isPending}
        error={retryError}
        onConfirm={() =>
          confirming &&
          retry.mutate(confirming, {
            onSuccess: () => {
              toast.success(retryDone(confirming.kind, confirming.case_number));
              setConfirming(null);
            },
            onError: (e) => setRetryError(errorMessage(e)),
          })
        }
      />
    </Page>
  );
}

const EMPTY: ActionListItem[] = [];
