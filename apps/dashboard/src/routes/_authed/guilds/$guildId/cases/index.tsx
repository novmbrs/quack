import { useQuery } from "@tanstack/react-query";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { FolderOpen, Hash, Plus, X } from "lucide-react";
import { useMemo, useState } from "react";

import { useCursorPager } from "~/api/paging";
import { casesQuery, type CaseFilters, rulesQuery } from "~/api/queries";
import type { ActionState, CaseSummary } from "~/api/types";
import { caseColumns } from "~/features/cases/columns";
import { CreateCaseDialog } from "~/features/cases/CreateCaseDialog";
import { MemberPicker } from "~/features/people/MemberPicker";
import { useCan } from "~/lib/permissions";
import { Button } from "~/ui/Button";
import { DataTable, features, Pager, useTable } from "~/ui/DataTable";
import { Select, TextInput } from "~/ui/Field";
import { Page } from "~/ui/Page";
import { Segmented } from "~/ui/Segmented";
import { Empty, ErrorState, SkeletonRows } from "~/ui/States";

import s from "./cases.module.css";

const PAGE = 50;

type CaseState = "valid" | "voided";
const actionStates: ActionState[] = ["done", "failed", "cancelled", "skipped"];

type Search = {
  state?: CaseState;
  rule?: string;
  member?: string;
  action?: ActionState;
  number?: number;
};

export const Route = createFileRoute("/_authed/guilds/$guildId/cases/")({
  validateSearch: (s: Record<string, unknown>): Search => ({
    state: s.state === "valid" || s.state === "voided" ? s.state : undefined,
    rule: str(s.rule),
    member: str(s.member),
    action: actionStates.includes(s.action as ActionState) ? (s.action as ActionState) : undefined,
    number: num(s.number),
  }),
  loaderDeps: ({ search }) => search,
  loader: ({ context, params, deps }) =>
    context.queryClient.ensureQueryData(casesQuery(params.guildId, filters(deps))),
  component: Cases,
});

function filters(s: Search): CaseFilters {
  return {
    limit: PAGE,
    state: s.state,
    rule_id: s.rule,
    member_id: s.member,
    action_state: s.action,
    number: s.number,
  };
}

function Cases() {
  const { guildId } = Route.useParams();
  const search = Route.useSearch();
  const navigate = useNavigate({ from: Route.fullPath });
  const can = useCan(guildId);
  const [creating, setCreating] = useState(false);
  const [number, setNumber] = useState(search.number ? String(search.number) : "");

  const pager = useCursorPager(JSON.stringify(filters(search)));
  const cases = useQuery(casesQuery(guildId, filters(search), pager.cursor));
  const active = useQuery(rulesQuery(guildId));
  const archived = useQuery(rulesQuery(guildId, true));
  const rules = [...(active.data ?? []), ...(archived.data ?? [])];

  const set = (patch: Partial<Search>) =>
    void navigate({ search: (prev) => ({ ...prev, ...patch }) });

  const columns = useMemo(() => caseColumns(guildId), [guildId]);

  const table = useTable({ features, columns, data: cases.data?.cases ?? EMPTY });
  const filtered = Boolean(
    search.state || search.rule || search.member || search.action || search.number,
  );

  return (
    <Page
      icon={<FolderOpen size={22} />}
      title="Cases"
      topic="Every case in this server, newest first."
      actions={
        can("moderate") ? (
          <Button size="sm" icon={<Plus size={16} />} onClick={() => setCreating(true)}>
            Add case
          </Button>
        ) : null
      }
    >
      <div className={s.filters}>
        <Segmented
          label="Status"
          value={search.state ?? "all"}
          onChange={(v) => set({ state: v === "all" ? undefined : (v as CaseState) })}
          options={[
            { value: "all", label: "All" },
            { value: "valid", label: "Valid" },
            { value: "voided", label: "Voided" },
          ]}
        />
        <form
          className={s.numberForm}
          onSubmit={(e) => {
            e.preventDefault();
            const n = Number.parseInt(number, 10);
            set({ number: Number.isFinite(n) && n > 0 ? n : undefined });
          }}
        >
          <TextInput
            aria-label="Case number"
            placeholder="Case #"
            inputMode="numeric"
            value={number}
            leading={<Hash size={14} />}
            onChange={(e) => setNumber(e.currentTarget.value.replace(/\D/g, ""))}
            onBlur={() => {
              const n = Number.parseInt(number, 10);
              if ((Number.isFinite(n) ? n : undefined) !== search.number)
                set({ number: Number.isFinite(n) && n > 0 ? n : undefined });
            }}
          />
        </form>
        <div className={s.member}>
          <MemberPicker
            guildId={guildId}
            value={search.member ?? null}
            onChange={(id) => set({ member: id ?? undefined })}
            placeholder="Filter by member"
          />
        </div>
        <Select
          aria-label="Rule"
          value={search.rule ?? ""}
          onChange={(e) => set({ rule: e.currentTarget.value || undefined })}
          className={s.select}
        >
          <option value="">Any rule</option>
          {rules.map((r) => (
            <option key={r.id} value={r.id}>
              {r.name}
              {r.archived_at ? " (archived)" : ""}
            </option>
          ))}
        </Select>
        <Select
          aria-label="Action outcome"
          value={search.action ?? ""}
          onChange={(e) =>
            set({ action: (e.currentTarget.value || undefined) as ActionState | undefined })
          }
          className={s.select}
        >
          <option value="">Any outcome</option>
          <option value="done">Action done</option>
          <option value="failed">Action failed</option>
          <option value="cancelled">Action cancelled</option>
          <option value="skipped">Action left in place</option>
        </Select>
        {filtered ? (
          <Button
            variant="ghost"
            size="sm"
            icon={<X size={14} />}
            onClick={() => {
              setNumber("");
              void navigate({ search: {} });
            }}
          >
            Clear
          </Button>
        ) : null}
      </div>

      {cases.isPending ? (
        <SkeletonRows rows={10} />
      ) : cases.isError ? (
        <ErrorState error={cases.error} retry={() => void cases.refetch()} />
      ) : (
        <>
          <DataTable
            table={table}
            dim={cases.isPlaceholderData}
            minWidth={560}
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
              filtered ? (
                <Empty icon="search" title="No cases match">
                  Try a different filter, or clear them to see everything.
                </Empty>
              ) : (
                <Empty
                  icon="case"
                  title="No cases yet"
                  action={
                    can("moderate") ? (
                      <Button onClick={() => setCreating(true)}>Add the first case</Button>
                    ) : null
                  }
                >
                  Cases show up here when a moderator applies a rule, from Discord with /case add or
                  right here.
                </Empty>
              )
            }
          />
          <Pager
            page={pager.page}
            hasPrevious={pager.hasPrevious}
            hasNext={Boolean(cases.data.next_cursor)}
            onPrevious={pager.previous}
            onNext={() => pager.next(cases.data.next_cursor)}
          />
        </>
      )}

      <CreateCaseDialog
        guildId={guildId}
        open={creating}
        onClose={() => setCreating(false)}
        initialMember={search.member}
      />
    </Page>
  );
}

const EMPTY: CaseSummary[] = [];

function num(v: unknown): number | undefined {
  const n = typeof v === "number" ? v : typeof v === "string" ? Number.parseInt(v, 10) : Number.NaN;
  return Number.isFinite(n) && n > 0 ? n : undefined;
}

function str(v: unknown): string | undefined {
  return typeof v === "string" && v ? v : typeof v === "number" ? String(v) : undefined;
}
